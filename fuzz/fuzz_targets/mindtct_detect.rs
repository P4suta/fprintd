// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! **Every [`Minutia`] `detect_minutiae` returns lands inside the image, with `theta` in range.**
//!
//! `crates/fprint-mindtct/tests/validity.rs` states this contract and sweeps it over the 13 corpus
//! images and seeded noise. This target is the coverage-guided search behind that test, over images
//! a fuzzer chose rather than an `Lcg` did.
//!
//! The bound is asserted at the producer because a violation downstream is **silent**:
//! `fprint_bozorth3`'s `xyt::prepare` validates nothing, so an out-of-image minutia is consumed as
//! a coordinate rather than rejected, and surfaces as a slightly wrong match score. That is worse
//! than a panic — which is why an oracle here is worth more than the panic-only claim.
//!
//! `validity.rs` names the sharp edge this hunts: `src/xyt.rs` computes `y = ih - minutia.y` and
//! nothing clamps it, so a minutia detected in row 0 would flip to `y == height` — one past the
//! last row. It is unreachable only because detection never emits inside the top block margin. If
//! an image exists that reaches row 0, this target is what finds it.
//!
//! ## Why the dimensions are bounded, and why a short buffer is rejected
//!
//! [`GrayImage`] documents `data.len() >= width * height` as an **unenforced precondition** and
//! panics in the padding copy otherwise — `tests/bounds.rs` pins that panic as the current
//! behaviour. It is a documented precondition, so violating it here would re-find a recorded panic
//! instead of a bug: this target rejects `width * height > data.len()` and fuzzes inside the
//! contract.
//!
//! Dimensions are capped at [`MAX_DIM`] so the target stays fast. The reject rule binds first in
//! practice — libFuzzer's default `-max_len` is 4096, so an accepted image is about 64×64 unless
//! the corpus is run with a larger one.
//!
//! An image with a dimension under 25 pixels cannot carry the block-map window, so `detect_minutiae`
//! answers with the empty list its size-error contract promises (`crates/fprint-mindtct/src/maps.rs`
//! rejects it, `crates/fprint-mindtct/tests/regressions.rs` pins the boundary). The validity
//! assertions below hold vacuously over that empty list, so the target fuzzes the whole size domain
//! without a special case.
//!
//! ## Limits
//!
//! Fuzzer bytes are noise, not a fingerprint. Noise exercises the bound cheaply and widely; it
//! cannot show that a *plausible* image drives a minutia to an edge. `ppi` is fixed at 500 — the
//! resolution every fixture and every `validity.rs` case uses — so this target says nothing about
//! the resolution-relative thresholds at another scan density.

#![no_main]

use arbitrary::Unstructured;
use fprint_fuzz::Bytes;
use fprint_mindtct::{detect_minutiae, GrayImage};
use fprint_testkit::ByteSource;
use libfuzzer_sys::fuzz_target;

/// Cap on either dimension, so one input cannot cost a second.
const MAX_DIM: i32 = 512;

/// The scan resolution every fixture and every `validity.rs` case uses.
const PPI: u16 = 500;

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let (width, height) = {
        let mut src = Bytes(&mut u);
        (
            src.in_range(0, MAX_DIM) as usize,
            src.in_range(0, MAX_DIM) as usize,
        )
    };

    // The rest of the input is the image, one byte per pixel — so a mutation the fuzzer makes is a
    // pixel it changed, not a length it broke.
    let pixels = u.take_rest();
    // Bounded by `MAX_DIM²`, so the product is an ordinary multiply.
    let needed = width * height;
    if needed > pixels.len() {
        // Short buffer: a documented precondition violation, and `tests/bounds.rs` already owns the
        // panic it causes.
        return;
    }

    let minutiae = detect_minutiae(GrayImage {
        data: &pixels[..needed],
        width,
        height,
        ppi: PPI,
    });

    for m in &minutiae {
        // Strict on both ends. `y == height` is exactly the `ih - minutia.y` flip landing one past
        // the last row, which is the failure this target exists to catch.
        assert!(
            m.x >= 0 && (m.x as usize) < width,
            "minutia x={} outside 0..{width} ({width}x{height} image): it reaches bozorth3 as a \
             coordinate, not as an error",
            m.x
        );
        assert!(
            m.y >= 0 && (m.y as usize) < height,
            "minutia y={} outside 0..{height} ({width}x{height} image): it reaches bozorth3 as a \
             coordinate, not as an error",
            m.y
        );
        assert!(
            (0..=359).contains(&m.theta),
            "minutia theta={} outside 0..=359 ({width}x{height} image)",
            m.theta
        );
    }
});
