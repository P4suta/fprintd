// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! End-to-end **image → minutiae → match**, hardware-free and fixture-free.
//!
//! This closes the host-image loop the pipeline joins: [`fprint_pipeline::template_from_images`]
//! runs the real MINDTCT detector ([`fprint_mindtct`]) over a synthetic fingerprint frame, converts the
//! detected minutiae into an [`fprint_core::Template::Nbis`], and
//! [`fprint_pipeline::nbis_match_score`] scores it with the real BOZORTH3 matcher
//! ([`fprint_bozorth3`]). No `.raw` fixtures are read: the images are generated in-process by a tiny LCG
//! grating (the same idiom as `docker/mindtct-oracle/gen_corpus.py`), so every byte — and therefore
//! every detected minutia and every score — is reproducible on any platform.
//!
//! The evidence: a capture self-matches strongly, a re-noised recapture of the *same* finger still
//! clears threshold, an unrelated finger scores far below it, and — because BOZORTH3 is rotation- and
//! translation-invariant by construction — a rotated recapture still matches.

use fprint_pipeline::fprint_core::Template;
use fprint_pipeline::{extract_minutiae, nbis_match_score, template_from_images, GrayImage};

/// Driver match threshold, matching the backend's `real_matching.rs` convention.
const THRESHOLD: u32 = 40;

/// Scan resolution recorded in every synthetic frame (500 ppi, as in the oracle corpus).
const PPI: u16 = 500;

// --- Deterministic synthetic-fingerprint generator (LCG grating) ---------------------------------
// A faithful port of `docker/mindtct-oracle/gen_corpus.py`'s grating: parallel sinusoidal ridges,
// gently curved, with scattered ridge-dislocation dipoles that plant ridge-ending / bifurcation
// minutiae — exactly the structure MINDTCT is built to find. A tiny LCG (no RNG crate) keeps it
// byte-reproducible.

struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Lcg(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1))
    }
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
}

/// Round-to-nearest into the 8-bit range.
fn clamp8(v: f64) -> u8 {
    (v + 0.5).floor().clamp(0.0, 255.0) as u8
}

/// The parameters of one synthetic ridge field (see [`grating`]).
struct Grating {
    width: usize,
    height: usize,
    seed: u64,
    /// Ridge spacing in pixels.
    period: f64,
    /// Ridge-field orientation in degrees.
    angle_deg: f64,
    /// Quadratic bend of the ridge normal (0 = perfectly straight ridges).
    curve: f64,
    /// Additive noise half-range in gray levels.
    noise: i64,
    /// Number of ridge-dislocation dipoles (each plants a resolvable minutia).
    disloc: usize,
}

/// Pick `disloc` ridge-dislocation dipoles inside the image, positions drawn from the LCG. Each
/// dipole is a +1/-1 pair of phase singularities a ridge-period apart; overlaid on the grating it
/// inserts one extra half-ridge — a clean ridge ending / bifurcation.
fn dislocations(g: &Grating) -> Vec<(f64, f64, f64)> {
    let mut r = Lcg::new(g.seed ^ 0x0D15_104A);
    let margin = (2.0 * g.period) as usize + 10;
    let mut sings = Vec::new();
    if g.width <= 2 * margin || g.height <= 2 * margin {
        return sings;
    }
    let sep = (g.period.round() as usize).max(4);
    for _ in 0..g.disloc {
        let sx = margin + (r.next() as usize) % (g.width - 2 * margin);
        let sy = margin + (r.next() as usize) % (g.height - 2 * margin);
        let ox = sep + (r.next() as usize) % 3;
        let oy = (r.next() as i64) % 3 - 1;
        sings.push((sx as f64, sy as f64, 1.0));
        sings.push(((sx + ox) as f64, (sy as i64 + oy) as f64, -1.0));
    }
    sings
}

/// Render a [`Grating`] to a row-major 8-bit grayscale buffer.
fn grating(g: &Grating) -> Vec<u8> {
    const AMP: f64 = 95.0;
    const DC: f64 = 128.0;
    let mut r = Lcg::new(g.seed);
    let (cx, cy) = (g.width as f64 / 2.0, g.height as f64 / 2.0);
    let (ca, sa) = (
        g.angle_deg.to_radians().cos(),
        g.angle_deg.to_radians().sin(),
    );
    let kf = 2.0 * std::f64::consts::PI / g.period;
    let sings = if g.disloc > 0 {
        dislocations(g)
    } else {
        Vec::new()
    };
    let mut data = vec![0u8; g.width * g.height];
    for y in 0..g.height {
        for x in 0..g.width {
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            // Coordinate along the ridge normal, bent by the quadratic curvature term.
            let u = dx * ca + dy * sa;
            let v = -dx * sa + dy * ca;
            let mut phase = kf * (u + g.curve * v * v);
            for &(sx, sy, ch) in &sings {
                phase += ch * (y as f64 - sy).atan2(x as f64 - sx);
            }
            let mut val = DC + AMP * phase.cos();
            if g.noise != 0 {
                val += ((r.next() as i64) % (2 * g.noise + 1) - g.noise) as f64;
            }
            data[y * g.width + x] = clamp8(val);
        }
    }
    data
}

/// Add independent per-pixel sensor noise (half-range 6) to a frame: a *recapture* of the same
/// finger, differing only in noise — a genuine, non-trivial re-detection, not a copy.
fn recapture(data: &[u8], seed: u64) -> Vec<u8> {
    let mut r = Lcg::new(seed);
    data.iter()
        .map(|&p| clamp8(f64::from(p) + ((r.next() as i64) % 13 - 6) as f64))
        .collect()
}

/// Rotate a frame 90° clockwise, returning `(data, new_width, new_height)`.
fn rotate90(data: &[u8], w: usize, h: usize) -> (Vec<u8>, usize, usize) {
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            // (x, y) in the w×h source maps to (h-1-y, x) in the h×w rotation.
            out[x * h + (h - 1 - y)] = data[y * w + x];
        }
    }
    (out, h, w)
}

fn image(data: &[u8], w: usize, h: usize) -> GrayImage<'_> {
    GrayImage::new(data, w, h, PPI).expect("valid dims")
}

/// A healthy, minutiae-rich reference finger (BOZORTH3 scores 0 below ~10 minutiae, so the frame is
/// sized to clear that with margin).
fn reference_finger() -> Grating {
    Grating {
        width: 256,
        height: 256,
        seed: 0x1111,
        period: 9.0,
        angle_deg: 20.0,
        curve: 0.0018,
        noise: 18,
        disloc: 10,
    }
}

/// An unrelated finger: a different seed (different dislocations), spacing, and orientation.
fn other_finger() -> Grating {
    Grating {
        width: 256,
        height: 256,
        seed: 0x2222,
        period: 10.0,
        angle_deg: 55.0,
        curve: 0.0036,
        noise: 22,
        disloc: 14,
    }
}

fn sample_count(t: &Template) -> usize {
    match t {
        Template::Nbis(s) => s.len(),
        _ => 0,
    }
}

fn minutiae_in_first_sample(t: &Template) -> usize {
    match t {
        Template::Nbis(s) => s.first().map_or(0, Vec::len),
        _ => 0,
    }
}

#[test]
fn extract_minutiae_finds_a_healthy_set() {
    let g = reference_finger();
    let data = grating(&g);
    let m = extract_minutiae(image(&data, g.width, g.height));
    assert!(
        m.len() >= 10,
        "the reference frame should yield a matchable minutiae set, got {}",
        m.len()
    );
}

#[test]
fn a_capture_self_matches_strongly() {
    let g = reference_finger();
    let data = grating(&g);
    let template = template_from_images(&[image(&data, g.width, g.height)]);
    assert_eq!(sample_count(&template), 1);
    assert!(minutiae_in_first_sample(&template) >= 10);

    let score = nbis_match_score(&template, &template);
    assert!(
        score >= THRESHOLD,
        "a capture must self-match above threshold {THRESHOLD}, got {score}"
    );
}

#[test]
fn re_noised_recapture_of_same_finger_matches() {
    let g = reference_finger();
    let data = grating(&g);
    let again = recapture(&data, 0xABCD);

    let enrolled = template_from_images(&[image(&data, g.width, g.height)]);
    let probe = template_from_images(&[image(&again, g.width, g.height)]);

    let score = nbis_match_score(&enrolled, &probe);
    assert!(
        score >= THRESHOLD,
        "a re-noised recapture of the same finger must verify (>= {THRESHOLD}), got {score}"
    );
}

#[test]
fn unrelated_finger_scores_below_threshold() {
    let a = reference_finger();
    let b = other_finger();
    let da = grating(&a);
    let db = grating(&b);

    let ta = template_from_images(&[image(&da, a.width, a.height)]);
    let tb = template_from_images(&[image(&db, b.width, b.height)]);

    let self_score = nbis_match_score(&ta, &ta);
    let cross = nbis_match_score(&ta, &tb);
    assert!(
        cross < THRESHOLD,
        "an unrelated finger must not verify (< {THRESHOLD}), got {cross}"
    );
    assert!(
        self_score > cross,
        "self-match ({self_score}) must dominate the unrelated cross-score ({cross})"
    );
}

#[test]
fn multi_capture_template_keeps_one_sample_per_frame() {
    // A host-image sensor enrolls several frames; `template_from_images` preserves one minutiae
    // sample per capture, and the matcher's max-over-samples still verifies the true finger while
    // rejecting a stranger.
    let a = reference_finger();
    let b = other_finger();
    let da = grating(&a);
    let da2 = recapture(&da, 0x1357);
    let db = grating(&b);

    let enrolled = template_from_images(&[
        image(&da, a.width, a.height),
        image(&da2, a.width, a.height),
    ]);
    assert_eq!(sample_count(&enrolled), 2, "one sample per enrolled frame");

    let genuine = template_from_images(&[image(&da, a.width, a.height)]);
    let stranger = template_from_images(&[image(&db, b.width, b.height)]);

    assert!(
        nbis_match_score(&enrolled, &genuine) >= THRESHOLD,
        "the enrolled finger must verify against the multi-sample template"
    );
    assert!(
        nbis_match_score(&enrolled, &stranger) < THRESHOLD,
        "a stranger must not verify against the multi-sample template"
    );
}

#[test]
fn rotated_recapture_still_matches_rotation_invariant() {
    // BOZORTH3 compares minutiae by relative geometry, so it is rotation- and translation-invariant
    // by construction: a rotated recapture of the same finger still matches. (This is why a rotation
    // cannot serve as a negative — the discrimination signal is finger identity, exercised above.)
    let g = reference_finger();
    let data = grating(&g);
    let (rot, rw, rh) = rotate90(&data, g.width, g.height);

    let enrolled = template_from_images(&[image(&data, g.width, g.height)]);
    let rotated = template_from_images(&[image(&rot, rw, rh)]);

    let score = nbis_match_score(&enrolled, &rotated);
    assert!(
        score >= THRESHOLD,
        "a rotated recapture of the same finger must still verify (>= {THRESHOLD}), got {score}"
    );
}
