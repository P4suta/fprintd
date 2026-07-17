<!-- SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors -->
<!-- -->
<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# fprint-mindtct

A pure-Rust, dependency-free reimplementation of MINDTCT — NIST NBIS's minutiae detector.
Given an 8-bit grayscale fingerprint image, `detect_minutiae` produces a list of `Minutia`
(`x`, `y`, `theta`, `quality`) reproducing the stock NBIS tool's `xyt` output. The detector is
a self-contained image-processing kernel taking its own `GrayImage` and returning its own
`Minutia` — the `xyt` triple is an interoperability fact — so it has no dependency on the
domain model. A consumer converts to `fprint_core::Minutia` at the boundary.

## Provenance

MINDTCT is public-domain U.S. Government software (17 USC §105). This crate is a faithful port
of the stock upstream NBIS algorithm, verified black-box against the stock C tool — reproducing
its `xyt` output bit-for-bit — and deliberately not derived from libfprint's patched
`nbis/mindtct/` copy. The NBIS lineage is provenance, not a licence; the crate carries
`MIT OR Apache-2.0` like the rest of the project.

## Quickstart

```text
use fprint_mindtct::{detect_minutiae, GrayImage};

// row-major, one byte per pixel (0 = black, 255 = white); ppi is the scan resolution
let img = GrayImage::new(&pixels, 128, 128, 500)?;
let minutiae = detect_minutiae(img);

for m in &minutiae {
    // m.x, m.y (origin bottom-left), m.theta in 0..=359, m.quality in 0..=100
}
```

`GrayImage::new` rejects an image MINDTCT cannot process — a dimension below the detection floor
(`ImageError::TooSmall`, `MIN_DETECTABLE_DIM`), one past `i32::MAX` (`TooLarge`), or a buffer shorter
than `width * height` (`BufferTooShort`); a longer buffer is accepted and its tail ignored. A
successfully built `GrayImage` is guaranteed detectable, so an empty `detect_minutiae` result means
"no minutiae", never "image rejected".

## Links

- API docs: <https://docs.rs/fprint-mindtct>
- crates.io: <https://crates.io/crates/fprint-mindtct>
- Algorithm notes: `docs/mindtct-algorithm.md`

## License

`MIT OR Apache-2.0`, at your option.
