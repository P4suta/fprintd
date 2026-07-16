// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Hardware-free, end-to-end exercise of the `net.reactivated.Fprint` D-Bus surface.
//!
//! It stands the daemon up over `fprint-backend-native`'s virtual backend and an
//! [`Authorizer::Fixed`] granting every action, on a bus connection, then drives it as a real client would:
//! `GetDevices` → `Claim` → `EnrollStart` → wait for `enroll-completed` → `VerifyStart` →
//! wait for `verify-match` → `Release`. Because the enrolled print is written to disk as FP3
//! and read back for verification, this also covers the storage + [`fprint_fp3`] round-trip.
//!
//! It is fully self-contained: [`common::PrivateBus`] spawns its own `dbus-daemon` for the
//! test's lifetime, so a plain `cargo test` works with no ambient D-Bus or `dbus-run-session`
//! wrapper.

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

/// A virtual host-image sensor that enrolls and then recognises finger identity `2`.
fn backend() -> VirtualBackend {
    VirtualBackend::single(
        VirtualDeviceBuilder::host_image_sensor().scenario(
            Scenario::new()
                .enroll(EnrollScript::default().produces(FingerId(2)))
                .present(FingerId(2)),
        ),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enroll_then_verify_over_dbus() {
    // A private session bus, so the test needs no ambient D-Bus (self-contained under a plain
    // `cargo test`). Must outlive both connections.
    let _bus = PrivateBus::shared();

    let tmp = std::env::temp_dir().join(format!("fprintd-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);

    // Stand up the daemon on the session bus. The daemon takes a backend *factory*; `backend`
    // is an `fn() -> VirtualBackend`, which is `Fn + Clone + Send + Sync + 'static`, so it
    // serves directly — each actor thread builds its own virtual device from it.
    let daemon = Daemon::with_store(
        backend,
        Arc::new(Authorizer::Fixed(ActionSet::ALL)),
        Store::with_root(tmp.clone()),
    );
    let builder = zbus::connection::Builder::session()
        .expect("session bus")
        .name("net.reactivated.Fprint")
        .expect("request name");
    let _daemon_conn = daemon.attach(builder).await.expect("attach daemon");

    // Client side.
    let client = zbus::Connection::session().await.expect("client session");
    let manager = ManagerProxy::new(&client).await.expect("manager proxy");

    let devices = manager.get_devices().await.expect("get devices");
    assert_eq!(devices.len(), 1, "one virtual device expected");
    let device_path = devices[0].clone();

    // Disable property caching: the daemon does not emit PropertiesChanged for
    // `num-enroll-stages`, so each read must hit the wire.
    let device = DeviceProxy::builder(&client)
        .path(device_path)
        .expect("device path")
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
        .expect("device proxy");

    // Before claim, num-enroll-stages is undefined (-1).
    assert_eq!(device.num_enroll_stages().await.expect("stages"), -1);

    device.claim("").await.expect("claim");
    assert_eq!(device.num_enroll_stages().await.expect("stages"), 5);

    // Enroll, waiting for the terminal enroll-completed.
    let mut enroll_stream = device.receive_enroll_status().await.expect("enroll stream");
    device
        .enroll_start("left-index-finger")
        .await
        .expect("enroll start");
    let completed = timeout(Duration::from_secs(5), async {
        while let Some(sig) = enroll_stream.next().await {
            let args = sig.args().expect("enroll args");
            if args.done {
                return args.result.to_string();
            }
        }
        String::from("<stream ended>")
    })
    .await
    .expect("enroll timed out");
    assert_eq!(completed, "enroll-completed");
    device.enroll_stop().await.expect("enroll stop");

    // The finger is now listed.
    let fingers = device.list_enrolled_fingers("").await.expect("list");
    assert_eq!(fingers, vec!["left-index-finger".to_string()]);

    // Verify, waiting for verify-match.
    let mut verify_stream = device.receive_verify_status().await.expect("verify stream");
    device
        .verify_start("left-index-finger")
        .await
        .expect("verify start");
    let result = timeout(Duration::from_secs(5), async {
        while let Some(sig) = verify_stream.next().await {
            let args = sig.args().expect("verify args");
            if args.done {
                return args.result.to_string();
            }
        }
        String::from("<stream ended>")
    })
    .await
    .expect("verify timed out");
    assert_eq!(result, "verify-match");

    device.verify_stop().await.expect("verify stop");
    device.release().await.expect("release");

    let _ = std::fs::remove_dir_all(&tmp);
}
