// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! **A claim confines the device to the client that took it.**
//!
//! SECURITY.md names "client isolation" as in scope. `Claim` records the caller's unique bus name
//! and every later method compares against it, so the whole session model rests on that one
//! comparison — and on it being made by *every* method, not most of them.
//!
//! ## Honest limits
//!
//! A "client" here is a distinct unique bus name, i.e. a separate `zbus::Connection`. Both run as
//! the same uid, so **this proves sender-keyed isolation — which is what `Session.sender` actually
//! keys on** — and proves nothing about two real users. Two uids would need two processes running
//! as two users, and the daemon would not behave differently: it never compares uids.
//!
//! One test here records a fact rather than enforcing a boundary; it says so where it sits.

#![cfg(target_os = "linux")]

mod common;
use common::{Harness, PrivateBus};

use std::time::Duration;

use fprint_backend_native::{
    EnrollScript, FingerId, Scenario, VirtualBackend, VirtualDeviceBuilder,
};
use fprintd::ActionSet;
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

const IN_USE: &str = "net.reactivated.Fprint.Error.AlreadyInUse";

/// The D-Bus error name a call failed with, or `None` if it succeeded.
fn error_name<T>(result: zbus::Result<T>) -> Option<String> {
    match result {
        Ok(_) => None,
        Err(zbus::Error::MethodError(name, _, _)) => Some(name.as_str().to_string()),
        Err(e) => panic!("expected a D-Bus method error, got {e:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_client_cannot_claim_or_operate_on_a_claim_it_does_not_own() {
    let _bus = PrivateBus::shared();
    let harness = Harness::serve("IsolationOwn", ActionSet::ALL, backend).await;

    let a_conn = harness.client().await;
    let a = harness.device(&a_conn).await;
    a.claim("").await.expect("A claims");

    let b_conn = harness.client().await;
    let b = harness.device(&b_conn).await;

    // Every method that requires a claim, from a client that does not hold it. One gate added and
    // another forgotten is exactly the shape of this mistake, so the list is the whole matrix
    // rather than a sample.
    assert_eq!(
        error_name(b.claim("").await).as_deref(),
        Some(IN_USE),
        "Claim"
    );
    assert_eq!(
        error_name(b.verify_start("any").await).as_deref(),
        Some(IN_USE),
        "VerifyStart"
    );
    assert_eq!(
        error_name(b.enroll_start("right-index-finger").await).as_deref(),
        Some(IN_USE),
        "EnrollStart"
    );
    assert_eq!(
        error_name(b.verify_stop().await).as_deref(),
        Some(IN_USE),
        "VerifyStop"
    );
    assert_eq!(
        error_name(b.enroll_stop().await).as_deref(),
        Some(IN_USE),
        "EnrollStop"
    );
    assert_eq!(
        error_name(b.delete_enrolled_fingers2().await).as_deref(),
        Some(IN_USE),
        "DeleteEnrolledFingers2"
    );
    assert_eq!(
        error_name(b.delete_enrolled_finger("right-index-finger").await).as_deref(),
        Some(IN_USE),
        "DeleteEnrolledFinger"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_release_by_a_non_owner_does_not_close_the_owners_sensor() {
    let _bus = PrivateBus::shared();
    let harness = Harness::serve("IsolationRelease", ActionSet::ALL, backend).await;

    let a_conn = harness.client().await;
    let a = harness.device(&a_conn).await;
    a.claim("").await.expect("A claims");

    let b_conn = harness.client().await;
    let b = harness.device(&b_conn).await;
    assert_eq!(
        error_name(b.release().await).as_deref(),
        Some(IN_USE),
        "B must not release A's claim"
    );

    // The interesting half. A refusal that had already torn the sensor down would leave A holding
    // a claim over a closed device — refused, and yet effective.
    a.enroll_start("right-index-finger")
        .await
        .expect("A's session must survive B's failed Release");
    a.enroll_stop().await.expect("enroll stop");
    a.release().await.expect("A releases its own claim");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_double_claim_is_refused_and_leaves_the_first_claim_intact() {
    let _bus = PrivateBus::shared();
    let harness = Harness::serve("IsolationDouble", ActionSet::ALL, backend).await;

    let conn = harness.client().await;
    let device = harness.device(&conn).await;
    device.claim("").await.expect("first claim");
    assert_eq!(
        error_name(device.claim("").await).as_deref(),
        Some(IN_USE),
        "the same client claiming twice is still already in use"
    );

    // The guard returns before touching the stored session — so the first claim survives. Only
    // that early return holds this; a guard that cleared and re-took would be invisible here
    // without the check.
    device
        .enroll_start("right-index-finger")
        .await
        .expect("the first claim must survive a refused second one");
    device.enroll_stop().await.expect("enroll stop");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_operation_is_refused_while_one_is_in_flight() {
    let _bus = PrivateBus::shared();
    let harness = Harness::serve("IsolationBusy", ActionSet::ALL, backend).await;

    let conn = harness.client().await;
    let device = harness.device(&conn).await;
    device.claim("").await.expect("claim");

    // Two enrolments rather than two verifies: enrolment needs no existing print, so the second
    // call can fail only for the reason under test. `VerifyStart` would refuse an empty store
    // first, and a test that passes for the wrong reason is worse than none.
    device
        .enroll_start("right-index-finger")
        .await
        .expect("first enroll");
    assert_eq!(
        error_name(device.enroll_start("left-thumb").await).as_deref(),
        Some(IN_USE),
        "one operation in flight per device"
    );
    device.enroll_stop().await.expect("enroll stop");
}

/// Signals reach clients that never claimed, and that is the D-Bus policy's job rather than the
/// daemon's — but the payload is the thing worth pinning.
///
/// zbus's `SignalEmitter` broadcasts, and so does upstream fprintd; what restricts who sees a
/// signal is the policy file the fprintd package ships, which this project deliberately borrows
/// rather than duplicates (ARCHITECTURE.md §Coexistence). **So this test records a fact it does not
/// enforce, and pins the property that makes the fact tolerable: the payload carries a status
/// string and a bool. No template bytes, no username, nothing an eavesdropper could enrol with.**
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn signals_reach_other_clients_and_carry_no_template_or_username() {
    let _bus = PrivateBus::shared();
    let harness = Harness::serve("IsolationSignals", ActionSet::ALL, backend).await;

    let a_conn = harness.client().await;
    let a = harness.device(&a_conn).await;
    a.claim("").await.expect("A claims");

    // B never claimed anything.
    let b_conn = harness.client().await;
    let b = harness.device(&b_conn).await;
    let mut watching = b.receive_enroll_status().await.expect("subscribe");

    a.enroll_start("right-index-finger")
        .await
        .expect("enroll start");

    let signal = timeout(Duration::from_secs(5), watching.next())
        .await
        .expect("an unclaimed client does see the signals")
        .expect("stream ended");
    let args = signal.args().expect("signal args");

    // The whole payload, by construction: `(result: String, done: bool)`. The status vocabulary is
    // fprintd's own, and none of it is secret.
    assert!(
        !args.result().is_empty(),
        "a status is a vocabulary word, and there must be one"
    );
    assert!(
        !args.result().contains("root") && !args.result().contains('/'),
        "a status word must not carry a username or a path: {}",
        args.result()
    );

    a.enroll_stop().await.expect("enroll stop");
}
