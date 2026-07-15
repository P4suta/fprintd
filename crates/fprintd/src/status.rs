// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The `verify-*` / `enroll-*` status-string vocabulary.
//!
//! These strings are the D-Bus contract carried by `Device::VerifyStatus` and
//! `Device::EnrollStatus`; clients switch on them verbatim. The mapping is transcribed from
//! fprintd's `verify_result_to_name` / `enroll_result_to_name` (`src/device.c`) — it is an
//! interoperability fact, kept in this one edge module so the domain layers never traffic in
//! wire strings.
//!
//! Each function returns `(result, done)`, where `done` is the `VerifyStatus`/`EnrollStatus`
//! boolean telling the client whether the operation has finished (retry-class statuses are
//! never `done`).

use fprint_core::{EnrollProgress, Error, RetryReason};

/// Terminal verify result for a completed 1:1 or 1:N match attempt.
pub fn verify_match(matched: bool) -> (&'static str, bool) {
    if matched {
        ("verify-match", true)
    } else {
        ("verify-no-match", true)
    }
}

/// A retry-class verify status (the attempt will be automatically restarted).
pub fn verify_retry(reason: RetryReason) -> (&'static str, bool) {
    let s = match reason {
        RetryReason::TooShort => "verify-swipe-too-short",
        RetryReason::NotCentered => "verify-finger-not-centered",
        RetryReason::RemoveAndRetry => "verify-remove-and-retry",
        RetryReason::TooFast => "verify-too-fast",
        // `General` and any future (`#[non_exhaustive]`) reason map to the generic retry.
        _ => "verify-retry-scan",
    };
    (s, false)
}

/// Terminal verify status for a non-retry error.
pub fn verify_error(error: &Error) -> (&'static str, bool) {
    let s = match error {
        // libfprint maps PROTO / REMOVED / TOO_HOT to "disconnected".
        Error::Transport(_) | Error::Protocol(_) | Error::NotFound => "verify-disconnected",
        // Cancellation and "no such print on device" both read as no-match to the client.
        Error::Cancelled | Error::DataNotFound => "verify-no-match",
        _ => "verify-unknown-error",
    };
    (s, true)
}

/// A retry-class enroll status.
pub fn enroll_retry(reason: RetryReason) -> &'static str {
    match reason {
        RetryReason::TooShort => "enroll-swipe-too-short",
        RetryReason::NotCentered => "enroll-finger-not-centered",
        RetryReason::RemoveAndRetry => "enroll-remove-and-retry",
        RetryReason::TooFast => "enroll-too-fast",
        // `General` and any future (`#[non_exhaustive]`) reason map to the generic retry.
        _ => "enroll-retry-scan",
    }
}

/// Map one intermediate [`EnrollProgress`] to its (non-`done`) status string, or `None` for
/// the final completing capture — whose completion is reported by the enroll *result*
/// instead (mirroring fprintd's `enroll_progress_cb`, which suppresses the last stage).
pub fn enroll_progress(progress: &EnrollProgress) -> Option<&'static str> {
    if let Some(reason) = progress.retry {
        Some(enroll_retry(reason))
    } else if progress.completed_stages < progress.total_stages {
        Some("enroll-stage-passed")
    } else {
        None
    }
}

/// Terminal enroll status for a non-retry error.
pub fn enroll_error(error: &Error) -> &'static str {
    match error {
        Error::DataFull => "enroll-data-full",
        Error::DataDuplicate => "enroll-duplicate",
        Error::Transport(_) | Error::Protocol(_) | Error::NotFound => "enroll-disconnected",
        Error::Cancelled => "enroll-failed",
        _ => "enroll-unknown-error",
    }
}
