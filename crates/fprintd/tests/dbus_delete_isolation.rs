// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A claimed "delete all my fingers" on a match-on-chip reader that supports both per-template
//! delete (`STORAGE_DELETE`) and wholesale wipe (`STORAGE_CLEAR`) must PREFER per-template delete:
//! it frees exactly the on-sensor slots its host records name, and leaves every other slot on the
//! sensor intact. The old code preferred `STORAGE_CLEAR` and wiped the whole sensor.
//!
//! The observable is the shared on-chip store itself (`.shared_storage()` + `storage_handle()`) —
//! the one thing the D-Bus surface never exposes directly. The sensor is driven to hold two slots
//! while the claimed user has a single host record backing one of them, so the two paths differ
//! visibly: per-template delete frees the one recorded slot and leaves the other (`len 2 -> 1`),
//! whereas a wholesale clear would take both (`len 2 -> 0`).
//!
//! Standing that up: `DUPLICATES_CHECK` is dropped from the advertised features so the sensor
//! accepts a second enrollment of the same scenario template without a duplicate error, and the
//! daemon's per-finger host store keeps a single record for the (single) finger name enrolled.
//! Two enrollments of that finger therefore leave two identical slots on the sensor but one host
//! record — the surplus slot is exactly what `STORAGE_CLEAR` would wrongly destroy. Finger identity
//! is beside the point here: the fix is that a claimed delete-all frees slots one-by-one against the
//! host records rather than wiping the sensor.

#![cfg(target_os = "linux")]

mod common;
use common::{DeviceProxy, ManagerProxy, PrivateBus};

use std::sync::Arc;
use std::time::Duration;

use fprint_backend_native::{
    EnrollScript, FingerId, Scenario, VirtualBackend, VirtualDeviceBuilder,
};
use fprint_core::DeviceFeature;
use fprintd::{ActionSet, Authorizer, Daemon, Store};
use futures_util::StreamExt;
use tokio::time::timeout;

/// Enroll `finger_name` on `device` and block until the terminal enroll-status arrives, returning
/// its result string ("enroll-completed" on success).
async fn enroll(device: &DeviceProxy<'_>, finger_name: &str) -> String {
    let mut stream = device.receive_enroll_status().await.expect("enroll stream");
    device
        .enroll_start(finger_name)
        .await
        .expect("enroll start");
    let result = timeout(Duration::from_secs(5), async {
        while let Some(sig) = stream.next().await {
            let args = sig.args().expect("enroll args");
            if args.done {
                return args.result.to_string();
            }
        }
        String::from("<stream ended>")
    })
    .await
    .expect("enroll timed out");
    device.enroll_stop().await.expect("enroll stop");
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_all_frees_per_template_not_a_wholesale_wipe() {
    let _bus = PrivateBus::shared();

    let tmp = std::env::temp_dir().join(format!("fprintd-delete-isolation-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);

    // A match-on-chip sensor that keeps per-template delete AND wholesale wipe, but drops
    // DUPLICATES_CHECK so a second enrollment of the same scenario template is not rejected as a
    // duplicate. `.shared_storage()` makes every device this builder mints share one on-chip store,
    // so slots survive the open/close the daemon drives on each Claim — modelling real MOC hardware.
    let builder = VirtualDeviceBuilder::chip_storage_sensor()
        .features(
            DeviceFeature::VERIFY
                | DeviceFeature::IDENTIFY
                | DeviceFeature::STORAGE
                | DeviceFeature::STORAGE_LIST
                | DeviceFeature::STORAGE_DELETE
                | DeviceFeature::STORAGE_CLEAR,
        )
        .scenario(Scenario::new().enroll(EnrollScript::default().produces(FingerId(1))))
        .shared_storage();
    let storage = builder.storage_handle().expect("shared storage handle");

    // The daemon takes a backend factory; cloning the builder shares the same on-chip PrintStore
    // Arc as `storage`, so `storage` observes every write the daemon's device makes.
    let factory = move || VirtualBackend::single(builder.clone());
    let daemon = Daemon::with_store(
        factory,
        Arc::new(Authorizer::Fixed(ActionSet::ALL)),
        Store::with_root(tmp.clone()),
    );
    let dbus_builder = zbus::connection::Builder::session()
        .expect("session bus")
        .name("net.reactivated.Fprint.DeleteIsolation")
        .expect("request name");
    let _daemon_conn = daemon.attach(dbus_builder).await.expect("attach daemon");

    // Client side.
    let client = zbus::Connection::session().await.expect("client session");
    let manager = ManagerProxy::builder(&client)
        .destination("net.reactivated.Fprint.DeleteIsolation")
        .expect("destination")
        .build()
        .await
        .expect("manager proxy");
    let devices = manager.get_devices().await.expect("get devices");
    assert_eq!(devices.len(), 1, "one virtual device expected");
    let device = DeviceProxy::builder(&client)
        .destination("net.reactivated.Fprint.DeleteIsolation")
        .expect("destination")
        .path(devices[0].clone())
        .expect("device path")
        .build()
        .await
        .expect("device proxy");

    // 1. Claim, enroll a finger, release. The shared on-sensor store now holds one slot, and it
    //    persists across the release/reclaim precisely because the store is shared.
    device.claim("").await.expect("claim 1");
    assert_eq!(
        enroll(&device, "right-index-finger").await,
        "enroll-completed"
    );
    device.release().await.expect("release 1");
    assert_eq!(storage.len(), 1, "one slot after the first enrollment");

    // 2. Reclaim and enroll the same finger again — still claimed. The sensor now holds two slots
    //    (DUPLICATES_CHECK is off, so the identical template is accepted), while the daemon's
    //    per-finger host store still records exactly one finger. This surplus on-sensor slot is the
    //    stand-in for a template the deleting user's records do not cover — the thing a wholesale
    //    clear would wrongly destroy.
    device.claim("").await.expect("claim 2");
    assert_eq!(
        enroll(&device, "right-index-finger").await,
        "enroll-completed"
    );
    assert_eq!(storage.len(), 2, "two slots after the second enrollment");

    // 3. Claimed "delete all MY fingers", then release.
    device
        .delete_enrolled_fingers2()
        .await
        .expect("delete enrolled fingers2");
    device.release().await.expect("release 2");

    // 4. The crux: delete-all freed slots one-by-one against the host records (2 -> 1), it did not
    //    wipe the sensor (2 -> 0). Exactly one slot survived — the one no host record named. The old
    //    STORAGE_CLEAR-first code left the store EMPTY.
    assert_eq!(
        storage.len(),
        1,
        "per-template delete frees only recorded slots; it is not a wholesale wipe"
    );
    assert!(
        !storage.is_empty(),
        "the surplus slot must remain on the sensor, not be wiped by a clear"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
