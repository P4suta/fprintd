// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Degenerate geometry at the public entry point: **an image MINDTCT cannot process never becomes a
//! [`GrayImage`] — [`GrayImage::new`] rejects it — and a well-formed one that simply carries no ridge
//! structure gives [`detect_minutiae`] an empty list. This file pins both.**
//!
//! The golden suite drives 13 well-formed fingerprints. Everything here is the input a caller sends
//! by accident: a zero dimension, a single pixel, a buffer whose length disagrees with the
//! dimensions, a flat field.
//!
//! ## The split, and why it is not arbitrary
//!
//! A [`GrayImage`] is *a detectable image*: its existence proves MINDTCT can run on it. So the two
//! failure modes fall on opposite sides of construction:
//!
//! - An image too small to carry the block-map window, too large for the detector's `i32` arithmetic,
//!   or backed by a buffer shorter than `width * height` **never becomes a [`GrayImage`]** —
//!   [`GrayImage::new`] returns an [`ImageError`]. The exact size floor is pinned in
//!   `tests/regressions.rs`; here we pin the buffer and over-large cases.
//! - A *well-formed* image (constructable) that carries no ridge structure — a flat field, or noise —
//!   runs the full pipeline and returns an **empty list**. That is the only meaning of an empty
//!   result: "no minutiae", never "image rejected".
//!
//! ## Honest limits
//!
//! This proves termination and the shape of the answer, never a minutia count — a count for a
//! degenerate image is not a property anyone should depend on. There is no large-image timing test:
//! wall-clock on a big input measures the machine, and a benchmark that fails on a loaded CI box
//! teaches nothing.

use fprint_mindtct::{detect_minutiae, GrayImage, ImageError};
use fprint_testkit::{gen, Lcg};

/// Run the detector over `width`×`height` of `fill`, sized exactly. Both dimensions must be at or
/// above the detection floor, or construction rejects the image before the detector runs.
fn detect_flat(fill: u8, width: usize, height: usize) -> usize {
    let data = vec![fill; width * height];
    let img = GrayImage::new(&data, width, height, 500).expect("a detectable image");
    detect_minutiae(img).len()
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
/// short-buffer rejection below. The padding stage copies `height` scanlines of `width` bytes and
/// never looks past them.
///
/// The body is textured noise and the tail is `0xff`, so "ignored" is distinguishable from "read": a
/// detector that consumed the tail would see a different image. The body is asserted to produce
/// minutiae first — comparing two empty lists would pass however the tail were treated.
#[test]
fn overlong_data_ignores_the_trailing_bytes() {
    use fprint_testkit::gen::gray_image;

    const SIZE: usize = 96;
    let body = gray_image(&mut Lcg::new(3), SIZE, SIZE);
    let want = detect_minutiae(GrayImage::new(&body, SIZE, SIZE, 500).expect("a detectable image"));
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

/// `data` shorter than `width * height` is rejected by [`GrayImage::new`]: the precondition is
/// enforced at the boundary, not by an index panic in the padding copy. The dimensions here clear the
/// size floor, so it is the buffer — not the geometry — that decides.
#[test]
fn short_data_is_rejected_by_construction() {
    let data = vec![0u8; 10];
    assert_eq!(
        GrayImage::new(&data, 64, 64, 500).unwrap_err(),
        ImageError::BufferTooShort {
            width: 64,
            height: 64,
            got: 10,
        }
    );
}

/// The limiting case of a short buffer: empty `data` with detectable dimensions. Rejected for the same
/// reason — the dimensions, not the buffer, decide the required length.
#[test]
fn empty_data_with_detectable_dimensions_is_rejected() {
    assert_eq!(
        GrayImage::new(&[], 32, 32, 500).unwrap_err(),
        ImageError::BufferTooShort {
            width: 32,
            height: 32,
            got: 0,
        }
    );
}

/// A dimension past `i32::MAX` is rejected before any allocation: the check reads the stated
/// dimensions, not the buffer, so an empty slice suffices to exercise it.
#[test]
fn oversized_dimension_is_rejected_by_construction() {
    let too_wide = i32::MAX as usize + 1;
    assert_eq!(
        GrayImage::new(&[], too_wide, 32, 500).unwrap_err(),
        ImageError::TooLarge {
            width: too_wide,
            height: 32,
        }
    );
}

/// A well-formed image of pure noise terminates.
///
/// A regression guard on a non-terminating input, not a benchmark: nothing here asserts a
/// duration. In `remove_or_adjust_side_minutiae_v2`, the two relocate arms remove a minutia and
/// do not advance the index, because the next minutia slides into the freed slot (matching the
/// reference: "no need to advance because the next minutia has slid into position"). Pure noise
/// reaches that arm; the 13 fingerprints of the golden corpus do not, and
/// `tests/corpus_adequacy.rs` records that gap, so this input guards the arm the corpus cannot.
///
/// The seed and size are the ones that reach it. `cargo test` has no timeout, so a regression in
/// that arm shows up as a hang rather than a failure.
#[test]
fn noise_terminates() {
    let mut lcg = Lcg::new(3);
    for (width, height) in [(64, 64), (124, 124), (128, 128)] {
        let data = gen::gray_image(&mut lcg, width, height);
        let img = GrayImage::new(&data, width, height, 500).expect("a detectable image");
        let _ = detect_minutiae(img);
    }
}
