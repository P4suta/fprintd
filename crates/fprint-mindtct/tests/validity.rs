// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The mindtct → bozorth3 seam: **every [`Minutia`] [`detect_minutiae`] returns lands inside the
//! image, with `theta` and `quality` inside their documented ranges.**
//!
//! The golden suite proves the port reproduces the stock C. It cannot prove the numbers are *usable*
//! — a faithful port of a coordinate flip that walks off the image would be bit-exact and still
//! wrong downstream. This file checks the contract the next crate relies on, over the 13 corpus
//! images and over `fprint_testkit::gen::gray_image` noise.
//!
//! ## Why the seam needs a guard here and not downstream
//!
//! An out-of-image minutia flows into `fprint_bozorth3`'s `xyt::prepare`, which validates nothing.
//! It would be consumed as a coordinate, not rejected — so the failure would be **silent**, showing
//! up as a slightly wrong match score rather than an error. That is worse than a panic, and it is
//! why the bound is asserted at the producer.
//!
//! ## The `y` flip, and the margin that saves it
//!
//! `src/xyt.rs`'s `lfs2nist_minutia_xyt` computes `y = ih - minutia.y` (a bottom-left origin). The
//! arithmetic is faithful to stock `lfs2nist_minutia_XYT`, and it maps `minutia.y == 0` to
//! `y == height` — one past the last row. Nothing in `xyt.rs` clamps it.
//!
//! `minutia.y == 0` is not reachable. Detection runs on a block grid and never emits a minutia in
//! the top [`DETECTOR_TOP_MARGIN`] rows; the observed pre-flip minimum is exactly that margin across
//! both the corpus and noise, which [`raw_minutiae_keep_clear_of_the_top_edge`] pins. So `y` lands in
//! `1..=height - DETECTOR_TOP_MARGIN` and the strict bound holds. The bound below is asserted
//! **strictly** (`y < height`): it passes today because of that margin, not because of a clamp, and
//! if detection ever reaches row 0 this test is the thing that says so.
//!
//! ## Honest limits
//!
//! Noise is not a fingerprint. It exercises the bound cheaply and widely, but it cannot show that a
//! *plausible* image drives a minutia to an edge — only that these inputs do not. Noise images are
//! capped at [`NOISE_SIZE`]; see `corpus_adequacy.rs` for why a larger one is not used here.

use std::path::{Path, PathBuf};

#[cfg(feature = "unstable-diagnostics")]
use fprint_mindtct::debug_raw_minutiae;
use fprint_mindtct::{detect_minutiae, GrayImage, Minutia};

/// The pre-flip row the detector never emits above — the block size the block-map grid steps in.
/// The observed minimum `minutia.y` over the corpus and over noise is exactly this value.
const DETECTOR_TOP_MARGIN: i32 = 8;

/// Edge of the noise images. Large enough to produce minutiae, small enough to stay fast.
const NOISE_SIZE: usize = 96;

/// Noise seeds drawn per size. Deterministic: a failure names the seed that produced it.
const NOISE_SEEDS: u64 = 24;

/// A margin of zero would make the `y < height` bound an accident rather than a consequence.
const _: () = assert!(DETECTOR_TOP_MARGIN > 0);

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The corpus image basenames, in `manifest.txt` order.
fn corpus_names() -> Vec<String> {
    let dir = fixtures_dir();
    let text = std::fs::read_to_string(dir.join("manifest.txt"))
        .expect("manifest.txt missing — run `mise run mindtct-oracle`");
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

/// A corpus image: its bytes and the geometry from its `.manifest` sidecar ("width height ppi").
struct Image {
    data: Vec<u8>,
    width: usize,
    height: usize,
    ppi: u16,
}

impl Image {
    fn as_gray(&self) -> GrayImage<'_> {
        GrayImage::new(&self.data, self.width, self.height, self.ppi)
            .expect("buffer holds the image")
    }
}

fn load_corpus_image(name: &str) -> Image {
    let dir = fixtures_dir();
    let text = std::fs::read_to_string(dir.join(format!("{name}.manifest")))
        .unwrap_or_else(|e| panic!("read {name}.manifest: {e}"));
    let mut it = text.split_whitespace();
    let width = it.next().expect("manifest: width").parse().expect("width");
    let height = it
        .next()
        .expect("manifest: height")
        .parse()
        .expect("height");
    let ppi = it.next().expect("manifest: ppi").parse().expect("ppi");
    let data = std::fs::read(dir.join(format!("{name}.raw")))
        .unwrap_or_else(|e| panic!("read {name}: {e}"));
    Image {
        data,
        width,
        height,
        ppi,
    }
}

/// Every way one minutia can violate the seam's contract, as human-readable strings.
fn violations(m: &Minutia, width: usize, height: usize) -> Vec<String> {
    let mut out = Vec::new();
    if !(0..width as i32).contains(&m.x) {
        out.push(format!("x {} outside 0..{width}", m.x));
    }
    // Strict: `lfs2nist` can represent `y == height`, and detection must never produce it.
    if !(0..height as i32).contains(&m.y) {
        out.push(format!("y {} outside 0..{height}", m.y));
    }
    if !(0..360).contains(&m.theta) {
        out.push(format!("theta {} outside 0..360", m.theta));
    }
    if !(0..=100).contains(&m.quality) {
        out.push(format!("quality {} outside 0..=100", m.quality));
    }
    out
}

/// Check one image's minutiae, returning a description per offending minutia.
fn check(minutiae: &[Minutia], width: usize, height: usize) -> Vec<String> {
    minutiae
        .iter()
        .enumerate()
        .flat_map(|(i, m)| {
            violations(m, width, height)
                .into_iter()
                .map(move |v| format!("[{i}] {m:?}: {v}"))
        })
        .collect()
}

#[test]
fn corpus_minutiae_are_inside_the_image_and_ranges() {
    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for name in corpus_names() {
        let img = load_corpus_image(&name);
        let got = detect_minutiae(img.as_gray());
        let bad = check(&got, img.width, img.height);
        if !bad.is_empty() {
            failures.push(format!(
                "{name} ({}x{}):\n    {}",
                img.width,
                img.height,
                bad.join("\n    ")
            ));
        }
        checked += 1;
    }
    assert!(checked > 0, "no images checked — corpus missing?");
    assert!(
        failures.is_empty(),
        "{}/{} corpus images produced an invalid minutia:\n  {}",
        failures.len(),
        checked,
        failures.join("\n  ")
    );
}

#[test]
fn noise_minutiae_are_inside_the_image_and_ranges() {
    use fprint_testkit::{gen::gray_image, Lcg};

    let mut failures: Vec<String> = Vec::new();
    for seed in 0..NOISE_SEEDS {
        let mut lcg = Lcg::new(seed);
        let data = gray_image(&mut lcg, NOISE_SIZE, NOISE_SIZE);
        let img =
            GrayImage::new(&data, NOISE_SIZE, NOISE_SIZE, 500).expect("buffer holds the image");
        let got = detect_minutiae(img);
        let bad = check(&got, NOISE_SIZE, NOISE_SIZE);
        if !bad.is_empty() {
            failures.push(format!("seed {}:\n    {}", lcg.seed(), bad.join("\n    ")));
        }
    }
    assert!(
        failures.is_empty(),
        "{} noise images produced an invalid minutia:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

/// The pre-flip guard behind the `y < height` bound: detection never emits a minutia in the top
/// [`DETECTOR_TOP_MARGIN`] rows, so `y = ih - minutia.y` never reaches `height`. Asserted on the raw
/// (pre-removal) list — removal only drops and relocates minutiae, so the raw list is the widest
/// spread the final one can be drawn from.
#[cfg(feature = "unstable-diagnostics")]
#[test]
fn raw_minutiae_keep_clear_of_the_top_edge() {
    use fprint_testkit::{gen::gray_image, Lcg};

    let mut failures: Vec<String> = Vec::new();
    let mut seen = 0usize;
    for name in corpus_names() {
        let img = load_corpus_image(&name);
        for (i, m) in debug_raw_minutiae(img.as_gray()).iter().enumerate() {
            seen += 1;
            if m.y < DETECTOR_TOP_MARGIN {
                failures.push(format!(
                    "{name} [{i}]: raw y {} is above the {DETECTOR_TOP_MARGIN}-row margin",
                    m.y
                ));
            }
        }
    }
    for seed in 0..NOISE_SEEDS {
        let data = gray_image(&mut Lcg::new(seed), NOISE_SIZE, NOISE_SIZE);
        let img =
            GrayImage::new(&data, NOISE_SIZE, NOISE_SIZE, 500).expect("buffer holds the image");
        for (i, m) in debug_raw_minutiae(img).iter().enumerate() {
            seen += 1;
            if m.y < DETECTOR_TOP_MARGIN {
                failures.push(format!(
                    "noise seed {seed} [{i}]: raw y {} is above the {DETECTOR_TOP_MARGIN}-row margin",
                    m.y
                ));
            }
        }
    }
    assert!(seen > 0, "no raw minutiae seen — corpus missing?");
    assert!(
        failures.is_empty(),
        "detection reached the top edge — the `y < height` bound is no longer structural:\n  {}",
        failures.join("\n  ")
    );
}
