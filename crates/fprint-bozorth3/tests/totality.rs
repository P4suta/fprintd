// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! What [`fprint_bozorth3::match_score`] does with input no fingerprint reader would produce.
//!
//! A published matcher is called with whatever the caller has. This file states the public seam's
//! behaviour on the degenerate ends of its domain — **each case traced through the code, and
//! recorded as it is rather than as it ought to be**.
//!
//! ## The two that are not what a reader expects
//!
//! * **[`MAX_BOZORTH_MINUTIAE`] is read by nothing.** It is exported and documented as the "hard cap
//!   on minutiae per print", but [`match_score`] passes [`DEFAULT_BOZORTH_MINUTIAE`] (150) to
//!   `xyt::prepare`, so 200 is not a cap on anything: 201 minutiae behave exactly like 150.
//! * **A full turn added to `theta` is not the identity.** `theta` is normalized by the reference's
//!   *single conditional subtract*, not a modulo, so `theta - 360` is the same direction and a
//!   different input — and a different score.
//!
//! The two squarings that carry unbounded deltas — `dx * dx` in `src/intra.rs` and `d * d` in
//! `src/inter.rs` — are computed in i64, so an extreme coordinate span or an unfolded `theta`
//! yields a well-defined score rather than an overflow. The valid-input scores are unchanged; the
//! widening only touches deltas past ±46340, which no reader produces.
//!
//! Where the reference's behaviour is an implementation detail rather than a promise — the score of
//! a degenerate print — these tests assert **termination only**. A guessed number would be a second
//! oracle competing with `tests/golden.rs`, and a weaker one.

use fprint_bozorth3::{
    match_score, Minutia, DEFAULT_BOZORTH_MINUTIAE, MAX_BOZORTH_MINUTIAE,
    MIN_COMPUTABLE_BOZORTH_MINUTIAE,
};
use fprint_testkit::{gen, Lcg};

/// A deterministic print of `n` minutiae over a realistic field.
fn print_of(seed: u64, n: usize) -> Vec<Minutia> {
    gen::xyt(&mut Lcg::new(seed), n, 400, 400)
        .into_iter()
        .map(|(x, y, theta)| Minutia { x, y, theta })
        .collect()
}

#[test]
fn nothing_under_min_computable_panics_or_scores() {
    let full = print_of(1, 40);
    for n in 0..MIN_COMPUTABLE_BOZORTH_MINUTIAE {
        let short = print_of(2, n);
        assert_eq!(short.len(), n);
        assert_eq!(match_score(&short, &full), 0, "{n}-minutia probe");
        assert_eq!(match_score(&full, &short), 0, "{n}-minutia gallery");
        assert_eq!(match_score(&short, &short), 0, "{n} against itself");
    }
    // The empty print is the floor of that range, stated on its own because it is the one a caller
    // reaches by accident.
    assert_eq!(match_score(&[], &[]), 0);
    assert_eq!(match_score(&[], &full), 0);
    assert_eq!(match_score(&full, &[]), 0);
}

#[test]
fn a_print_of_identical_minutiae_terminates() {
    // Ten minutiae at one pixel, one angle: every edge has distance 0, and every pair is a tie the
    // `(x, y)` sort cannot order. Past `MIN_COMPUTABLE_BOZORTH_MINUTIAE`, so it runs the full
    // pipeline rather than the guard.
    let same = vec![
        Minutia {
            x: 100,
            y: 100,
            theta: 40,
        };
        MIN_COMPUTABLE_BOZORTH_MINUTIAE
    ];
    // Termination is the whole claim. The score of a print that cannot exist is an implementation
    // detail of the reference, and pinning a guess here would say something this file does not know.
    //
    // The size stays at the minimum on purpose. Identical minutiae make every pair a zero-distance
    // edge and every edge compatible with every other, so stage 2 saturates its 19999-row table from
    // about 20 minutiae up and the cluster stage then walks that table from every seed: a few dozen
    // identical points cost minutes, and buy no claim this does not already make.
    let _ = match_score(&same, &same);
}

#[test]
fn every_length_at_or_above_the_cap_scores_as_its_first_150_do() {
    // `MAX_BOZORTH_MINUTIAE` is exported at lib.rs:47 and documented as the hard cap on minutiae per
    // print. Nothing reads it: `match_score` passes `DEFAULT_BOZORTH_MINUTIAE` to `xyt::prepare`,
    // whose `.take(150)` is the only cap there is. So 200 is not a boundary — 151, 200, 201 and 300
    // all score as the first 150 alone, and the constant names a rule the code does not have.
    //
    // The gallery *is* the 150 the cap keeps, so the kept prefix scores a strong self-match and a
    // dropped tail is unmistakable in the number.
    let long = print_of(6, 300);
    let gallery: Vec<Minutia> = long[..DEFAULT_BOZORTH_MINUTIAE].to_vec();
    let want = match_score(&long[..DEFAULT_BOZORTH_MINUTIAE], &gallery);
    assert!(
        want > 100,
        "the kept prefix must self-match strongly: {want}"
    );

    for n in [
        DEFAULT_BOZORTH_MINUTIAE + 1,
        MAX_BOZORTH_MINUTIAE - 1,
        MAX_BOZORTH_MINUTIAE,
        MAX_BOZORTH_MINUTIAE + 1,
        300,
    ] {
        assert_eq!(
            match_score(&long[..n], &gallery),
            want,
            "{n} minutiae must score as the first {DEFAULT_BOZORTH_MINUTIAE} do; \
             MAX_BOZORTH_MINUTIAE ({MAX_BOZORTH_MINUTIAE}) is not a cap on anything"
        );
    }
}

#[test]
fn over_the_cap_the_first_150_are_kept_in_input_order() {
    // The cap keeps the first 150 *as given*: it is not the 150 best, nor a set. Moving the tail to
    // the front therefore hands `prepare` a different print, and the score collapses from a
    // self-match to a stranger's. This is why `src/xyt.rs:17-19` guarantees score parity only under
    // the cap, and why `tests/properties.rs` scopes permutation invariance to `len <= 150`.
    let long = print_of(6, 300);
    let gallery: Vec<Minutia> = long[..DEFAULT_BOZORTH_MINUTIAE].to_vec();
    let kept = match_score(&long[..DEFAULT_BOZORTH_MINUTIAE], &gallery);

    let mut tail_first = long.clone();
    tail_first.rotate_left(DEFAULT_BOZORTH_MINUTIAE);
    let dropped = match_score(&tail_first, &gallery);
    assert!(
        dropped < kept / 10,
        "the same 300 minutiae in a different order must present a different print: \
         kept {kept}, tail-first {dropped}"
    );
}

#[test]
fn a_full_turn_added_to_theta_changes_the_score() {
    // `prepare` normalizes theta with the reference's *single conditional subtract*, `t > 180 ?
    // t - 360 : t` — not a modulo. So `theta` and `theta - 360` are the same direction and **not the
    // same input**: the second lands outside `(-180, 180]`, the representation stage 1 is written
    // for, and the score moves.
    //
    // Recorded, not endorsed. `rem_euclid` here would be a one-line "fix" that silently moved every
    // score in `tests/golden.rs` away from the stock C tool, which is why `src/xyt.rs:12-15` calls
    // the conditional subtract out as a faithful detail rather than a bug.
    for seed in 1..5u64 {
        let base = print_of(seed, 40);
        let turned: Vec<Minutia> = base
            .iter()
            .map(|m| Minutia {
                theta: m.theta - 360,
                ..*m
            })
            .collect();
        assert_ne!(
            match_score(&turned, &base),
            match_score(&base, &base),
            "seed {seed}: a full turn is the same angle, so a folding normalizer would score \
             these identically — this crate's does not, and golden.rs depends on that"
        );
    }
}

#[test]
fn theta_outside_the_canonical_range_is_accepted() {
    // Nothing rejects a non-canonical angle: full turns either way, the quarter turns the
    // `Minutia::theta` doc names, and a theta orders of magnitude past a circle all run.
    //
    // Far-out angles are covered by `a_theta_beyond_46340_scores`; this list stays near a circle to
    // exercise the common non-canonical cases directly.
    let base = print_of(7, 40);
    for by in [-10_000, -720, -360, -270, 0, 270, 360, 720, 10_000] {
        let bumped: Vec<Minutia> = base
            .iter()
            .map(|m| Minutia {
                theta: m.theta + by,
                ..*m
            })
            .collect();
        let _ = match_score(&bumped, &base);
        let _ = match_score(&bumped, &bumped);
    }
}

/// A print 100000 pixels wide, with enough minutiae to pass the `MIN_COMPUTABLE` guard and reach
/// stage 1.
fn wide_print() -> Vec<Minutia> {
    let mut v = Vec::new();
    for x in [0, 100_000] {
        for i in 0..5 {
            v.push(Minutia {
                x,
                y: i * 10,
                theta: 0,
            });
        }
    }
    v
}

/// The squared distance is computed before the length guard (with the `dx > DM` break *inside*
/// it), so nothing bounds `dx` before it is squared. A print spanning 100000 pixels puts `dx` far
/// past the `46340` where an i32 `dx * dx` would overflow; the i64 multiply carries it, and
/// `match_score` returns a well-defined score.
///
/// Reached through the public seam, because that is where a caller meets it. Termination is the
/// whole claim — a guessed number would be a second oracle competing with `tests/golden.rs`.
#[test]
fn a_print_spanning_more_than_46340_pixels_scores() {
    let wide = wide_print();
    assert!(
        wide.len() >= MIN_COMPUTABLE_BOZORTH_MINUTIAE,
        "must reach stage 1"
    );
    let _ = match_score(&wide, &wide);
}

/// The width at which an i32 `dx * dx` would overflow (`46340² = 2_147_395_600 <= i32::MAX`, one
/// pixel more overflows) is not a boundary in behaviour: the i64 square carries both sides, so each
/// yields a well-defined score rather than a panic or wrap.
#[test]
fn a_print_at_the_i32_square_boundary_scores() {
    for span in [46_340, 46_341] {
        let mut v = Vec::new();
        for x in [0, span] {
            for i in 0..5 {
                v.push(Minutia {
                    x,
                    y: i * 10,
                    theta: 0,
                });
            }
        }
        let _ = match_score(&v, &v);
    }
}

/// The same squaring, one stage later and reached by an angle rather than a distance:
/// `src/inter.rs` computes `d * d` on the difference between a probe and a gallery beta.
///
/// `Minutia::theta` accepts any `i32` and normalizes only canonical input, so a far-out theta stays
/// far out; `iangle180` then folds it with a single ±360 step, which leaves a beta of roughly
/// `-theta`. Match that against a print with canonical angles and `d` is about `theta` — past the
/// ±46340 where an i32 `d * d` would overflow. The i64 square carries it, so `match_score` returns
/// a well-defined score.
#[test]
fn a_theta_beyond_46340_scores() {
    let base = print_of(7, 40);
    let far: Vec<Minutia> = base
        .iter()
        .map(|m| Minutia {
            theta: m.theta + 46_500,
            ..*m
        })
        .collect();
    let _ = match_score(&far, &base);
}
