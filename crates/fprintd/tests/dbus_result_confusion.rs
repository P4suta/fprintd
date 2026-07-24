// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! **A verify result means one thing, and every client reads the same thing from it.**
//!
//! SECURITY.md names "any way to bypass or confuse a verify/identify result" as in scope. A
//! fingerprint reader answers one question, and the ways to get a wrong answer out of it are not
//! only cryptographic: two clients disagreeing about the same result, or a "no print" answered as
//! "no match", are both a confused result.
//!
//! ## Honest limits — two properties here are not tested, and why
//!
//! * **An out-of-range `match_index` is not a match.** `identify_loop` does
//!   `outcome.match_index.and_then(|i| gallery.get(i))`, which is the daemon's only defence against
//!   a backend that miscounts: `gallery[i]` would panic and `unwrap_or(first)` would be a silent
//!   authentication bypass. Reaching it needs a backend that returns an index it has no print for,
//!   and `fprint-backend-native`'s scenario cannot script one — it has `present(FingerId)` and
//!   nothing that lies. **Untested, and the gap is in the test backend rather than in the daemon.**
//! * **A retry never terminates a verify.** `EnrollScript` can script a retry; the verify path
//!   cannot. Same reason.
//!
//! Both would need a scripting knob on the virtual device. Naming them here is worth more than a
//! test that pretends to reach them.

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

const NO_PRINTS: &str = "net.reactivated.Fprint.Error.NoEnrolledPrints";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn verify_start_for_a_finger_with_no_print_is_refused_not_answered_no_match() {
    let _bus = PrivateBus::shared();
    let harness = Harness::serve("ConfusionNoPrints", ActionSet::ALL, backend).await;
    let conn = harness.client().await;
    let device = harness.device(&conn).await;
    device.claim("").await.expect("claim");

    // "Nothing to compare against" and "compared, and it was not you" are different answers, and a
    // caller that cannot tell them apart cannot tell an unenrolled user from a rejected one.
    match device.verify_start("any").await {
        Err(zbus::Error::MethodError(name, _, _)) => assert_eq!(
            name.as_str(),
            NO_PRINTS,
            "an empty store must be refused, not answered"
        ),
        other => panic!("expected {NO_PRINTS}, got {other:?}"),
    }
}

/// `VerifyFingerMatched` is emitted **before** `VerifyStatus verify-match`.
///
/// The order is an interoperability fact that upstream's `src/device.c` establishes, and clients
/// split on which of the two they read: one waits for the named-finger signal, another for the
/// status word. Emitting them the other way round makes those two clients disagree about the moment
/// a match happened — the same result, read two ways.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn verify_finger_matched_precedes_verify_status_match() {
    let _bus = PrivateBus::shared();
    let harness = Harness::serve("ConfusionOrder", ActionSet::ALL, backend).await;
    let conn = harness.client().await;
    let device = harness.device(&conn).await;
    device.claim("").await.expect("claim");

    // Enrol first: a verify needs something to compare against.
    let mut enrolling = device.receive_enroll_status().await.expect("subscribe");
    device
        .enroll_start("right-index-finger")
        .await
        .expect("enroll start");
    timeout(Duration::from_secs(5), async {
        while let Some(signal) = enrolling.next().await {
            if *signal.args().expect("args").done() {
                return;
            }
        }
        panic!("enroll stream ended before completing");
    })
    .await
    .expect("enroll completes");
    device.enroll_stop().await.expect("enroll stop");

    // Both streams open before the verify starts, so neither can miss its signal.
    let mut matched = device.receive_verify_finger_matched().await.expect("sub");
    let mut status = device.receive_verify_status().await.expect("sub");
    device.verify_start("any").await.expect("verify start");

    // The named-finger signal must already be deliverable when the terminal status arrives. Reading
    // it first with no timeout would pass whatever the order; the assertion is that it is *there*.
    let finger = timeout(Duration::from_secs(5), matched.next())
        .await
        .expect("VerifyFingerMatched must be emitted for a match")
        .expect("stream ended");
    assert_eq!(
        finger.args().expect("args").finger_name(),
        "right-index-finger",
        "the matched signal names the finger that matched"
    );

    let terminal = timeout(Duration::from_secs(5), async {
        while let Some(signal) = status.next().await {
            let args = signal.args().expect("args");
            if *args.done() {
                return args.result().to_string();
            }
        }
        panic!("verify stream ended before a terminal status");
    })
    .await
    .expect("a terminal VerifyStatus");
    assert_eq!(
        terminal, "verify-match",
        "and the status word agrees with the signal"
    );

    device.verify_stop().await.expect("verify stop");
}
