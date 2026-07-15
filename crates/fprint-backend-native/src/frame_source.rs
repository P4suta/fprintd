// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [`FrameSource`]: the capture seam an [`crate::ImageDevice`] drives.
//!
//! The one hardware-facing abstraction of the host-image pipeline: everything above it
//! (detect → match, in `crate::detector` / `crate::matcher`) is pure and deterministic, and
//! everything a real sensor does — arm the reader, wait for a finger, hand back a frame — lives
//! behind [`FrameSource::capture`]. Implementors that need no bring-up (such as
//! [`crate::SyntheticFrameSource`]) inherit the default no-op [`arm`](FrameSource::arm) /
//! [`disarm`](FrameSource::disarm); a USB transport overrides them.
//!
//! Cancellation follows the project model: `capture` is the only awaiting step, so its poll boundary
//! is where a dropped [`crate::ImageDevice::enroll`] future cancels. A [`Capture::Retry`] is a *weak*
//! capture (the stage does not advance; it drives an [`fprint_core::EnrollProgress::retry`]); an
//! `Err(Error::RetryScan | Transport)` is a *hard* failure.

use crate::frame::Frame;

/// The outcome of one [`FrameSource::capture`] attempt.
///
/// `Frame` is a usable capture; `Retry` is a weak capture that did not advance the operation and
/// carries the reason to forward to the user (the daemon renders the matching status string).
pub enum Capture {
    /// A usable captured frame.
    Frame(Frame),
    /// A weak capture: no frame this time, present the finger again.
    Retry(fprint_core::RetryReason),
}

/// A source of captured grayscale frames — the sensor seam behind [`crate::ImageDevice`].
///
/// `async fn` in a public trait mirrors [`fprint_core::Device`]: static dispatch, so callers never add a
/// `+ Send` bound and the `async_fn_in_trait` lint is intentionally allowed here.
#[allow(async_fn_in_trait)] // Static dispatch (no `+ Send` needed), same rationale as `fprint_core::Device`.
pub trait FrameSource {
    /// Wait for and return the next capture (or a retry). This is the operation's poll boundary.
    async fn capture(&mut self) -> fprint_core::Result<Capture>;

    /// Ready the sensor for capture (default: nothing; a USB transport overrides this).
    async fn arm(&mut self) -> fprint_core::Result<()> {
        Ok(())
    }

    /// Release the sensor after capture (default: nothing; a USB transport overrides this).
    async fn disarm(&mut self) -> fprint_core::Result<()> {
        Ok(())
    }
}
