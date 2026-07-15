// SPDX-FileCopyrightText: NIST NBIS — U.S. Government work, public domain (title 17 §105)
//
// SPDX-License-Identifier: LicenseRef-NBIS-PD

//! Fast, hardware-free sanity checks: the matcher must not panic on degenerate or realistic
//! inputs, and must honour the coarse invariants (too-few-minutiae → 0; a print matches itself
//! more strongly than it matches an unrelated print). Exact score parity with the C tool is
//! covered separately by the golden-fixture test.

use fprint_bozorth3::{match_score, Minutia};

/// A deterministic pseudo-random minutia set of `n` points (no RNG crate — a tiny LCG).
fn synth(n: usize, seed: u64) -> Vec<Minutia> {
    let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
    let mut next = || {
        s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (s >> 33) as i32
    };
    (0..n)
        .map(|_| Minutia {
            x: next().rem_euclid(500),
            y: next().rem_euclid(500),
            theta: next().rem_euclid(360),
        })
        .collect()
}

#[test]
fn too_few_minutiae_scores_zero() {
    let a = synth(9, 1);
    let b = synth(9, 1);
    assert_eq!(
        match_score(&a, &b),
        0,
        "fewer than 10 minutiae must score 0"
    );
    assert_eq!(match_score(&[], &[]), 0);
    assert_eq!(match_score(&synth(20, 7), &synth(3, 9)), 0);
}

#[test]
fn identical_print_matches_itself() {
    let a = synth(40, 42);
    let self_score = match_score(&a, &a);
    // A print matched against itself: every edge is compatible, so the score is substantial.
    assert!(
        self_score >= 40,
        "self-match unexpectedly low: {self_score}"
    );
}

#[test]
fn self_match_beats_unrelated() {
    let a = synth(40, 42);
    let b = synth(40, 99);
    let self_score = match_score(&a, &a);
    let cross = match_score(&a, &b);
    assert!(
        self_score > cross,
        "self ({self_score}) should beat unrelated ({cross})"
    );
}

#[test]
fn does_not_panic_on_varied_sizes_and_dupes() {
    // Exercise a range of sizes, including at/above the cap and coincident coordinates.
    for n in [10usize, 11, 50, 150, 200, 260] {
        let a = synth(n, n as u64);
        let b = synth(n, (n as u64).wrapping_add(1));
        let _ = match_score(&a, &b);
        let _ = match_score(&a, &a);
    }
    // Coincident coordinates (degenerate) must not panic.
    let dupes: Vec<Minutia> = (0..15)
        .map(|i| Minutia {
            x: 100,
            y: 100,
            theta: (i * 20) % 360,
        })
        .collect();
    let _ = match_score(&dupes, &dupes);
}
