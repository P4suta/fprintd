// SPDX-FileCopyrightText: NIST NBIS — U.S. Government work, public domain (title 17 §105)
//
// SPDX-License-Identifier: LicenseRef-NBIS-PD

//! Constants and small integer/float helpers, taken verbatim from stock NBIS `bozorth.h`.
//!
//! Every value here is an **interoperability fact** copied from the public-domain reference; the
//! comments cite the reference macro. See `docs/bozorth3-algorithm.md`.

/// `DM` — maximum inter-minutia edge length. The stored distance is `dx² + dy² ∈ [0, DM²]`.
pub const DM: i32 = 125;
/// `SQUARED(DM)` — the distance cap actually compared against (`125² = 15625`).
pub const DM_SQUARED: i32 = DM * DM;

/// `FD` — the fixed squared-distance (`= 75²`) at which `bz_find` prunes each Web.
pub const FD: i32 = 5625;
/// `FDD` — minimum pruned edge count kept per Web when enough edges exist.
pub const FDD: i32 = 500;

/// `TK` — fractional distance-agreement tolerance (single precision, on purpose).
pub const TK: f32 = 0.05;

/// `TXS` — squared angle-tolerance low bound (`= 11²`).
pub const TXS: i32 = 121;
/// `CTXS` — squared angle-tolerance high bound (`= 349²`), the near-360° wrap allowance.
pub const CTXS: i32 = 121_801;

/// `MSTR` — minimum edge-pairs for a path to become a cluster.
pub const MSTR: i32 = 3;
/// `MMSTR` — score below which `bz_final_loop` is skipped.
pub const MMSTR: i32 = 8;
/// `WWIM` — cap on the number of endpoint-conflict groups (`*ww`).
pub const WWIM: i32 = 10;

/// `QQ_SIZE` — capacity of the `qq[]` work queue.
pub const QQ_SIZE: i32 = 4000;
/// `QQ_OVERFLOW_SCORE` — sentinel score returned on `qq[]` overflow (`= QQ_SIZE`).
pub const QQ_OVERFLOW_SCORE: i32 = QQ_SIZE;
/// `ZERO_MATCH_SCORE` — returned when a print has too few minutiae.
pub const ZERO_MATCH_SCORE: i32 = 0;

/// `MAX_BOZORTH_MINUTIAE` — hard cap on minutiae per print.
pub const MAX_BOZORTH_MINUTIAE: usize = 200;
/// `DEFAULT_BOZORTH_MINUTIAE` — default cap (`max_minutiae`).
pub const DEFAULT_BOZORTH_MINUTIAE: usize = 150;
/// `MIN_COMPUTABLE_BOZORTH_MINUTIAE` — below this (either print) the score is `0`.
pub const MIN_COMPUTABLE_BOZORTH_MINUTIAE: usize = 10;

/// The comparison/compatibility tables stop one short of their 20000-row capacity.
pub const TABLE_OVERFLOW_LIMIT: usize = 19_999;

/// `IANGLE180(deg)` — fold a degree value into `(-180, 180]` with a single ±360 step.
///
/// Correct only for `deg ∈ (-540, 540]`, exactly as the reference macro; all call sites feed it a
/// value already bounded to `|deg| ≤ 360`.
#[inline]
pub const fn iangle180(deg: i32) -> i32 {
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
pub const fn sense(a: i32, b: i32) -> i32 {
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
pub const fn sense_neg_pos(a: i32, b: i32) -> i32 {
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
pub fn round_half_away(f: f32) -> i32 {
    if f < 0.0 {
        (f - 0.5) as i32
    } else {
        (f + 0.5) as i32
    }
}
