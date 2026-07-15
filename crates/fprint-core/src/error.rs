// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Error type for the core API.

/// Crate result alias.
pub type Result<T> = core::result::Result<T, Error>;

/// Errors surfaced by [`Backend`]/[`Device`] operations.
///
/// Roughly mirrors libfprint's `FpDeviceError` (`FP_DEVICE_ERROR_*`) plus a few
/// transport/proto buckets. `retry`-class variants correspond to fprintd's
/// `*-retry-scan` / `*-remove-and-retry` status strings and mean "try again", not "give up".
///
/// [`Backend`]: crate::Backend
/// [`Device`]: crate::Device
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// No such device, or it went away.
    NotFound,
    /// Operation not supported by this device (missing [`crate::DeviceFeature`]).
    NotSupported,
    /// Device is busy with another operation.
    Busy,
    /// The device was in the wrong state for the request (e.g. not open).
    ProtoState,
    /// Operation was cancelled by the caller.
    Cancelled,
    /// The user should present the finger again (bad scan, too fast, off-centre, …).
    RetryScan(RetryReason),
    /// On-device template storage is full.
    DataFull,
    /// A print with this finger is already enrolled (`DUPLICATES_CHECK` devices).
    DataDuplicate,
    /// The template does not exist on the device / in the gallery.
    DataNotFound,
    /// Low-level transport failure (USB/SPI).
    Transport(String),
    /// Protocol/parse failure talking to the sensor.
    Protocol(String),
    /// Anything a backend cannot map to the above.
    Other(String),
}

/// Why a scan needs to be retried (maps to fprintd `enroll-*`/`verify-*` retry strings).
///
/// Mirrors libfprint's open-ended `FpDeviceRetry` vocabulary, so it is `#[non_exhaustive]`:
/// a future retry reason must not be a breaking change for downstream matchers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RetryReason {
    General,
    TooShort,
    NotCentered,
    RemoveAndRetry,
    TooFast,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::NotFound => f.write_str("device not found"),
            Error::NotSupported => f.write_str("operation not supported by device"),
            Error::Busy => f.write_str("device busy"),
            Error::ProtoState => f.write_str("device in wrong state for operation"),
            Error::Cancelled => f.write_str("operation cancelled"),
            Error::RetryScan(r) => write!(f, "retry scan: {r:?}"),
            Error::DataFull => f.write_str("on-device storage full"),
            Error::DataDuplicate => f.write_str("finger already enrolled"),
            Error::DataNotFound => f.write_str("template not found"),
            Error::Transport(m) => write!(f, "transport error: {m}"),
            Error::Protocol(m) => write!(f, "protocol error: {m}"),
            Error::Other(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for Error {}
