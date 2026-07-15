// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PolicyKit authorization, as a two-variant enum so tests can bypass it.
//!
//! fprintd gates every privileged method on a PolicyKit action —
//! `net.reactivated.fprint.device.{verify,enroll,setusername}`. The action definitions come
//! from the fprintd package, which we depend on rather than duplicate (ARCHITECTURE.md
//! §Coexistence); these ids must match it exactly. We express the check as the
//! [`Authorizer`] enum:
//! [`Authorizer::Polkit`] performs the real `CheckAuthorization` call against
//! `org.freedesktop.PolicyKit1.Authority` (via [`PolkitAuthorizer`]), while
//! [`Authorizer::AllowAll`] grants everything so the hardware-free D-Bus integration test can
//! run without a PolicyKit daemon. The enum uses native `async fn` and hand-written
//! delegation — the same static-dispatch spirit as `fprint-integration`'s `CompositeDevice` — so
//! the daemon needs neither `async-trait` nor `Arc<dyn Authorizer>`.

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

/// Decides whether a D-Bus caller may perform a [`PolkitAction`].
///
/// [`Authorizer::AllowAll`] grants everything (tests and the `--test-mode` daemon only);
/// [`Authorizer::Polkit`] delegates to a real [`PolkitAuthorizer`].
pub enum Authorizer {
    /// Grants every request unconditionally.
    AllowAll,
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
            Authorizer::AllowAll => Ok(()),
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
