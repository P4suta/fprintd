// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! **Every privileged method refuses a caller the authorizer does not grant, and refuses it as
//! `PermissionDenied`.**
//!
//! PolicyKit is what stands between a local caller and someone else's enrolled fingers.
//! SECURITY.md names "PolicyKit action enforcement" as in scope, and until this file every D-Bus
//! test stood the daemon up with an authorizer that granted everything — so the refusing half of
//! the surface had never run.
//!
//! Each scenario is a daemon on the shared private bus under its own well-known name, holding an
//! `Authorizer::Fixed(set)`. That is not a test hook: `Fixed` is the type the `--test-mode` daemon
//! already takes, and a set is what makes it say more than yes.
//!
//! ## What this proves, and what it does not
//!
//! The action a method requires is pinned **differentially** — by the smallest grant that admits
//! it — rather than by observing which check ran. So this proves **what the wire admits**, which
//! is what the threat model asks about and what survives a refactor. It does *not* prove which
//! internal call was made, and it does not exercise `Authorizer::Polkit` at all: that needs a
//! PolicyKit daemon, and the ids it would send are pinned in `src/authorizer.rs` instead.
//!
//! Every caller here is the same uid. These tests are about the *action* gate, not about which
//! user the caller is; `dbus_claim_lifetime.rs` covers the session gate.

#![cfg(target_os = "linux")]

mod common;
use common::{Harness, PrivateBus};

use fprint_backend_native::{
    EnrollScript, FingerId, Scenario, VirtualBackend, VirtualDeviceBuilder,
};
use fprintd::ActionSet;

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

/// The D-Bus error name a call failed with, or `None` if it succeeded.
///
/// The name is the contract — `net.reactivated.Fprint.Error.PermissionDenied` is what a client
/// matches on — so asserting it distinguishes "refused because unauthorized" from "refused because
/// the device was not claimed", which is the distinction every test below turns on.
fn error_name<T>(result: zbus::Result<T>) -> Option<String> {
    match result {
        Ok(_) => None,
        Err(zbus::Error::MethodError(name, _, _)) => Some(name.as_str().to_string()),
        Err(e) => panic!("expected a D-Bus method error, got {e:?}"),
    }
}

const DENIED: &str = "net.reactivated.Fprint.Error.PermissionDenied";
const CLAIM_FIRST: &str = "net.reactivated.Fprint.Error.ClaimDevice";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claim_needs_verify_or_enroll_and_either_alone_is_enough() {
    let _bus = PrivateBus::shared();

    // Neither: refused.
    let none = Harness::serve("AuthzClaimNone", ActionSet::NONE, backend).await;
    let conn = none.client().await;
    assert_eq!(
        error_name(none.device(&conn).await.claim("").await).as_deref(),
        Some(DENIED),
        "Claim with no grant at all must be refused"
    );

    // `Claim` gates on verify OR enroll, so each alone admits it. Both arms, because a mistake
    // that dropped the fallback would still pass a test that only tried `verify`.
    for (scenario, grant) in [
        ("AuthzClaimVerify", ActionSet::VERIFY),
        ("AuthzClaimEnroll", ActionSet::ENROLL),
    ] {
        let harness = Harness::serve(scenario, grant, backend).await;
        let conn = harness.client().await;
        assert_eq!(
            error_name(harness.device(&conn).await.claim("").await),
            None,
            "{scenario}: one of the two actions must admit Claim on its own"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enroll_start_requires_enroll_not_verify() {
    let _bus = PrivateBus::shared();
    let harness = Harness::serve("AuthzEnrollStart", ActionSet::VERIFY, backend).await;
    let conn = harness.client().await;
    let device = harness.device(&conn).await;

    // `verify` admits the Claim, so the refusal below is EnrollStart's own gate and not a
    // side-effect of an unclaimed device.
    device.claim("").await.expect("verify admits Claim");
    assert_eq!(
        error_name(device.enroll_start("right-index-finger").await).as_deref(),
        Some(DENIED),
        "EnrollStart must require the enroll action, which VERIFY does not carry"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn verify_start_requires_verify_not_enroll() {
    let _bus = PrivateBus::shared();
    let harness = Harness::serve("AuthzVerifyStart", ActionSet::ENROLL, backend).await;
    let conn = harness.client().await;
    let device = harness.device(&conn).await;

    device.claim("").await.expect("enroll admits Claim");
    assert_eq!(
        error_name(device.verify_start("any").await).as_deref(),
        Some(DENIED),
        "VerifyStart must require the verify action, which ENROLL does not carry"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleting_prints_requires_enroll() {
    let _bus = PrivateBus::shared();
    let harness = Harness::serve("AuthzDelete", ActionSet::VERIFY, backend).await;
    let conn = harness.client().await;
    let device = harness.device(&conn).await;
    device.claim("").await.expect("verify admits Claim");

    // Destroying an enrolment is an enrolment operation. All three spellings the contract carries,
    // because a gate added to one and forgotten on another is exactly the shape of this mistake.
    assert_eq!(
        error_name(device.delete_enrolled_fingers("").await).as_deref(),
        Some(DENIED),
        "DeleteEnrolledFingers"
    );
    assert_eq!(
        error_name(device.delete_enrolled_fingers2().await).as_deref(),
        Some(DENIED),
        "DeleteEnrolledFingers2"
    );
    assert_eq!(
        error_name(device.delete_enrolled_finger("right-index-finger").await).as_deref(),
        Some(DENIED),
        "DeleteEnrolledFinger"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn listing_enrolled_fingers_requires_verify() {
    let _bus = PrivateBus::shared();
    let harness = Harness::serve("AuthzList", ActionSet::NONE, backend).await;
    let conn = harness.client().await;
    assert_eq!(
        error_name(none_list(&harness, &conn).await).as_deref(),
        Some(DENIED),
        "which fingers a user has enrolled is not public"
    );
}

async fn none_list(harness: &Harness, conn: &zbus::Connection) -> zbus::Result<Vec<String>> {
    harness.device(conn).await.list_enrolled_fingers("").await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acting_for_another_user_requires_setusername() {
    let _bus = PrivateBus::shared();
    let harness = Harness::serve(
        "AuthzSetUsername",
        ActionSet::VERIFY | ActionSet::ENROLL,
        backend,
    )
    .await;
    let conn = harness.client().await;
    let device = harness.device(&conn).await;

    // Both device actions are granted, so nothing but the setusername gate can refuse this. Reading
    // another user's prints is what that action exists to gate.
    assert_eq!(
        error_name(device.claim("someone-else").await).as_deref(),
        Some(DENIED),
        "claiming on behalf of another user must require setusername"
    );

    // The control: the same authorizer, the caller's own user, admitted. So the refusal above is
    // the username and not the grant set.
    assert_eq!(
        error_name(device.claim("").await),
        None,
        "the empty username means the caller's own, and needs no setusername"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unclaimed_unauthorized_caller_is_told_to_claim_first() {
    let _bus = PrivateBus::shared();
    let harness = Harness::serve("AuthzOrder", ActionSet::NONE, backend).await;
    let conn = harness.client().await;
    let device = harness.device(&conn).await;

    // This looks like trivia and is not. `verify_start` checks the claim *before* the authorizer,
    // and that order is load-bearing: `PolkitAuthorizer` passes ALLOW_USER_INTERACTION
    // unconditionally, so an authorizer consulted first would let any caller on the bus raise an
    // authentication dialog by calling a method it never claimed for. Reversing the two lines
    // turns every start method into that. Nothing else holds this in place.
    assert_eq!(
        error_name(device.verify_start("any").await).as_deref(),
        Some(CLAIM_FIRST),
        "an unclaimed caller must be refused for the claim, before any authorization is asked"
    );
    assert_eq!(
        error_name(device.enroll_start("right-index-finger").await).as_deref(),
        Some(CLAIM_FIRST),
        "and the same for EnrollStart"
    );
}
