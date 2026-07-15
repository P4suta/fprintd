// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The daemon's D-Bus error type.
//!
//! [`DaemonError`] maps 1:1 onto the `net.reactivated.Fprint.Error.*` names that fprintd
//! clients (pam_fprintd, GNOME/KDE settings) already recognise — these names are an
//! interoperability fact, taken verbatim from `net.reactivated.Fprint.Device.xml`. The
//! `#[zbus(prefix)]` attribute turns each variant into the matching wire name (e.g.
//! [`DaemonError::AlreadyInUse`] → `net.reactivated.Fprint.Error.AlreadyInUse`).

/// Errors returned to `net.reactivated.Fprint` callers, and the daemon's internal error
/// currency. Each non-`ZBus` variant carries a human-readable message that rides along in
/// the D-Bus error reply.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "net.reactivated.Fprint.Error")]
pub enum DaemonError {
    /// Transport-level failures from zbus itself (the derive's mandatory fallback variant,
    /// which also provides `From<zbus::Error>`).
    #[zbus(error)]
    ZBus(zbus::Error),

    /// No such fingerprint reader (`Manager.GetDefaultDevice` with no devices).
    NoSuchDevice(String),
    /// The device was not claimed before an operation that requires a claim.
    ClaimDevice(String),
    /// The device is already claimed, or an operation is already in flight.
    AlreadyInUse(String),
    /// An internal failure (open failed, serialization failed, …).
    Internal(String),
    /// The caller lacks the required PolicyKit authorization.
    PermissionDenied(String),
    /// The user has no prints enrolled for the requested finger.
    NoEnrolledPrints(String),
    /// `VerifyStop`/`EnrollStop` with nothing in progress.
    NoActionInProgress(String),
    /// The finger name was empty, `"any"`, or otherwise not a real finger.
    InvalidFingername(String),
    /// A stored print could not be removed.
    PrintsNotDeleted(String),
}

impl From<fprint_core::Error> for DaemonError {
    /// Lift a core operation error into the closest D-Bus error. Retry-class errors are
    /// handled by the status-signal machinery ([`crate::status`]) and should never reach
    /// here as a method reply, so they fall through to [`DaemonError::Internal`].
    fn from(e: fprint_core::Error) -> Self {
        use fprint_core::Error as E;
        match e {
            E::NotFound => DaemonError::NoSuchDevice("device not found".into()),
            E::Busy => DaemonError::AlreadyInUse("device busy".into()),
            E::ProtoState => DaemonError::ClaimDevice("device not ready for use".into()),
            E::DataNotFound => DaemonError::NoEnrolledPrints("no such print".into()),
            other => DaemonError::Internal(other.to_string()),
        }
    }
}
