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
    EnrollScript, FingerId, Scenario, VirtualBackend, VirtualDevice, VirtualDeviceBuilder,
};
use fprint_core::{
    Backend as CoreBackend, Device as CoreDevice, DeviceId, DeviceInfo, EnrollProgress,
    FingerStatus, IdentifyOutcome, Print, Result as CoreResult, Temperature, VerifyOutcome,
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

/// A backend whose enrollment remains pending until the daemon cancels it.
///
/// The ordinary virtual backend deliberately completes quickly, so observing `EnrollStart` return
/// does not prove that its pump is still active when the next D-Bus call arrives.
/// This wrapper keeps only enrollment pending and delegates every other device operation.
struct PendingEnrollBackend(VirtualBackend);

struct PendingEnrollDevice(VirtualDevice);

fn pending_enroll_backend() -> PendingEnrollBackend {
    PendingEnrollBackend(backend())
}

impl CoreBackend for PendingEnrollBackend {
    type Device = PendingEnrollDevice;

    async fn enumerate(&self) -> CoreResult<Vec<Self::Device>> {
        Ok(self
            .0
            .enumerate()
            .await?
            .into_iter()
            .map(PendingEnrollDevice)
            .collect())
    }

    async fn open(&self, id: &DeviceId) -> CoreResult<Self::Device> {
        self.0.open(id).await.map(PendingEnrollDevice)
    }
}

impl CoreDevice for PendingEnrollDevice {
    fn info(&self) -> &DeviceInfo {
        self.0.info()
    }

    fn temperature(&self) -> Option<Temperature> {
        self.0.temperature()
    }

    async fn open(&mut self) -> CoreResult<()> {
        self.0.open().await
    }

    async fn close(&mut self) -> CoreResult<()> {
        self.0.close().await
    }

    async fn enroll<F: FnMut(EnrollProgress)>(
        &mut self,
        _print: Print,
        _on_progress: F,
    ) -> CoreResult<Print> {
        std::future::pending().await
    }

    async fn verify_with_status<F: FnMut(FingerStatus)>(
        &mut self,
        enrolled: &Print,
        on_status: F,
    ) -> CoreResult<VerifyOutcome> {
        self.0.verify_with_status(enrolled, on_status).await
    }

    async fn identify_with_status<F: FnMut(FingerStatus)>(
        &mut self,
        gallery: &[Print],
        on_status: F,
    ) -> CoreResult<IdentifyOutcome> {
        self.0.identify_with_status(gallery, on_status).await
    }

    async fn list_prints(&mut self) -> CoreResult<Vec<Print>> {
        self.0.list_prints().await
    }

    async fn delete_print(&mut self, print: &Print) -> CoreResult<()> {
        self.0.delete_print(print).await
    }

    async fn clear_storage(&mut self) -> CoreResult<()> {
        self.0.clear_storage().await
    }

    async fn suspend(&mut self) -> CoreResult<()> {
        self.0.suspend().await
    }

    async fn resume(&mut self) -> CoreResult<()> {
        self.0.resume().await
    }
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
    let harness = Harness::serve("IsolationBusy", ActionSet::ALL, pending_enroll_backend).await;

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
