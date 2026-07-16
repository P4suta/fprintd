// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The block-map window band: **an image with either dimension under 25 pixels is consistent but
//! cannot carry the block-map window, so [`detect_minutiae`] answers with the empty list it promises
//! for a size error — never a panic.**
//!
//! `tests/bounds.rs` states the general law (a consistent-but-degenerate image returns an empty list)
//! but exercises it only side of this band: 1×1, one-pixel strips and 512×512. This file pins the
//! band itself, because it is a distinct size precondition from "smaller than one block".
//!
//! ## The geometry
//!
//! `src/maps.rs` bounds each block's low-contrast window origin between
//!
//! ```text
//! xminlimit = pad                              // 13 for the V2 geometry
//! xmaxlimit = pw - pad - windowsize - 1        // pw == iw + 26, windowsize == 24  ->  iw - 12
//! ```
//!
//! and the same for `y`. A `windowsize × windowsize` (24×24) window has somewhere to sit clear of the
//! padding only when `xmaxlimit >= xminlimit`, i.e. `iw >= 25` (and `ih >= 25`). Below that the limits
//! invert and no origin exists between them, so `gen_initial_maps` surfaces the size error the
//! front-end already answers with an empty minutiae list — the same way `block_offsets` rejects an
//! image smaller than a single block.
//!
//! ## The reachable set
//!
//! Swept over every shape in `0..=40` squared: an image is in the band exactly when
//! `iw >= 8 && ih >= 8 && (iw < 25 || ih < 25)` — from 8 (where `block_offsets` takes over) up to 25
//! in one dimension. Below 8 `block_offsets` rejects it first; from 25×25 up the window fits and the
//! detector runs normally. **A realistic fingerprint is far outside the band**, which is why the
//! golden corpus never exercises this precondition — and why it lives here rather than there.

use fprint_mindtct::{detect_minutiae, GrayImage};
use fprint_testkit::{gen, Lcg};

/// A flat mid-gray image of exactly `width * height` bytes — inside [`GrayImage`]'s precondition.
///
/// A flat block is low contrast, so it reaches the histogram path rather than the DFT sums.
fn detect_flat(width: usize, height: usize) -> usize {
    let data = vec![0x80u8; width * height];
    let img = GrayImage::new(&data, width, height, 500).expect("buffer holds the image");
    detect_minutiae(img).len()
}

/// A noise image of exactly `width * height` bytes, from the seed a failure can be replayed with.
fn detect_noise(width: usize, height: usize, seed: u64) -> usize {
    let data = gen::gray_image(&mut Lcg::new(seed), width, height);
    let img = GrayImage::new(&data, width, height, 500).expect("buffer holds the image");
    detect_minutiae(img).len()
}

/// The shape reduced to its geometry: flat, so the pixels are not part of it. A flat block would land
/// in the low-contrast histogram, but the size guard answers before any block is analyzed.
#[test]
fn a_flat_eight_by_eight_image_returns_the_empty_list() {
    assert_eq!(detect_flat(8, 8), 0);
}

/// Width does not lift a short image out of the band: `ih == 8 < 25`, so the window has nowhere to sit
/// vertically and the image is rejected the same way.
#[test]
fn a_flat_full_width_image_eight_rows_tall_returns_the_empty_list() {
    assert_eq!(detect_flat(512, 8), 0);
}

/// Noise rather than a flat fill would reach the DFT sums (a flat block is low contrast and stops one
/// stage earlier), but the size guard precedes both paths, so the answer is the same empty list.
#[test]
fn a_noise_image_sixteen_wide_returns_the_empty_list() {
    assert_eq!(detect_noise(16, 16, 1), 0);
}

/// The lower edge: below 8, `block_offsets` rejects the image before the window guard runs, and the
/// answer is the empty list `src/lib.rs` promises.
#[test]
fn an_image_under_eight_pixels_is_rejected_before_the_window_guard() {
    assert_eq!(detect_flat(24, 7), 0);
    assert_eq!(detect_flat(7, 24), 0);
    assert_eq!(detect_noise(24, 7, 1), 0);
}

/// The upper edge: from 25 in both dimensions the window fits, so the detector runs to completion.
///
/// Termination is the claim, not a count — an image this size may legitimately detect a minutia or
/// none, and pinning either would say something this file does not know.
#[test]
fn an_image_at_least_twenty_five_in_both_dimensions_runs_the_detector() {
    for seed in 1..40 {
        let _ = detect_noise(25, 25, seed);
    }
    let _ = detect_noise(32, 32, 1);
    let _ = detect_noise(64, 64, 1);
}

/// A fingerprint is nowhere near the band; the size guard is a boundary condition, not a change to the
/// detector — `tests/golden.rs` is unaffected.
#[test]
fn a_realistic_image_runs_the_detector() {
    let _ = detect_noise(320, 480, 1);
}
