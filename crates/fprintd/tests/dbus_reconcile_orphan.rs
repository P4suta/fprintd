// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Reconciliation frees an orphaned on-sensor slot at `Claim`.
//!
//! An unclaimed, host-only `DeleteEnrolledFingers` removes the host record but cannot reach the
//! sensor, orphaning the on-chip slot. The next `Claim` runs `reconcile_host_with_device`, which
//! must free that orphan (the "device ← host" direction: drop any on-sensor slot the host no longer
//! records, when the reader advertises `STORAGE_DELETE`). Without the fix the slot survives, and
//! re-enrolling the same finger fails the reader's `DUPLICATES_CHECK`.
//!
//! The test uses a `shared_storage` match-on-chip sensor so the slot persists across the open/close
//! the daemon drives on each `Claim`, and a [`SharedStorage`] handle to observe the slot count
//! directly — the one thing the D-Bus surface never exposes.

#![cfg(target_os = "linux")]

mod common;
use common::{DeviceProxy, ManagerProxy, PrivateBus};

use std::sync::Arc;
use std::time::Duration;

use fprint_backend_native::{
    EnrollScript, FingerId, Scenario, VirtualBackend, VirtualDeviceBuilder,
};
use fprintd::{ActionSet, Authorizer, Daemon, Store};
use futures_util::StreamExt;
use tokio::time::timeout;

/// Drive one enrollment of `finger_name` to its terminal status and return that status string.
async fn enroll_terminal(device: &DeviceProxy<'_>, finger_name: &str) -> String {
    let mut stream = device.receive_enroll_status().await.expect("enroll stream");
    device
        .enroll_start(finger_name)
        .await
        .expect("enroll start");
    let terminal = timeout(Duration::from_secs(5), async {
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
    terminal
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claim_reconciliation_frees_orphaned_on_sensor_slot() {
    let _bus = PrivateBus::shared();

    let tmp = std::env::temp_dir().join(format!("fprintd-orphan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);

    // A match-on-chip sensor whose on-chip store persists across the open/close the daemon drives on
    // each `Claim` — without `shared_storage` the device rebuilds empty and no orphan can exist.
    let builder = VirtualDeviceBuilder::chip_storage_sensor()
        .shared_storage()
        .scenario(Scenario::new().enroll(EnrollScript::default().produces(FingerId(1))));
    let storage = builder.storage_handle().expect("shared storage handle");
    let factory = move || VirtualBackend::single(builder.clone());

    let daemon = Daemon::with_store(
        factory,
        Arc::new(Authorizer::Fixed(ActionSet::ALL)),
        Store::with_root(tmp.clone()),
    );
    let bus_name = "net.reactivated.Fprint.ReconcileOrphan";
    let conn_builder = zbus::connection::Builder::session()
        .expect("session bus")
        .name(bus_name)
        .expect("request name");
    let _daemon_conn = daemon.attach(conn_builder).await.expect("attach daemon");

    // Client side.
    let client = zbus::Connection::session().await.expect("client session");
    let manager = ManagerProxy::builder(&client)
        .destination(bus_name)
        .expect("destination")
        .build()
        .await
        .expect("manager proxy");
    let devices = manager.get_devices().await.expect("get devices");
    assert_eq!(devices.len(), 1, "one virtual device expected");
    let device = DeviceProxy::builder(&client)
        .destination(bus_name)
        .expect("destination")
        .path(devices[0].clone())
        .expect("device path")
        .build()
        .await
        .expect("device proxy");

    // 1. Enroll: writes a host record and an on-sensor slot, then release.
    device.claim("").await.expect("claim");
    assert_eq!(
        enroll_terminal(&device, "right-index-finger").await,
        "enroll-completed",
        "initial enroll should complete",
    );
    assert_eq!(storage.len(), 1, "one slot on the sensor after enroll");
    device.release().await.expect("release");

    // 2. Unclaimed, legacy host-only delete: drops the host record but cannot touch the sensor, so
    //    the on-chip slot is orphaned.
    device
        .delete_enrolled_fingers("")
        .await
        .expect("delete enrolled fingers (unclaimed)");
    assert_eq!(
        storage.len(),
        1,
        "the on-sensor slot is orphaned — host record gone, sensor slot survives",
    );

    // 3. Re-claim: reconciliation runs and must free the orphaned slot (device ← host).
    device.claim("").await.expect("re-claim");
    assert!(
        storage.is_empty(),
        "reconciliation must free the orphaned on-sensor slot at Claim",
    );

    // 4. Re-enroll the same finger: with the orphan freed this reaches enroll-completed; the old
    //    code left the slot in place and the reader's DUPLICATES_CHECK would fail this enroll.
    assert_eq!(
        enroll_terminal(&device, "right-index-finger").await,
        "enroll-completed",
        "re-enroll after reconciliation must not trip DUPLICATES_CHECK",
    );
    device.release().await.expect("final release");

    let _ = std::fs::remove_dir_all(&tmp);
}

/// **Reconciliation frees only the *claiming user's* orphans, never another user's slot.**
///
/// Reconcile matches slots to users by their stored owner (username). A second user merely claiming
/// a shared match-on-chip reader must not touch the first user's on-sensor template. (Regression:
/// the device←host prune once matched on finger alone and freed any slot the claimant did not
/// record — so any user's claim silently wiped every other user's enrollment.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_users_claim_keeps_the_first_users_slot() {
    let _bus = PrivateBus::shared();

    let tmp = std::env::temp_dir().join(format!("fprintd-xuser-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);

    let builder = VirtualDeviceBuilder::chip_storage_sensor()
        .shared_storage()
        .scenario(Scenario::new().enroll(EnrollScript::default().produces(FingerId(1))));
    let storage = builder.storage_handle().expect("shared storage handle");
    let factory = move || VirtualBackend::single(builder.clone());

    let daemon = Daemon::with_store(
        factory,
        Arc::new(Authorizer::Fixed(ActionSet::ALL)),
        Store::with_root(tmp.clone()),
    );
    let bus_name = "net.reactivated.Fprint.ReconcileXUser";
    let conn_builder = zbus::connection::Builder::session()
        .expect("session bus")
        .name(bus_name)
        .expect("request name");
    let _daemon_conn = daemon.attach(conn_builder).await.expect("attach daemon");

    let client = zbus::Connection::session().await.expect("client session");
    let manager = ManagerProxy::builder(&client)
        .destination(bus_name)
        .expect("destination")
        .build()
        .await
        .expect("manager proxy");
    let devices = manager.get_devices().await.expect("get devices");
    let device = DeviceProxy::builder(&client)
        .destination(bus_name)
        .expect("destination")
        .path(devices[0].clone())
        .expect("device path")
        .build()
        .await
        .expect("device proxy");

    // Alice enrolls a finger — one slot on the sensor, owned by alice. (Fixed(ALL) grants
    // SetUsername, so claiming as an explicit username works even though the OS user differs.)
    device.claim("alice").await.expect("claim alice");
    assert_eq!(
        enroll_terminal(&device, "right-index-finger").await,
        "enroll-completed",
        "alice's enroll should complete",
    );
    device.release().await.expect("release alice");
    assert_eq!(
        storage.len(),
        1,
        "one slot on the sensor after alice enrolls"
    );

    // Bob merely claims the same reader. Reconciliation runs — and must leave alice's slot alone.
    device.claim("bob").await.expect("claim bob");
    assert_eq!(
        storage.len(),
        1,
        "bob's claim must not free alice's on-sensor slot",
    );
    assert_eq!(
        storage.fingers(),
        vec![fprint_core::Finger::RightIndex],
        "the surviving slot is still alice's right index",
    );
    device.release().await.expect("release bob");

    let _ = std::fs::remove_dir_all(&tmp);
}
