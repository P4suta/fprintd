// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The detection size floor: **an image with either dimension under [`MIN_DETECTABLE_DIM`] (25 px)
//! cannot carry the block-map window, so [`GrayImage::new`] rejects it — a value a caller holds is
//! always detectable, and [`detect_minutiae`] never has to answer a size error.**
//!
//! This pins the exact boundary. `tests/bounds.rs` states the broader law (buffer consistency, and
//! that a well-formed image with no ridge structure returns an empty list); this file pins where the
//! floor sits and that it is enforced at construction, not deep in the padding copy.
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
//! invert; below 8, `block_offsets` has no room for a single block. Both floors collapse into one
//! constructor precondition: [`MIN_DETECTABLE_DIM`], the tighter of the two (25). **A realistic
//! fingerprint is far above it**, which is why the golden corpus never exercises this precondition —
//! and why it lives here rather than there.

use fprint_mindtct::{detect_minutiae, GrayImage, ImageError, MIN_DETECTABLE_DIM};
use fprint_testkit::{gen, Lcg};

/// The floor is 25 for the shipping V2 geometry: `max(blocksize, windowsize + 1) = max(8, 25)`.
#[test]
fn the_detection_floor_is_twenty_five_pixels() {
    assert_eq!(MIN_DETECTABLE_DIM, 25);
}

/// Every shape with a dimension below the floor is rejected at construction with the precise error —
/// swept over `0..MIN_DETECTABLE_DIM` against a partner on both sides of the floor. A flat buffer long
/// enough for the shape isolates the *dimension* as the cause: it is never a short-buffer rejection.
#[test]
fn a_dimension_below_the_floor_is_rejected_at_construction() {
    let big = MIN_DETECTABLE_DIM + 7;
    for small in 0..MIN_DETECTABLE_DIM {
        for &(w, h) in &[(small, big), (big, small), (small, small)] {
            let data = vec![0x80u8; w * h];
            assert_eq!(
                GrayImage::new(&data, w, h, 500).unwrap_err(),
                ImageError::TooSmall {
                    width: w,
                    height: h,
                    min: MIN_DETECTABLE_DIM,
                },
                "a {w}x{h} image is under the floor and must be rejected"
            );
        }
    }
}

/// The upper edge: at exactly the floor in both dimensions the window fits, so the image constructs
/// and the detector runs to completion.
///
/// Termination is the claim, not a count — an image this size may legitimately detect a minutia or
/// none, and pinning either would say something this file does not know.
#[test]
fn an_image_at_the_floor_constructs_and_runs_the_detector() {
    let dim = MIN_DETECTABLE_DIM;
    for seed in 1..40 {
        let data = gen::gray_image(&mut Lcg::new(seed), dim, dim);
        let img =
            GrayImage::new(&data, dim, dim, 500).expect("an image at the floor is detectable");
        let _ = detect_minutiae(img);
    }
}

/// A fingerprint is nowhere near the floor; the guard is a boundary condition, not a change to the
/// detector — `tests/golden.rs` is unaffected.
#[test]
fn a_realistic_image_runs_the_detector() {
    let data = gen::gray_image(&mut Lcg::new(1), 320, 480);
    let img = GrayImage::new(&data, 320, 480, 500).expect("a realistic image is detectable");
    let _ = detect_minutiae(img);
}
