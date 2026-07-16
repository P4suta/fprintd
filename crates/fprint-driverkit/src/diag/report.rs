// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [`QualityReport`]: image statistics a driver author reads to explain a weak capture.
//!
//! The report is computed tooling-side from the raw frame pixels plus the detected minutiae. It
//! never asks MINDTCT for more than the minutiae it already returns — the detector stays the
//! authority on `x`, `y`, `theta`, `quality`, and this module only summarizes what surrounds them.
//! [`hints`] turns the summary into concrete bring-up hypotheses.

use serde::{Deserialize, Serialize};

use fprint_backend_native::Frame;
use fprint_mindtct::Minutia;

/// Side of the square block the foreground estimate segments the frame into, in pixels.
const FOREGROUND_BLOCK: usize = 16;

/// A block whose gray-level span (`max - min`) reaches this counts as ridge-bearing foreground.
const FOREGROUND_CONTRAST: u8 = 24;

/// Below this many minutiae, [`hints`] suspects a geometry or resolution problem.
const FEW_MINUTIAE: usize = 8;

/// Below this gray-level span, [`hints`] suspects a contrast or exposure problem.
const LOW_DYNAMIC_RANGE: u8 = 40;

/// Below this foreground fraction, [`hints`] suspects the finger covers too little of the frame.
const LOW_FOREGROUND: f64 = 0.25;

/// Below this mean reliability, [`hints`] suspects noisy, weakly detected minutiae.
const LOW_RELIABILITY: f64 = 25.0;

/// A mean gray level this dark or (mirrored) this bright reads as a saturated exposure.
const DARK_MEAN: f64 = 32.0;

/// A summary of a captured frame's fitness for minutiae detection.
///
/// All fields are derived from the raw pixels and the minutiae MINDTCT already returned.
/// `foreground_fraction` is a **diagnostic estimate** from a simple block-contrast segmentation, not
/// the NBIS quality/segmentation map — it is a bring-up signpost, deliberately cheap, and is not
/// bit-comparable to any NBIS output.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QualityReport {
    /// Frame width in pixels.
    pub width: usize,
    /// Frame height in pixels.
    pub height: usize,
    /// How many minutiae the detector returned.
    pub minutiae_count: usize,
    /// Mean of the detected minutiae `quality` (0..=100), or `0` when there are none.
    pub mean_reliability: f64,
    /// Mean pixel gray level (0..=255).
    pub pixel_mean: f64,
    /// Population standard deviation of the pixel gray levels.
    pub pixel_stdev: f64,
    /// Gray-level span `max - min` across the frame (0..=255).
    pub dynamic_range: u8,
    /// Fraction of blocks judged ridge-bearing by the block-contrast estimate (0.0..=1.0).
    pub foreground_fraction: f64,
}

/// Summarize a captured frame and its detected minutiae into a [`QualityReport`].
///
/// The pixels come from `frame` and the minutiae from a prior [`crate::diag::detect`]; the two are
/// summarized independently, so a caller may pass the minutiae it already has. A zero-area frame
/// yields an all-zero report rather than dividing by an empty pixel set.
#[must_use]
pub fn quality_report(frame: &Frame, minutiae: &[Minutia]) -> QualityReport {
    let (width, height) = (frame.width, frame.height);
    let n = width * height;

    let mean_reliability = if minutiae.is_empty() {
        0.0
    } else {
        let sum: f64 = minutiae.iter().map(|m| f64::from(m.quality)).sum();
        sum / minutiae.len() as f64
    };

    if n == 0 {
        return QualityReport {
            width,
            height,
            minutiae_count: minutiae.len(),
            mean_reliability,
            pixel_mean: 0.0,
            pixel_stdev: 0.0,
            dynamic_range: 0,
            foreground_fraction: 0.0,
        };
    }

    let px = &frame.data[..n];
    let mut min = u8::MAX;
    let mut max = u8::MIN;
    let mut sum = 0.0;
    for &p in px {
        sum += f64::from(p);
        min = min.min(p);
        max = max.max(p);
    }
    let pixel_mean = sum / n as f64;
    let mut sq = 0.0;
    for &p in px {
        let d = f64::from(p) - pixel_mean;
        sq += d * d;
    }
    let pixel_stdev = (sq / n as f64).sqrt();

    QualityReport {
        width,
        height,
        minutiae_count: minutiae.len(),
        mean_reliability,
        pixel_mean,
        pixel_stdev,
        dynamic_range: max - min,
        foreground_fraction: foreground_fraction(px, width, height),
    }
}

/// The fraction of [`FOREGROUND_BLOCK`]-sized blocks whose gray-level span reaches
/// [`FOREGROUND_CONTRAST`] — a cheap stand-in for ridge coverage. Flat blocks (background, a
/// saturated region, or an off-image geometry) span little and read as non-foreground.
fn foreground_fraction(px: &[u8], width: usize, height: usize) -> f64 {
    let mut total = 0usize;
    let mut foreground = 0usize;
    for by in (0..height).step_by(FOREGROUND_BLOCK) {
        for bx in (0..width).step_by(FOREGROUND_BLOCK) {
            let y_end = (by + FOREGROUND_BLOCK).min(height);
            let x_end = (bx + FOREGROUND_BLOCK).min(width);
            let mut lo = u8::MAX;
            let mut hi = u8::MIN;
            for y in by..y_end {
                let row = &px[y * width..];
                for &p in &row[bx..x_end] {
                    lo = lo.min(p);
                    hi = hi.max(p);
                }
            }
            total += 1;
            if hi - lo >= FOREGROUND_CONTRAST {
                foreground += 1;
            }
        }
    }
    if total == 0 {
        0.0
    } else {
        foreground as f64 / total as f64
    }
}

/// Map a [`QualityReport`]'s symptoms to concrete bring-up hypotheses, most-diagnostic first.
///
/// Each returned line names a symptom and the knob to check. A clean report returns a single
/// "no obvious defect" line, so the caller always has something to print.
#[must_use]
pub fn hints(report: &QualityReport) -> Vec<String> {
    let mut out = Vec::new();

    if report.minutiae_count < FEW_MINUTIAE {
        out.push(
            "few minutiae: check ppi (MINDTCT thresholds are resolution-relative) and the frame \
             geometry — a transposed or wrong-width buffer shears ridges into noise"
                .to_string(),
        );
    }
    if report.dynamic_range < LOW_DYNAMIC_RANGE {
        out.push(
            "low dynamic range: raise contrast or exposure — ridges and valleys sit too close in \
             gray level to binarize cleanly"
                .to_string(),
        );
    }
    if report.foreground_fraction < LOW_FOREGROUND {
        out.push(
            "little foreground: the ridge field covers little of the frame — check finger \
             placement, the sensor crop, or a geometry that lands the image off-frame"
                .to_string(),
        );
    }
    if report.minutiae_count > 0 && report.mean_reliability < LOW_RELIABILITY {
        out.push(
            "low mean reliability: the detected minutiae are weak — reduce noise or increase \
             pressure and contrast"
                .to_string(),
        );
    }
    if report.pixel_mean < DARK_MEAN || report.pixel_mean > 255.0 - DARK_MEAN {
        out.push(
            "near-saturated exposure: the mean gray level is at an extreme — adjust gain or \
             exposure so the histogram is centered"
                .to_string(),
        );
    }

    if out.is_empty() {
        out.push("no obvious defect: the frame statistics look healthy for detection".to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(data: Vec<u8>, width: usize, height: usize) -> Frame {
        Frame {
            data,
            width,
            height,
            ppi: 500,
        }
    }

    #[test]
    fn flat_frame_has_zero_range_and_no_foreground() {
        let f = frame(vec![128u8; 64 * 64], 64, 64);
        let r = quality_report(&f, &[]);
        assert_eq!(r.width, 64);
        assert_eq!(r.height, 64);
        assert_eq!(r.dynamic_range, 0);
        assert_eq!(r.pixel_mean, 128.0);
        assert_eq!(r.pixel_stdev, 0.0);
        assert_eq!(r.foreground_fraction, 0.0);
        assert_eq!(r.minutiae_count, 0);
        assert_eq!(r.mean_reliability, 0.0);
    }

    #[test]
    fn high_contrast_block_reads_as_foreground() {
        // A single 16x16 frame that swings the full range: one foreground block, full span.
        let mut data = vec![0u8; 16 * 16];
        for (i, p) in data.iter_mut().enumerate() {
            *p = if i % 2 == 0 { 0 } else { 255 };
        }
        let r = quality_report(&frame(data, 16, 16), &[]);
        assert_eq!(r.dynamic_range, 255);
        assert_eq!(r.foreground_fraction, 1.0);
    }

    #[test]
    fn mean_reliability_averages_quality() {
        let ms = [
            Minutia {
                x: 1,
                y: 1,
                theta: 0,
                quality: 20,
            },
            Minutia {
                x: 2,
                y: 2,
                theta: 0,
                quality: 40,
            },
        ];
        let r = quality_report(&frame(vec![0u8; 4], 2, 2), &ms);
        assert_eq!(r.minutiae_count, 2);
        assert_eq!(r.mean_reliability, 30.0);
    }

    #[test]
    fn zero_area_frame_is_all_zero() {
        let r = quality_report(&frame(Vec::new(), 0, 0), &[]);
        assert_eq!(r.pixel_mean, 0.0);
        assert_eq!(r.dynamic_range, 0);
        assert_eq!(r.foreground_fraction, 0.0);
    }

    #[test]
    fn hints_flag_a_dark_flat_frame() {
        let report = QualityReport {
            width: 64,
            height: 64,
            minutiae_count: 0,
            mean_reliability: 0.0,
            pixel_mean: 8.0,
            pixel_stdev: 2.0,
            dynamic_range: 6,
            foreground_fraction: 0.0,
        };
        let hints = hints(&report);
        assert!(hints.iter().any(|h| h.contains("few minutiae")));
        assert!(hints.iter().any(|h| h.contains("low dynamic range")));
        assert!(hints.iter().any(|h| h.contains("little foreground")));
        assert!(hints.iter().any(|h| h.contains("near-saturated")));
    }

    #[test]
    fn hints_pass_a_healthy_report() {
        let report = QualityReport {
            width: 256,
            height: 256,
            minutiae_count: 40,
            mean_reliability: 55.0,
            pixel_mean: 128.0,
            pixel_stdev: 60.0,
            dynamic_range: 200,
            foreground_fraction: 0.8,
        };
        let hints = hints(&report);
        assert_eq!(hints.len(), 1);
        assert!(hints[0].contains("no obvious defect"));
    }
}
