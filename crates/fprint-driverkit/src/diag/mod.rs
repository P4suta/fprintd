// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Shared diagnostics for the bring-up subcommands: detect, draw, and score a captured frame.
//!
//! The `match` and `doctor` subcommands both start from the same three steps — run the detector,
//! render what it found, and summarize the frame's fitness — so those live here as pure functions,
//! separate from either command's argument handling and output. Everything is deterministic:
//! [`detect`] over a fixed frame yields fixed minutiae, [`render_overlay`] yields fixed pixels, and
//! [`quality_report`] yields a fixed report, which is what lets a golden pin "what the developer
//! sees" to "what CI checks".
//!
//! [`detect`] deliberately calls [`fprint_mindtct::detect_minutiae`] directly rather than the
//! `fprint_core`-facing seam: the diagnostics need each minutia's `quality`, which the xyt-only
//! domain conversion drops.

mod overlay;
mod report;

pub use overlay::{reliability_color, render_overlay, OverlayOptions};
pub use report::{hints, quality_report, QualityReport};

use fprint_backend_native::Frame;
use fprint_mindtct::Minutia;

/// Detect the minutiae in `frame`, keeping each point's `quality`.
///
/// This is the diagnostics' entry point onto MINDTCT. It returns the detector's own [`Minutia`]
/// (with `quality`), not the xyt-only `fprint_core::Minutia` the matcher consumes, because the
/// overlay tint and the reliability summary both need it.
#[must_use]
pub fn detect(frame: &Frame) -> Vec<Minutia> {
    fprint_mindtct::detect_minutiae(frame.as_gray())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_keeps_quality_on_a_ridge_frame() {
        // A deterministic ridge grating with a dislocation plants detectable minutiae; the point is
        // that `detect` surfaces their quality rather than dropping it at the domain seam.
        let (w, h) = (96usize, 96usize);
        let mut data = vec![0u8; w * h];
        let period = 8.0;
        let kf = 2.0 * std::f64::consts::PI / period;
        for y in 0..h {
            for x in 0..w {
                let phase = kf * x as f64 + if x > w / 2 { 1.5 } else { 0.0 };
                data[y * w + x] = (128.0 + 95.0 * phase.cos()).round().clamp(0.0, 255.0) as u8;
            }
        }
        let frame = Frame {
            data,
            width: w,
            height: h,
            ppi: 500,
        };
        let minutiae = detect(&frame);
        // Whatever the count, the detector's quality field must be carried through unclamped-away.
        for m in &minutiae {
            assert!((0..=100).contains(&m.quality));
        }
    }
}
