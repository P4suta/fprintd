// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Degenerate geometry at the public entry point: **a consistent but ill-shaped image gives
//! [`detect_minutiae`] an empty list, and an inconsistent one cannot reach it — [`GrayImage::new`]
//! rejects the short buffer first. This file pins both.**
//!
//! The golden suite drives 13 well-formed fingerprints. Everything here is the input a caller sends
//! by accident: a zero dimension, a single pixel, a one-pixel-wide strip, a flat field, a buffer
//! whose length disagrees with the dimensions. `src/lib.rs` promises "an empty list on the (size)
//! error paths" and until this file nothing checked it.
//!
//! ## The two answers, and why the split is not arbitrary
//!
//! A *degenerate but consistent* image (`data.len() >= width * height`) builds and returns an empty
//! list. No block map survives an image smaller than a block, so the pipeline reaches its size error
//! path and says "no minutiae" — the truthful answer.
//!
//! An *inconsistent* one (`data.len() < width * height`) never becomes a [`GrayImage`]:
//! [`GrayImage::new`] returns [`ImageError::TooSmall`], so a short buffer is rejected at the boundary
//! rather than surfacing as an index panic deep in the padding copy. An over-long buffer is the
//! asymmetric case — it is accepted, its tail ignored.
//!
//! ## Honest limits
//!
//! This proves termination and the shape of the answer, never a minutia count — a count for a
//! degenerate image is not a property anyone should depend on. There is no large-image test here:
//! wall-clock on a big input measures the machine, and a benchmark that fails on a loaded CI box
//! teaches nothing.

use fprint_mindtct::{detect_minutiae, GrayImage, ImageError};
use fprint_testkit::{gen, Lcg};

/// Run the detector over `width`×`height` of `fill`, sized exactly.
fn detect_flat(fill: u8, width: usize, height: usize) -> usize {
    let data = vec![fill; width * height];
    let img = GrayImage::new(&data, width, height, 500).expect("buffer holds the image");
    detect_minutiae(img).len()
}

#[test]
fn zero_sized_image_yields_no_minutiae() {
    let img = GrayImage::new(&[], 0, 0, 500).expect("an empty buffer holds a 0x0 image");
    assert_eq!(detect_minutiae(img).len(), 0);
}

#[test]
fn single_pixel_image_yields_no_minutiae() {
    assert_eq!(detect_flat(0, 1, 1), 0);
}

/// A one-pixel-wide strip: an extreme aspect ratio, and far taller than any corpus image. The block
/// grid cannot form across a single column, so the size error path answers.
#[test]
fn single_column_strip_yields_no_minutiae() {
    assert_eq!(detect_flat(0, 1, 10_000), 0);
}

/// A flat field carries no ridge structure at either extreme of the intensity range. Both are
/// well-formed images of a realistic size — the pipeline runs to completion and finds nothing.
#[test]
fn uniform_black_image_yields_no_minutiae() {
    assert_eq!(detect_flat(0x00, 512, 512), 0);
}

#[test]
fn uniform_white_image_yields_no_minutiae() {
    assert_eq!(detect_flat(0xff, 512, 512), 0);
}

/// A buffer *longer* than `width * height` is accepted and its tail ignored — the counterpart to the
/// short-buffer panic below. The padding stage copies `height` scanlines of `width` bytes and never
/// looks past them.
///
/// The body is textured noise and the tail is `0xff`, so "ignored" is distinguishable from "read": a
/// detector that consumed the tail would see a different image. The body is asserted to produce
/// minutiae first — comparing two empty lists would pass however the tail were treated.
#[test]
fn overlong_data_ignores_the_trailing_bytes() {
    use fprint_testkit::{gen::gray_image, Lcg};

    const SIZE: usize = 96;
    let body = gray_image(&mut Lcg::new(3), SIZE, SIZE);
    let want =
        detect_minutiae(GrayImage::new(&body, SIZE, SIZE, 500).expect("buffer holds the image"));
    assert!(
        !want.is_empty(),
        "the body must produce minutiae or the comparison below has no teeth"
    );

    let mut padded = body.clone();
    padded.extend(std::iter::repeat_n(0xff, SIZE * SIZE));
    let got = detect_minutiae(
        GrayImage::new(&padded, SIZE, SIZE, 500).expect("over-long buffer is accepted"),
    );
    assert_eq!(got, want, "the trailing bytes changed the result");
}

/// `data` shorter than `width * height` is rejected by [`GrayImage::new`] before the detector runs:
/// the precondition is enforced at the boundary, not by an index panic in the padding copy.
#[test]
fn short_data_is_rejected_by_construction() {
    let data = vec![0u8; 10];
    assert_eq!(
        GrayImage::new(&data, 64, 64, 500).unwrap_err(),
        ImageError::TooSmall {
            width: 64,
            height: 64,
            got: 10,
        }
    );
}

/// The limiting case of a short buffer: empty `data` with non-zero dimensions. Rejected for the same
/// reason — the dimensions, not the buffer, decide the required length.
#[test]
fn empty_data_with_nonzero_dimensions_is_rejected() {
    assert_eq!(
        GrayImage::new(&[], 8, 8, 500).unwrap_err(),
        ImageError::TooSmall {
            width: 8,
            height: 8,
            got: 0,
        }
    );
}

/// A well-formed image of pure noise terminates.
///
/// Not a benchmark: nothing here asserts a duration. It is a **regression guard on a non-terminating
/// input**. `remove_or_adjust_side_minutiae_v2`'s two relocate arms used to leave a removed minutia
/// in the list without advancing the index, so the loop re-processed it forever; the reference
/// removes it there and says so ("no need to advance because the next minutia has slid into
/// position"). Noise reaches that arm where the 13 fingerprints of the golden corpus never do —
/// `tests/corpus_adequacy.rs` records that blindness — so this input, and not the corpus, is what
/// holds the fix in place.
///
/// The seed and size are the ones that reproduced it. `cargo test` has no timeout, so a regression
/// shows up as a hang rather than a failure; that is still infinitely better than silence.
#[test]
fn noise_terminates() {
    let mut lcg = Lcg::new(3);
    for (width, height) in [(64, 64), (124, 124), (128, 128)] {
        let data = gen::gray_image(&mut lcg, width, height);
        let img = GrayImage::new(&data, width, height, 500).expect("buffer holds the image");
        let _ = detect_minutiae(img);
    }
}
