// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The algebraic properties of [`fprint_bozorth3::match_score`], over generated inputs.
//!
//! `tests/golden.rs` pins what the matcher *scores*; this pins what the score **does not depend
//! on**. Every case is drawn from an [`Lcg`] seed, and every failure message carries the seed.
//!
//! ## Every pair here must score above zero
//!
//! BOZORTH3 scores two unrelated prints **0**, and two independently generated prints are unrelated
//! — so an invariance checked on such a pair reads `0 == 0` and **passes whatever the code does**.
//! A suite of them would survive deleting the sort out of `xyt::prepare`.
//!
//! So the pairs come from [`related_pair`], which jitters one print into a second impression of
//! itself, and every test asserts the pair scores above zero before comparing anything. That guard
//! is the load-bearing line in this file: without it these tests would look identical and prove
//! nothing.
//!
//! ## What is proved, and how far
//!
//! * **Translation is exact.** Shifting a print moves no score at all. Stage 1 reads only `dx`/`dy`,
//!   `xyt::prepare`'s `(x, y)` sort key is order-preserving under a uniform shift, and the one
//!   absolute-coordinate reader — the cluster centroid — enters the score only as a difference
//!   between two centroids of the same print. **The honest limit: the centroid divides with
//!   truncation toward zero**, so `(sum + tot·d) / tot == sum / tot + d` fails where the coordinate
//!   sum crosses zero (pinned in `src/cluster.rs`). These tests therefore stay on non-negative
//!   coordinates and non-negative shifts, where it holds.
//! * **Permutation is exact, under two conditions.** Both are load-bearing and both are documented:
//!   the `(x, y)` sort breaks ties by input position, so **all `(x, y)` must be distinct**
//!   (`src/xyt.rs:51-52`), and the cap keeps the first 150 *as given*, so **`len` must be ≤ 150**
//!   (`src/xyt.rs:17-19`). This is the direct check on `src/xyt.rs:8`'s "the sort is
//!   **load-bearing**": the sort is what makes input order irrelevant.
//! * **Rotation is a threshold, not an identity.** Rotating a print through trig changes the rounded
//!   integer coordinates, which changes the `(x, y)` sort order, which changes the rows stage 1
//!   emits. So a rotated impression does not reproduce the score — it **out-scores an unrelated
//!   print**, which is the claim `src/lib.rs` makes and the only one that is true.
//!
//! ## What is deliberately absent
//!
//! * **Symmetry.** `match_score(a, b) != match_score(b, a)` in general: stage 2's loop bounds are
//!   asymmetric by construction (`src/inter.rs:22-24`).
//! * **Self-match maximality.** A print need not score highest against itself. It is not a theorem,
//!   so it is not asserted here.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use fprint_bozorth3::{
    match_score, Minutia, DEFAULT_BOZORTH_MINUTIAE, MIN_COMPUTABLE_BOZORTH_MINUTIAE,
};
use fprint_testkit::{gen, ByteSource, Lcg};

/// The field generated prints are scattered over. Large enough that 150 distinct `(x, y)` are easy
/// to draw, small enough that a print has the dense edge structure the matcher is about.
const FIELD: i32 = 400;

/// Minutiae per generated print: above [`MIN_COMPUTABLE_BOZORTH_MINUTIAE`], below
/// [`DEFAULT_BOZORTH_MINUTIAE`], so neither guard is what these tests are measuring.
const N: usize = 40;

/// `n` triples with **pairwise-distinct `(x, y)`** and non-negative coordinates — the precondition
/// [`permutation_invariance_holds_for_distinct_coordinates_under_the_cap`] needs, and the
/// non-negativity [`translation_is_exact_for_non_negative_shifts`] needs.
///
/// Drawn from an oversized batch and deduplicated, so the draw is bounded rather than a retry loop.
fn distinct_triples(src: &mut Lcg, n: usize) -> Vec<(i32, i32, i32)> {
    let mut seen = BTreeSet::new();
    let out: Vec<(i32, i32, i32)> = gen::xyt(src, n * 3, FIELD, FIELD)
        .into_iter()
        .filter(|&(x, y, _)| seen.insert((x, y)))
        .take(n)
        .collect();
    assert_eq!(
        out.len(),
        n,
        "seed {}: could not draw {n} distinct points",
        src.seed()
    );
    out
}

fn as_minutiae(v: &[(i32, i32, i32)]) -> Vec<Minutia> {
    v.iter()
        .map(|&(x, y, theta)| Minutia { x, y, theta })
        .collect()
}

/// A print with distinct `(x, y)`, for the tests that need only one.
fn distinct_print(src: &mut Lcg, n: usize) -> Vec<Minutia> {
    as_minutiae(&distinct_triples(src, n))
}

/// A probe and a **genuine second impression of it**: the same points, re-placed and re-measured
/// within the matcher's tolerances. Both sides have pairwise-distinct `(x, y)`.
///
/// Two *independent* prints will not do, and this is the whole reason this helper exists: BOZORTH3
/// scores unrelated prints **0**, and an invariant checked on a pair that scores 0 is `0 == 0` —
/// it holds no matter what the code does. Every caller asserts the pair scores above zero before
/// trusting the comparison, so a generator that drifts back to noise fails loudly instead of
/// quietly proving nothing.
fn related_pair(src: &mut Lcg, n: usize) -> (Vec<Minutia>, Vec<Minutia>) {
    let base = distinct_triples(src, n);
    let jittered = gen::xyt_jittered(src, &base, 3);
    // Jitter can move two points onto one pixel; keep the first at each, so both sides stay
    // distinct and the gallery reads as a slightly partial capture.
    let mut seen = BTreeSet::new();
    let gallery: Vec<(i32, i32, i32)> = jittered
        .into_iter()
        .filter(|&(x, y, _)| seen.insert((x, y)))
        .collect();
    assert!(
        gallery.len() >= MIN_COMPUTABLE_BOZORTH_MINUTIAE,
        "seed {}: jitter collapsed the gallery to {} points",
        src.seed(),
        gallery.len()
    );
    (as_minutiae(&base), as_minutiae(&gallery))
}

/// The guard that keeps every property below from being vacuous: a pair that scores 0 satisfies
/// each of them trivially.
fn assert_meaningful(score: u32, seed: u64) {
    assert!(
        score > 0,
        "seed {seed}: the pair scores 0, so every invariance below would hold trivially — \
         the generator, not the matcher, is what this would be testing"
    );
}

/// `print`, moved by `(dx, dy)`.
fn shifted(print: &[Minutia], dx: i32, dy: i32) -> Vec<Minutia> {
    print
        .iter()
        .map(|m| Minutia {
            x: m.x + dx,
            y: m.y + dy,
            theta: m.theta,
        })
        .collect()
}

/// A Fisher-Yates shuffle driven by `src`: a failing order is reproducible from the seed.
fn shuffled(src: &mut Lcg, print: &[Minutia]) -> Vec<Minutia> {
    let mut v = print.to_vec();
    for i in (1..v.len()).rev() {
        let j = src.in_range(0, i as i32) as usize;
        v.swap(i, j);
    }
    v
}

/// Non-negative shifts, kept well inside `i32` so that the coordinate *sums* the centroid divides
/// cannot overflow — this test is about truncation, not about a different failure.
const SHIFTS: [(i32, i32); 6] = [(0, 0), (1, 0), (0, 1), (7, 13), (250, 250), (1000, 1000)];

#[test]
fn translation_is_exact_for_non_negative_shifts() {
    for seed in 0..12u64 {
        let mut lcg = Lcg::new(seed);
        let (a, b) = related_pair(&mut lcg, N);
        let want = match_score(&a, &b);
        assert_meaningful(want, lcg.seed());

        for (dx, dy) in SHIFTS {
            // The probe alone.
            assert_eq!(
                match_score(&shifted(&a, dx, dy), &b),
                want,
                "seed {}: shifting the probe by ({dx}, {dy}) moved the score",
                lcg.seed()
            );
            // The gallery alone: the centroid difference is taken per print, so each side is
            // independently invariant.
            assert_eq!(
                match_score(&a, &shifted(&b, dx, dy)),
                want,
                "seed {}: shifting the gallery by ({dx}, {dy}) moved the score",
                lcg.seed()
            );
            // Both, which is the rigid re-placement a second impression actually is.
            assert_eq!(
                match_score(&shifted(&a, dx, dy), &shifted(&b, dx, dy)),
                want,
                "seed {}: shifting both by ({dx}, {dy}) moved the score",
                lcg.seed()
            );
        }
    }
}

#[test]
fn translation_is_exact_for_a_self_match_too() {
    // A self-match drives the cluster stage hardest — every edge is compatible, so the centroid
    // path this property turns on is the one under test.
    for seed in 100..106u64 {
        let mut lcg = Lcg::new(seed);
        let a = distinct_print(&mut lcg, N);
        let want = match_score(&a, &a);
        assert_meaningful(want, lcg.seed());
        for (dx, dy) in SHIFTS {
            let moved = shifted(&a, dx, dy);
            assert_eq!(
                match_score(&moved, &moved),
                want,
                "seed {}: a self-match moved under a ({dx}, {dy}) shift",
                lcg.seed()
            );
        }
    }
}

#[test]
fn permutation_invariance_holds_for_distinct_coordinates_under_the_cap() {
    for seed in 0..12u64 {
        let mut lcg = Lcg::new(seed);
        let (a, b) = related_pair(&mut lcg, N);
        let want = match_score(&a, &b);
        assert_meaningful(want, lcg.seed());
        assert!(
            a.len() <= DEFAULT_BOZORTH_MINUTIAE && b.len() <= DEFAULT_BOZORTH_MINUTIAE,
            "the cap is a precondition of this property, not a thing it proves"
        );

        for round in 0..8 {
            let pa = shuffled(&mut lcg, &a);
            let pb = shuffled(&mut lcg, &b);
            assert_eq!(
                match_score(&pa, &b),
                want,
                "seed {}, round {round}: shuffling the probe moved the score",
                lcg.seed()
            );
            assert_eq!(
                match_score(&a, &pb),
                want,
                "seed {}, round {round}: shuffling the gallery moved the score",
                lcg.seed()
            );
            assert_eq!(
                match_score(&pa, &pb),
                want,
                "seed {}, round {round}: shuffling both moved the score",
                lcg.seed()
            );
        }
    }
}

#[test]
fn match_score_is_deterministic() {
    for seed in 0..12u64 {
        let mut lcg = Lcg::new(seed);
        let (a, b) = related_pair(&mut lcg, N);
        let first = match_score(&a, &b);
        assert_meaningful(first, lcg.seed());
        for _ in 0..3 {
            assert_eq!(
                match_score(&a, &b),
                first,
                "seed {}: the same pair scored twice, differently",
                lcg.seed()
            );
        }
    }
}

#[test]
fn a_print_under_min_computable_always_scores_zero() {
    // True by construction — `prepare` caps `nrows` at the input length, and the guard reads
    // `nrows` — so this holds for every input, not for the three in `tests/smoke.rs`.
    for seed in 0..20u64 {
        let mut lcg = Lcg::new(seed);
        let full = distinct_print(&mut lcg, N);
        for n in 0..MIN_COMPUTABLE_BOZORTH_MINUTIAE {
            let short = distinct_print(&mut lcg, n);
            assert_eq!(
                match_score(&short, &full),
                0,
                "seed {}: a {n}-minutia probe must score 0",
                lcg.seed()
            );
            assert_eq!(
                match_score(&full, &short),
                0,
                "seed {}: a {n}-minutia gallery must score 0",
                lcg.seed()
            );
            assert_eq!(
                match_score(&short, &short),
                0,
                "seed {}: {n} against {n} must score 0, self-match or not",
                lcg.seed()
            );
        }
    }
}

// --- the rotation claim -----------------------------------------------------------------------

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Parse a 3-column `.xyt` file ("x y theta" per line) into minutiae.
fn load_xyt(path: &Path) -> Vec<Minutia> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let mut it = l.split_whitespace();
            let x = it.next().unwrap().parse().unwrap();
            let y = it.next().unwrap().parse().unwrap();
            let theta = it.next().unwrap().parse().unwrap();
            Minutia { x, y, theta }
        })
        .collect()
}

/// Every pair in `pairs.txt`, scored.
fn scored_pairs() -> Vec<(String, u32)> {
    let dir = fixtures_dir();
    let pairs_text = std::fs::read_to_string(dir.join("pairs.txt")).expect("pairs.txt missing");
    pairs_text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let mut it = line.split_whitespace();
            let tag = it.next().unwrap().to_string();
            let probe = load_xyt(&dir.join(it.next().unwrap()));
            let gallery = load_xyt(&dir.join(it.next().unwrap()));
            let score = match_score(&probe, &gallery);
            (tag, score)
        })
        .collect()
}

#[test]
fn rotated_self_match_beats_unrelated() {
    // The true rotation claim, and the reason `src/lib.rs` says "tolerates rotation" rather than
    // "rotation-invariant": a rigid rotation is not score-preserving, it is score-*surviving*.
    //
    // The corpus already holds what this needs, so nothing is invented here: `rot{deg}_{key}` pairs
    // a base print with a rigid copy of itself rotated by `deg` about its centroid and translated,
    // and `cross_{key}_{other}` pairs the same base print with an unrelated one. Same probe on both
    // sides of the comparison, so the only difference is what it is matched against. Seven angles
    // across all four quadrants, on every base print in the corpus.
    //
    // **How much slack there is: all of it.** Every unrelated control scores 0, so this threshold is
    // "a rotated print scores above nothing at all", and the margin is the rotated score itself —
    // 6 at ten minutiae, 948 at a hundred and fifty. That is a weak bar deliberately: the strong
    // statement about these same pairs is `tests/golden.rs`, which holds each rotated score to the
    // stock C tool's integer. This test says the one thing the golden cannot — that the number means
    // a match — and it is the claim `src/lib.rs` now makes.
    let scores: Vec<(String, u32)> = scored_pairs();
    let cross: Vec<&(String, u32)> = scores
        .iter()
        .filter(|(t, _)| t.starts_with("cross_"))
        .collect();
    assert!(!cross.is_empty(), "no cross pairs — corpus missing?");

    let mut checked = 0usize;
    let mut failures = Vec::new();
    for (tag, rot_score) in scores.iter().filter(|(t, _)| t.starts_with("rot")) {
        // "rot15_10s1" → key "10s1" → the "cross_10s1_*" pair built from the same base print.
        let key = tag.split_once('_').expect("bad rot tag").1;
        let prefix = format!("cross_{key}_");
        let (cross_tag, cross_score) = cross
            .iter()
            .find(|(t, _)| t.starts_with(&prefix))
            .unwrap_or_else(|| panic!("no cross pair for {key}"));
        if rot_score <= cross_score {
            failures.push(format!(
                "{tag} scored {rot_score}, not above unrelated {cross_tag} at {cross_score}"
            ));
        }
        checked += 1;
    }
    assert!(checked > 0, "no rot pairs checked — corpus missing?");
    assert!(
        failures.is_empty(),
        "{}/{checked} rotated pairs failed to beat their unrelated control:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}
