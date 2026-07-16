// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PolicyKit authorization: a fixed answer, or ask.
//!
//! fprintd gates every privileged method on a PolicyKit action —
//! `net.reactivated.fprint.device.{verify,enroll,setusername}`. The action definitions come
//! from the fprintd package, which we depend on rather than duplicate (ARCHITECTURE.md
//! §Coexistence); these ids must match it exactly. We express the check as the
//! [`Authorizer`] enum: [`Authorizer::Polkit`] performs the real `CheckAuthorization` call
//! against `org.freedesktop.PolicyKit1.Authority` (via [`PolkitAuthorizer`]), and
//! [`Authorizer::Fixed`] answers from a set decided in advance. The enum uses native `async fn`
//! and hand-written delegation, so the daemon needs neither `async-trait` nor
//! `Arc<dyn Authorizer>`.
//!
//! [`Authorizer::Fixed`] is not a test hook. `Fixed(ActionSet::ALL)` is the bring-up authorizer
//! the `--test-mode` daemon takes; `Fixed(ActionSet::NONE)` is its dual, and both are honest
//! answers a production enum can hold. That the D-Bus tests can also ask for
//! `Fixed(ActionSet::VERIFY)` and watch enrolment be refused is a consequence of the type being
//! expressive, not the reason it exists — which is why there is no `#[cfg(test)]` here, and no
//! cargo feature to forget to turn off.

use std::collections::HashMap;

use zbus::zvariant::OwnedValue;

use crate::error::DaemonError;

/// A PolicyKit action a method may require.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolkitAction {
    /// `net.reactivated.fprint.device.verify`
    Verify,
    /// `net.reactivated.fprint.device.enroll`
    Enroll,
    /// `net.reactivated.fprint.device.setusername`
    SetUsername,
}

impl PolkitAction {
    /// The PolicyKit action id string.
    pub fn id(self) -> &'static str {
        match self {
            PolkitAction::Verify => "net.reactivated.fprint.device.verify",
            PolkitAction::Enroll => "net.reactivated.fprint.device.enroll",
            PolkitAction::SetUsername => "net.reactivated.fprint.device.setusername",
        }
    }
}

/// A set of [`PolkitAction`]s.
///
/// Hand-rolled bitflags, mirroring `fprint_core::DeviceFeature`; no bitflags crate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActionSet(u8);

impl ActionSet {
    /// Grants nothing.
    pub const NONE: ActionSet = ActionSet(0);
    /// `net.reactivated.fprint.device.verify`
    pub const VERIFY: ActionSet = ActionSet(1 << 0);
    /// `net.reactivated.fprint.device.enroll`
    pub const ENROLL: ActionSet = ActionSet(1 << 1);
    /// `net.reactivated.fprint.device.setusername`
    pub const SET_USERNAME: ActionSet = ActionSet(1 << 2);
    /// Every action.
    pub const ALL: ActionSet = ActionSet(0b111);

    /// The bit for one action.
    const fn bit(action: PolkitAction) -> ActionSet {
        match action {
            PolkitAction::Verify => ActionSet::VERIFY,
            PolkitAction::Enroll => ActionSet::ENROLL,
            PolkitAction::SetUsername => ActionSet::SET_USERNAME,
        }
    }

    /// Whether this set grants `action`.
    #[must_use]
    pub const fn contains(self, action: PolkitAction) -> bool {
        self.0 & ActionSet::bit(action).0 != 0
    }
}

impl std::ops::BitOr for ActionSet {
    type Output = ActionSet;
    fn bitor(self, rhs: ActionSet) -> ActionSet {
        ActionSet(self.0 | rhs.0)
    }
}

/// Decides whether a D-Bus caller may perform a [`PolkitAction`].
///
/// [`Authorizer::Fixed`] answers from a set decided in advance; [`Authorizer::Polkit`] asks a
/// real [`PolkitAuthorizer`].
pub enum Authorizer {
    /// Grants exactly the actions in the set and refuses the rest.
    Fixed(ActionSet),
    /// Consults PolicyKit on the system bus.
    Polkit(PolkitAuthorizer),
}

impl Authorizer {
    /// `Ok(())` if `subject_bus_name` is authorized for `action`, else
    /// [`DaemonError::PermissionDenied`].
    pub async fn check(
        &self,
        subject_bus_name: &str,
        action: PolkitAction,
    ) -> Result<(), DaemonError> {
        match self {
            Authorizer::Fixed(granted) if granted.contains(action) => Ok(()),
            Authorizer::Fixed(_) => Err(DaemonError::PermissionDenied(format!(
                "Not Authorized: {}",
                action.id()
            ))),
            Authorizer::Polkit(p) => p.check(subject_bus_name, action).await,
        }
    }
}

/// `POLKIT_CHECK_AUTHORIZATION_FLAGS_ALLOW_USER_INTERACTION` — let PolicyKit prompt the user.
const ALLOW_USER_INTERACTION: u32 = 1;

#[zbus::proxy(
    interface = "org.freedesktop.PolicyKit1.Authority",
    default_service = "org.freedesktop.PolicyKit1",
    default_path = "/org/freedesktop/PolicyKit1/Authority"
)]
trait PolicyKitAuthority {
    /// `CheckAuthorization((sa{sv}) subject, s action_id, a{ss} details, u flags,
    /// s cancellation_id) -> (bba{ss})`.
    fn check_authorization(
        &self,
        subject: &(String, HashMap<String, OwnedValue>),
        action_id: &str,
        details: &HashMap<String, String>,
        flags: u32,
        cancellation_id: &str,
    ) -> zbus::Result<(bool, bool, HashMap<String, String>)>;
}

/// The real authorizer: proxies `CheckAuthorization` on the system bus with the caller's
/// bus name as a `system-bus-name` subject.
pub struct PolkitAuthorizer {
    proxy: PolicyKitAuthorityProxy<'static>,
}

impl PolkitAuthorizer {
    /// Connect to PolicyKit on the system bus.
    pub async fn new() -> Result<Self, DaemonError> {
        let conn = zbus::Connection::system().await?;
        let proxy = PolicyKitAuthorityProxy::new(&conn).await?;
        Ok(PolkitAuthorizer { proxy })
    }

    /// `Ok(())` if `subject_bus_name` is authorized for `action`, else
    /// [`DaemonError::PermissionDenied`].
    pub async fn check(
        &self,
        subject_bus_name: &str,
        action: PolkitAction,
    ) -> Result<(), DaemonError> {
        let name = OwnedValue::try_from(zbus::zvariant::Value::from(subject_bus_name))
            .map_err(|e| DaemonError::Internal(format!("polkit subject: {e}")))?;
        let mut subject_details = HashMap::new();
        subject_details.insert("name".to_string(), name);
        let subject = ("system-bus-name".to_string(), subject_details);
        let details = HashMap::new();

        let (authorized, _challenge, _details) = self
            .proxy
            .check_authorization(&subject, action.id(), &details, ALLOW_USER_INTERACTION, "")
            .await?;

        if authorized {
            Ok(())
        } else {
            Err(DaemonError::PermissionDenied(format!(
                "Not Authorized: {}",
                action.id()
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACTIONS: [PolkitAction; 3] = [
        PolkitAction::Verify,
        PolkitAction::Enroll,
        PolkitAction::SetUsername,
    ];

    /// The ids the fprintd package's PolicyKit policy declares. A typo here is not a failed check
    /// but an *unfindable* action, which PolicyKit refuses — so every privileged method would
    /// break at once, on a real system only.
    #[test]
    fn action_ids_match_the_fprintd_policy() {
        assert_eq!(
            PolkitAction::Verify.id(),
            "net.reactivated.fprint.device.verify"
        );
        assert_eq!(
            PolkitAction::Enroll.id(),
            "net.reactivated.fprint.device.enroll"
        );
        assert_eq!(
            PolkitAction::SetUsername.id(),
            "net.reactivated.fprint.device.setusername"
        );
    }

    #[test]
    fn none_grants_nothing_and_all_grants_everything() {
        for action in ACTIONS {
            assert!(!ActionSet::NONE.contains(action), "NONE granted {action:?}");
            assert!(ActionSet::ALL.contains(action), "ALL refused {action:?}");
        }
    }

    /// Each flag is its own bit: a set built from one action grants that one and no other. A
    /// duplicated or overlapping bit would silently widen every grant that names it.
    #[test]
    fn each_action_has_its_own_bit() {
        for granted in ACTIONS {
            let set = ActionSet::bit(granted);
            for action in ACTIONS {
                assert_eq!(
                    set.contains(action),
                    action == granted,
                    "{granted:?} alone: contains({action:?})"
                );
            }
        }
    }

    #[test]
    fn bitor_unions_and_all_is_the_union_of_every_action() {
        let union = ACTIONS
            .into_iter()
            .fold(ActionSet::NONE, |acc, a| acc | ActionSet::bit(a));
        assert_eq!(union, ActionSet::ALL, "ALL must be exactly every action");
        let two = ActionSet::VERIFY | ActionSet::ENROLL;
        assert!(two.contains(PolkitAction::Verify) && two.contains(PolkitAction::Enroll));
        assert!(!two.contains(PolkitAction::SetUsername));
        assert_eq!(ActionSet::NONE | ActionSet::ALL, ActionSet::ALL);
    }

    /// `Default` is the safe answer. A set that defaulted to `ALL` would make a forgotten
    /// initializer grant everything.
    #[test]
    fn the_default_set_grants_nothing() {
        assert_eq!(ActionSet::default(), ActionSet::NONE);
    }
}
