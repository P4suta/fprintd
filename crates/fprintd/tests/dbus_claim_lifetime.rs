// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! **A claim does not outlive the client that took it.**
//!
//! A claim is held by a *connection*, and nothing in the `net.reactivated.Fprint` contract obliges
//! a client to release one — it may simply exit, crash, or be killed. Without a watcher that
//! answer would be permanent: `Claim` records the caller's unique bus name, every later method
//! compares against it, and only `Release` clears it. So one client that claimed and left would
//! make every subsequent caller — `pam_fprintd` included — see `AlreadyInUse` until the daemon
//! restarted. The reader is the login path, so that is a denial of service reachable in two D-Bus
//! calls by anyone the PolicyKit `verify` action admits.
//!
//! `Claim` therefore starts a `NameOwnerChanged` watcher on its own bus name, and a vanish runs the
//! same teardown `Release` does.
//!
//! ## What is *not* asserted here, having been checked
//!
//! **An enrolment that completed before its client vanished stays on disk, and that is correct.**
//! A print belongs to the username the session resolved, not to the connection that asked for it:
//! a user who gave the sensor five impressions and whose client then crashed has enrolled, and
//! throwing that away would lose the work rather than protect anything. Teardown signals a *live*
//! pump to stop; it does not un-save a finished one, and no test here pretends it should.
//!
//! ## Honest limits
//!
//! Dropping a `zbus::Connection` is what stands in for a client dying. That exercises the daemon's
//! side exactly — the bus sends `NameOwnerChanged` for a dropped connection just as it does for a
//! killed process — but it does not prove anything about a client that stays connected while
//! wedged. Nothing here claims otherwise: a live connection holds its claim, by design.
//!
//! Whether a vanish cancels an in-flight enrolment is **not** tested, because against the virtual
//! device it cannot be: its enrol completes in microseconds, so the signal never arrives first.
//! That race is the test's, not the daemon's — a real sensor takes seconds — and a test whose
//! answer depends on which of two events wins is worse than no test.
//!
//! The waits below are bounded by [`RELEASED_WITHIN`] rather than by a signal, because the daemon
//! offers no signal for "the claim is free". A timeout is what a regression looks like, and a
//! generous bound keeps that unambiguous on a loaded machine.

#![cfg(target_os = "linux")]

mod common;
use common::{Harness, PrivateBus};

use std::time::Duration;

use fprint_backend_native::{
    EnrollScript, FingerId, Scenario, VirtualBackend, VirtualDeviceBuilder,
};
use tokio::time::timeout;

/// How long the bus and the daemon get to notice a vanished client. Far above the real cost (a
/// signal round-trip), so a failure here means the claim was never released at all.
const RELEASED_WITHIN: Duration = Duration::from_secs(5);

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

/// Poll `Claim` from a fresh client until it succeeds, or give up after [`RELEASED_WITHIN`].
///
/// Polls rather than waits on a signal because the daemon publishes none for "the claim is free" —
/// that is a fact about its internals, not part of the contract. Returns the connection with the
/// proxy: dropping the connection would release the claim again, so the caller must hold it.
async fn claim_when_free(
    harness: &Harness,
) -> Option<(zbus::Connection, common::DeviceProxy<'static>)> {
    let conn = harness.client().await;
    let device = harness.device(&conn).await;
    let claimed = timeout(RELEASED_WITHIN, async {
        while device.claim("").await.is_err() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok();
    claimed.then_some((conn, device))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_vanished_client_releases_its_claim() {
    let _bus = PrivateBus::shared();
    let harness = Harness::serve("ClaimLifetime", fprintd::ActionSet::ALL, backend).await;

    // A client claims, and leaves without releasing.
    {
        let conn = harness.client().await;
        let device = harness.device(&conn).await;
        device.claim("").await.expect("first claim");

        // The claim is genuinely held: a second client is refused while the first is alive. Without
        // this, a passing test could mean the claim never took.
        let other = harness.client().await;
        let other_device = harness.device(&other).await;
        assert!(
            other_device.claim("").await.is_err(),
            "a live claim must exclude a second client, or this test proves nothing"
        );
    } // both connections drop here — the bus reports the first name vanishing

    assert!(
        claim_when_free(&harness).await.is_some(),
        "the vanished client's claim was never released"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_vanished_client_leaves_the_device_usable_not_merely_free() {
    let _bus = PrivateBus::shared();
    let harness = Harness::serve("EnrollLifetime", fprintd::ActionSet::ALL, backend).await;

    // Leave with an operation started, so teardown has both a claim and a pump to clear.
    {
        let conn = harness.client().await;
        let device = harness.device(&conn).await;
        device.claim("").await.expect("claim");
        device
            .enroll_start("right-index-finger")
            .await
            .expect("enroll start");
    }

    let (_conn, device) = claim_when_free(&harness)
        .await
        .expect("the vanished client's claim was never released");

    // Freeing the claim is half the job. `Claim` and the active-operation guard are separate
    // pieces of state, so a teardown that cleared only the first would leave the next client
    // holding a device it cannot use: claimed, then `AlreadyInUse` from a pump nobody owns.
    //
    // `EnrollStart` rather than `VerifyStart`, because enrolment needs no existing print — this
    // must fail for the reason under test, if it fails at all.
    device
        .enroll_start("left-thumb")
        .await
        .expect("the device must be idle, not just unclaimed");
    device.enroll_stop().await.expect("enroll stop");
}
