// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Constants and small integer/float helpers, taken verbatim from stock NBIS `bozorth.h`.
//!
//! Every value here is an **interoperability fact** copied from the public-domain reference; the
//! comments cite the reference macro. See `docs/bozorth3-algorithm.md`.

/// `DM` — maximum inter-minutia edge length. The stored distance is `dx² + dy² ∈ [0, DM²]`.
pub(crate) const DM: i32 = 125;
/// `SQUARED(DM)` — the distance cap actually compared against (`125² = 15625`).
pub(crate) const DM_SQUARED: i32 = DM * DM;

/// `FD` — the fixed squared-distance (`= 75²`) at which `bz_find` prunes each Web.
pub(crate) const FD: i32 = 5625;
/// `FDD` — minimum pruned edge count kept per Web when enough edges exist.
pub(crate) const FDD: i32 = 500;

/// `TK` — fractional distance-agreement tolerance (single precision, on purpose).
pub(crate) const TK: f32 = 0.05;

/// `TXS` — squared angle-tolerance low bound (`= 11²`).
pub(crate) const TXS: i32 = 121;
/// `CTXS` — squared angle-tolerance high bound (`= 349²`), the near-360° wrap allowance.
pub(crate) const CTXS: i32 = 121_801;

/// `MSTR` — minimum edge-pairs for a path to become a cluster.
pub(crate) const MSTR: i32 = 3;
/// `MMSTR` — score below which `bz_final_loop` is skipped.
pub(crate) const MMSTR: i32 = 8;
/// `WWIM` — cap on the number of endpoint-conflict groups (`*ww`).
pub(crate) const WWIM: i32 = 10;

/// `QQ_SIZE` — capacity of the `qq[]` work queue.
pub(crate) const QQ_SIZE: i32 = 4000;
/// `QQ_OVERFLOW_SCORE` — sentinel score returned on `qq[]` overflow (`= QQ_SIZE`).
pub(crate) const QQ_OVERFLOW_SCORE: i32 = QQ_SIZE;
/// `ZERO_MATCH_SCORE` — returned when a print has too few minutiae.
pub(crate) const ZERO_MATCH_SCORE: i32 = 0;

/// `MAX_BOZORTH_MINUTIAE` — hard cap on minutiae per print.
pub const MAX_BOZORTH_MINUTIAE: usize = 200;
/// `DEFAULT_BOZORTH_MINUTIAE` — default cap (`max_minutiae`).
pub const DEFAULT_BOZORTH_MINUTIAE: usize = 150;
/// `MIN_COMPUTABLE_BOZORTH_MINUTIAE` — below this (either print) the score is `0`.
///
/// `0` here means "not computable", not "no resemblance": too few minutiae to decide either way.
/// A caller that treats the score as a similarity must check the count first, or it will read every
/// under-sized capture as a confident non-match.
///
/// ```
/// use fprint_bozorth3::{match_score, Minutia, MIN_COMPUTABLE_BOZORTH_MINUTIAE};
///
/// // Two prints that agree perfectly — and are one minutia short of computable.
/// let a: Vec<Minutia> = (0..MIN_COMPUTABLE_BOZORTH_MINUTIAE - 1)
///     .map(|i| Minutia { x: 20 + i as i32 * 9, y: 30, theta: 0 })
///     .collect();
/// assert_eq!(match_score(&a, &a), 0);
/// ```
pub const MIN_COMPUTABLE_BOZORTH_MINUTIAE: usize = 10;

// The caps' ordering, stated to the compiler rather than to a test: a print cannot be required to
// carry more minutiae than it is allowed to keep. A build is the only place this can be wrong.
const _: () = assert!(MIN_COMPUTABLE_BOZORTH_MINUTIAE <= DEFAULT_BOZORTH_MINUTIAE);
const _: () = assert!(DEFAULT_BOZORTH_MINUTIAE <= MAX_BOZORTH_MINUTIAE);

/// The comparison/compatibility tables stop one short of their 20000-row capacity.
pub(crate) const TABLE_OVERFLOW_LIMIT: usize = 19_999;

/// `IANGLE180(deg)` — fold a degree value into `(-180, 180]` with a single ±360 step.
///
/// Correct only for `deg ∈ (-540, 540]`, exactly as the reference macro; all call sites feed it a
/// value already bounded to `|deg| ≤ 360`.
#[inline]
pub(crate) const fn iangle180(deg: i32) -> i32 {
    if deg > 180 {
        deg - 360
    } else if deg <= -180 {
        deg + 360
    } else {
        deg
    }
}

/// `SENSE(a,b)` — the integer three-way comparison `-1 / 0 / +1`.
#[inline]
pub(crate) const fn sense(a: i32, b: i32) -> i32 {
    if a < b {
        -1
    } else if a == b {
        0
    } else {
        1
    }
}

/// `SENSE_NEG_POS(a,b)` — like [`sense`] but collapses the equal case to `+1` (never `0`).
#[inline]
pub(crate) const fn sense_neg_pos(a: i32, b: i32) -> i32 {
    if a < b {
        -1
    } else {
        1
    }
}

/// `ROUND(f)` (default, non-library form): round half away from zero by biasing ±0.5 in `f32`
/// then truncating toward zero (the C cast). Single precision throughout — see the doc's
/// bit-exactness notes.
#[inline]
pub(crate) fn round_half_away(f: f32) -> i32 {
    if f < 0.0 {
        (f - 0.5) as i32
    } else {
        (f + 0.5) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every value against `bozorth.h`, as tabulated in `docs/bozorth3-algorithm.md`. These are
    /// interoperability facts: a changed value here is a changed score, and the golden suite would
    /// say so only as a number, without saying which constant moved.
    #[test]
    fn constants_match_reference_header() {
        assert_eq!(DM, 125, "DM");
        assert_eq!(FD, 5625, "FD");
        assert_eq!(FDD, 500, "FDD");
        assert_eq!(TK, 0.05, "TK");
        assert_eq!(TXS, 121, "TXS");
        assert_eq!(CTXS, 121_801, "CTXS");
        assert_eq!(MSTR, 3, "MSTR");
        assert_eq!(MMSTR, 8, "MMSTR");
        assert_eq!(WWIM, 10, "WWIM");
        assert_eq!(QQ_SIZE, 4000, "QQ_SIZE");
        assert_eq!(ZERO_MATCH_SCORE, 0, "ZERO_MATCH_SCORE");
        assert_eq!(MAX_BOZORTH_MINUTIAE, 200, "MAX_BOZORTH_MINUTIAE");
        assert_eq!(DEFAULT_BOZORTH_MINUTIAE, 150, "DEFAULT_BOZORTH_MINUTIAE");
        assert_eq!(
            MIN_COMPUTABLE_BOZORTH_MINUTIAE, 10,
            "MIN_COMPUTABLE_BOZORTH_MINUTIAE"
        );
        assert_eq!(TABLE_OVERFLOW_LIMIT, 19_999, "TABLE_OVERFLOW_LIMIT");
    }

    /// The roots the comments above name. Each is a `SQUARED()` in the reference, so a value
    /// edited without its root — or vice versa — is caught here rather than in a score.
    ///
    /// The caps' ordering is not here: it is a `const _: () = assert!(..)` above, where the
    /// compiler holds it.
    #[test]
    fn derived_constants_are_consistent() {
        assert_eq!(DM_SQUARED, 15_625);
        assert_eq!(FD, 75 * 75);
        assert_eq!(TXS, 11 * 11);
        assert_eq!(CTXS, 349 * 349);
        assert_eq!(QQ_OVERFLOW_SCORE, QQ_SIZE);
    }

    /// Exhaustive over the domain the reference macro is correct on. A total statement, so it is
    /// worth more than samples: for every input, the result is in `(-180, 180]` and congruent to
    /// the input mod 360.
    #[test]
    fn iangle180_folds_into_half_open_180() {
        for deg in -539..=540 {
            let r = iangle180(deg);
            assert!(
                r > -180 && r <= 180,
                "iangle180({deg}) = {r}, outside (-180, 180]"
            );
            assert_eq!(
                r.rem_euclid(360),
                deg.rem_euclid(360),
                "iangle180({deg}) = {r} is not the same angle"
            );
        }
        // The boundaries the half-open range turns on: 180 stays, 181 wraps; -180 wraps, -179
        // stays. An off-by-one in either comparison survives every sample that avoids them.
        assert_eq!(iangle180(180), 180);
        assert_eq!(iangle180(181), -179);
        assert_eq!(iangle180(-180), 180);
        assert_eq!(iangle180(-179), -179);
    }

    /// `SENSE` and `SENSE_NEG_POS` differ on exactly one input: equality.
    #[test]
    fn sense_and_sense_neg_pos_agree_except_on_equality() {
        for a in -3..=3 {
            for b in -3..=3 {
                assert_ne!(
                    sense_neg_pos(a, b),
                    0,
                    "sense_neg_pos({a},{b}) must never be 0"
                );
                if a == b {
                    assert_eq!(sense(a, b), 0);
                    assert_eq!(sense_neg_pos(a, b), 1);
                } else {
                    assert_eq!(sense(a, b), sense_neg_pos(a, b), "sense({a},{b})");
                }
            }
        }
    }

    /// `ROUND` is round-half-away-from-zero, **not** Rust's `f32::round_ties_even`, and its cast
    /// saturates where C's would be undefined.
    #[test]
    fn round_half_away_rounds_ties_away_from_zero() {
        for (f, want) in [
            (0.5_f32, 1),
            (-0.5, -1),
            (1.5, 2),
            (-1.5, -2),
            (2.5, 3),
            (-2.5, -3),
            (0.4, 0),
            (-0.4, 0),
        ] {
            assert_eq!(round_half_away(f), want, "round_half_away({f})");
        }
        // 2.5 is the witness: ties-even would give 2. Reaching for `f.round_ties_even() as i32`
        // here would move every theta_kj that lands on a half.
        assert_eq!(round_half_away(2.5), 3);
        assert_ne!(round_half_away(2.5), 2.5_f32.round_ties_even() as i32);
        // Rust's float-to-int cast saturates; C's is undefined for out-of-range values. A
        // deliberate divergence: the reference never feeds it one.
        assert_eq!(round_half_away(f32::MAX), i32::MAX);
        assert_eq!(round_half_away(f32::MIN), i32::MIN);
    }
}
