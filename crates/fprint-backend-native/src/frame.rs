// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [`Frame`]: an owned captured grayscale frame, bridged to the detector's borrowed image.
//!
//! A [`crate::FrameSource`] yields owned `Frame`s (it captured the bytes, so it owns them), but
//! [`fprint_mindtct`] takes a **borrowing** [`fprint_mindtct::GrayImage`] — it never copies the pixels it
//! detects over. [`Frame::as_gray`] is the seam between the two: it hands the detector a view into
//! the owned buffer for the duration of one detection, so a borrow-based `GrayImage` and this
//! crate's `#![forbid(unsafe_code)]` coexist with no copy and no lifetime gymnastics.

/// An owned 8-bit grayscale capture: exactly `width * height` row-major bytes plus the scan
/// resolution the detector's resolution-relative thresholds need.
#[derive(Clone, Debug)]
pub struct Frame {
    pub data: Vec<u8>,
    pub width: usize,
    pub height: usize,
    /// Scan resolution in pixels-per-inch.
    pub ppi: u16,
}

impl Frame {
    /// Borrow this owned frame as an [`fprint_mindtct::GrayImage`] for one detection pass.
    ///
    /// The returned view borrows `self`, so it cannot outlive the frame — the detector reads the
    /// pixels in place and returns owned minutiae, and the buffer is never copied.
    #[must_use]
    pub fn as_gray(&self) -> fprint_mindtct::GrayImage<'_> {
        fprint_mindtct::GrayImage {
            data: &self.data,
            width: self.width,
            height: self.height,
            ppi: self.ppi,
        }
    }
}
