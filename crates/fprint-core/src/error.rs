// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Error type for the core API.

/// Crate result alias.
pub type Result<T> = core::result::Result<T, Error>;

/// Errors surfaced by [`Backend`]/[`Device`] operations.
///
/// A flat *translation vocabulary*, not a source-chaining wrapper: a closed classification
/// mirroring libfprint's `FpDeviceError` (`FP_DEVICE_ERROR_*`) plus a few transport/proto
/// buckets. Each backend maps its native failures (glib `GError`, USB errors) onto these
/// variants, and the daemon remaps them onto D-Bus names; the terminal form is a human-readable
/// string at that edge. `retry`-class variants correspond to fprintd's `*-retry-scan` /
/// `*-remove-and-retry` status strings and mean "try again", not "give up".
///
/// The boundary variants (`Transport`/`Protocol`/`Other`) carry a `String`, not a `dyn` source:
/// the vocabulary is closed and value-semantic, so it collapses to strings at the D-Bus edge
/// rather than chaining a cause.
///
/// [`Backend`]: crate::Backend
/// [`Device`]: crate::Device
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// `FP_DEVICE_RETRY_GENERAL`: the scan was unusable, with no more specific cause.
    General,
    /// `FP_DEVICE_RETRY_TOO_SHORT`: the swipe covered too little of the sensor.
    TooShort,
    /// `FP_DEVICE_RETRY_CENTER_FINGER`: the finger was off the centre of the sensor.
    NotCentered,
    /// `FP_DEVICE_RETRY_REMOVE_FINGER`: the finger must leave the sensor before retrying.
    RemoveAndRetry,
    /// `FP_DEVICE_RETRY_TOO_FAST`: the swipe was too quick to digitize.
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

#[cfg(test)]
mod tests {
    use super::{Error, RetryReason};

    /// **Every [`Error`] variant renders a message that carries its payload.**
    ///
    /// The `match` is the point: it has no wildcard arm, and `#[non_exhaustive]` does not apply
    /// inside the defining crate, so a new variant makes this test *fail to compile* until someone
    /// states what its message must contain. The compiler does the enumerating.
    ///
    /// Only the shape is asserted — that `Transport(m)` surfaces `m`, not the words around it. The
    /// prose is not an interface and no test should freeze it.
    ///
    /// The honest limit: `samples` is hand-written, so a new variant is compile-forced into the
    /// match but must still be added there to be exercised at runtime.
    #[test]
    fn every_error_variant_renders_its_payload() {
        const MARKER: &str = "cafef00d-marker";
        // Every retry reason, since `RetryScan` renders the reason rather than a fixed string.
        let reasons = [
            RetryReason::General,
            RetryReason::TooShort,
            RetryReason::NotCentered,
            RetryReason::RemoveAndRetry,
            RetryReason::TooFast,
        ];
        let samples = [
            Error::NotFound,
            Error::NotSupported,
            Error::Busy,
            Error::ProtoState,
            Error::Cancelled,
            Error::DataFull,
            Error::DataDuplicate,
            Error::DataNotFound,
            Error::Transport(MARKER.to_string()),
            Error::Protocol(MARKER.to_string()),
            Error::Other(MARKER.to_string()),
        ];

        let samples: Vec<Error> = samples
            .into_iter()
            .chain(reasons.map(Error::RetryScan))
            .collect();

        for error in &samples {
            let rendered = error.to_string();
            assert!(!rendered.trim().is_empty(), "{error:?} renders nothing");

            match error {
                // Payload-free variants: a non-empty message is the whole contract.
                Error::NotFound
                | Error::NotSupported
                | Error::Busy
                | Error::ProtoState
                | Error::Cancelled
                | Error::DataFull
                | Error::DataDuplicate
                | Error::DataNotFound => {}
                // Payload-carrying variants must not swallow the payload.
                Error::RetryScan(reason) => {
                    let shown = format!("{reason:?}");
                    assert!(rendered.contains(&shown), "{rendered:?} drops {shown:?}");
                }
                Error::Transport(message) | Error::Protocol(message) | Error::Other(message) => {
                    assert!(rendered.contains(message), "{rendered:?} drops {message:?}");
                }
            }
        }
    }

    /// `Display` distinguishes the payload-free variants from each other: a copy-paste that gave
    /// two of them the same message would leave a caller unable to tell them apart in a log.
    #[test]
    fn payload_free_variants_render_distinctly() {
        let messages = [
            Error::NotFound.to_string(),
            Error::NotSupported.to_string(),
            Error::Busy.to_string(),
            Error::ProtoState.to_string(),
            Error::Cancelled.to_string(),
            Error::DataFull.to_string(),
            Error::DataDuplicate.to_string(),
            Error::DataNotFound.to_string(),
        ];
        for (i, a) in messages.iter().enumerate() {
            for b in &messages[i + 1..] {
                assert_ne!(a, b, "two variants share the message {a:?}");
            }
        }
    }
}
