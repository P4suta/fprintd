// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! **Concurrent `VerifyStart` calls on one device spawn exactly one pump.**
//!
//! zbus dispatches `&self` interface methods concurrently across the runtime's worker threads (it
//! does not serialise per object), so the "one operation in flight" guard must claim the slot and
//! spawn the pump under a single lock. (Regression: the guard was two lock scopes — check idle, then
//! store the active op — so a burst of `VerifyStart` calls could all pass the check together, each
//! spawn a pump, and every pump but the last be orphaned with no way for `VerifyStop` to reach it.)

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

/// A host-image sensor whose verify **hangs** after reporting the finger present, so a winning
/// pump stays alive (its `ActiveOp` unfinished) for the whole burst — exactly the state in which a
/// second, orphaned pump would be observable.
fn backend() -> VirtualBackend {
    VirtualBackend::single(
        VirtualDeviceBuilder::host_image_sensor().scenario(
            Scenario::new()
                .enroll(EnrollScript::default().produces(FingerId(2)))
                .present(FingerId(2))
                .hang(),
        ),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_verify_start_spawns_exactly_one() {
    let _bus = PrivateBus::shared();

    let tmp = std::env::temp_dir().join(format!("fprintd-toctou-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);

    let daemon = Daemon::with_store(
        backend,
        Arc::new(Authorizer::Fixed(ActionSet::ALL)),
        Store::with_root(tmp.clone()),
    );
    let bus_name = "net.reactivated.Fprint.Toctou";
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

    // Claim and enroll a finger so `VerifyStart`'s print lookup succeeds (the race is in the guard,
    // not the lookup).
    device.claim("").await.expect("claim");
    let mut enroll_stream = device.receive_enroll_status().await.expect("enroll stream");
    device
        .enroll_start("left-index-finger")
        .await
        .expect("enroll start");
    let done = timeout(Duration::from_secs(5), async {
        while let Some(sig) = enroll_stream.next().await {
            if sig.args().expect("enroll args").done {
                return true;
            }
        }
        false
    })
    .await
    .expect("enroll timed out");
    assert!(done, "enroll should complete");
    device.enroll_stop().await.expect("enroll stop");

    // Fire a burst of concurrent VerifyStart calls on the one claimed device. With the atomic guard
    // exactly one wins the single-in-flight slot; the rest get AlreadyInUse. (Repeated bursts, since
    // the race is timing-dependent — the buggy guard let *most* bursts double-spawn.)
    for round in 0..40 {
        let results = futures_util::future::join_all(
            (0..48).map(|_| device.verify_start("left-index-finger")),
        )
        .await;
        let wins = results.iter().filter(|r| r.is_ok()).count();
        assert_eq!(
            wins, 1,
            "round {round}: exactly one concurrent VerifyStart may claim the slot (got {wins})",
        );
        // Stop the winner's (hanging) verify so the next round starts idle.
        device.verify_stop().await.expect("verify stop");
    }

    device.release().await.expect("release");
    let _ = std::fs::remove_dir_all(&tmp);
}
