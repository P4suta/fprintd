// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Input normalization: cap the minutia count, then sort by `(x, y)` ascending.
//!
//! This mirrors stock NBIS `bz_load` / `bz_prune` (`bz_io.c`) + `sort_x_y` (`bz_sort.c`) for the
//! no-quality case: normalize theta, keep up to `max` minutiae, then sort by increasing `x`, ties
//! by increasing `y`. The sort is **load-bearing** — stage 1's `dx > DM` early `break` relies on
//! x-ascending order.
//!
//! Two faithful details that a "modulo/stable-sort" shortcut would get wrong:
//! * **Theta** is normalized by a *single conditional subtract* `(t > 180) ? t - 360 : t`, not a
//!   full modulo — for canonical input `t ∈ 0..=359` the result lands in `(-180, 180]`, which is the
//!   representation `bz_comp` expects.
//! * The reference's over-cap trim keeps the top-`max` minutiae by a custom quality quicksort whose
//!   tie order we do **not** reproduce; we therefore only guarantee score parity for prints with
//!   **≤ `max` (150)** minutiae (true for all `Template::Nbis` samples and our fixtures). Above the
//!   cap we keep the first `max` in input order and score parity is not guaranteed.

use crate::Minutia;

/// A print reduced to the three parallel integer columns stage 1 consumes.
pub(crate) struct Prepared {
    /// Number of retained minutiae (`= min(input.len(), max)`).
    pub nrows: usize,
    pub x: Vec<i32>,
    pub y: Vec<i32>,
    /// Ridge angle in integer degrees, normalized to `(-180, 180]`.
    pub theta: Vec<i32>,
}

/// Cap to `max` minutiae (input order), normalize theta, then sort by `(x, y)` ascending.
///
/// Theta uses the reference's single conditional subtract `(t > 180) ? t - 360 : t` (see the module
/// docs) — **not** a modulo — so canonical `0..=359` input maps to `(-180, 180]`.
pub(crate) fn prepare(minutiae: &[Minutia], max: usize) -> Prepared {
    let mut rows: Vec<(i32, i32, i32)> = minutiae
        .iter()
        .take(max)
        .map(|m| {
            let t = if m.theta > 180 {
                m.theta - 360
            } else {
                m.theta
            };
            (m.x, m.y, t)
        })
        .collect();

    // Stable sort by (x, y); the final tie-break on original position keeps it deterministic where
    // the reference's qsort would be unspecified (two minutiae at the same pixel — degenerate).
    rows.sort_by_key(|a| (a.0, a.1));

    let nrows = rows.len();
    let mut x = Vec::with_capacity(nrows);
    let mut y = Vec::with_capacity(nrows);
    let mut theta = Vec::with_capacity(nrows);
    for (mx, my, mt) in rows {
        x.push(mx);
        y.push(my);
        theta.push(mt);
    }
    Prepared { nrows, x, y, theta }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::DEFAULT_BOZORTH_MINUTIAE;

    fn m(x: i32, y: i32, theta: i32) -> Minutia {
        Minutia { x, y, theta }
    }

    /// The `(x, y)` columns as pairs, for asserting order without three parallel indexes.
    fn xy(p: &Prepared) -> Vec<(i32, i32)> {
        p.x.iter().copied().zip(p.y.iter().copied()).collect()
    }

    #[test]
    fn prepare_sorts_by_x_then_y() {
        let p = prepare(&[m(3, 1, 0), m(1, 9, 0), m(3, 0, 0), m(1, 2, 0)], 150);
        assert_eq!(xy(&p), [(1, 2), (1, 9), (3, 0), (3, 1)]);
    }

    #[test]
    fn prepare_theta_is_single_conditional_subtract() {
        // Canonical input (`0..=359`) lands in `(-180, 180]` — what stage 1 expects.
        for (input, want) in [
            (0, 0),
            (1, 1),
            (180, 180),
            (181, -179),
            (270, -90),
            (359, -1),
        ] {
            let p = prepare(&[m(0, 0, input)], 150);
            assert_eq!(p.theta[0], want, "canonical theta {input}");
        }
        // Non-canonical input is not folded: one subtract happens, or none. These are the values
        // that make "any integer is folded into 0..=359" false, and they are the reference's
        // behaviour, not ours — a `rem_euclid` "fix" here would move every score.
        for (input, want) in [(360, 0), (400, 40), (-90, -90), (720, 360), (-270, -270)] {
            let p = prepare(&[m(0, 0, input)], 150);
            assert_eq!(p.theta[0], want, "non-canonical theta {input}");
        }
    }

    #[test]
    fn prepare_caps_at_max_in_input_order() {
        // 200 minutiae at descending x, so input order and sorted order disagree everywhere. The
        // cap keeps the first `max` *as given*, then sorts; it does not keep the 150 smallest x.
        let input: Vec<Minutia> = (0..200).map(|i| m(200 - i, 0, 0)).collect();
        let p = prepare(&input, DEFAULT_BOZORTH_MINUTIAE);
        assert_eq!(p.nrows, DEFAULT_BOZORTH_MINUTIAE);
        // The first 150 given were x = 200 down to 51; sorting puts 51 first and 200 last.
        assert_eq!(p.x.first(), Some(&51));
        assert_eq!(p.x.last(), Some(&200));
    }

    #[test]
    fn prepare_is_stable_for_duplicate_xy() {
        // Same pixel, different theta: the reference's qsort leaves this order unspecified, so we
        // pin ours. Input order decides.
        let p = prepare(&[m(5, 5, 10), m(5, 5, 20), m(5, 5, 30)], 150);
        assert_eq!(p.theta, [10, 20, 30]);
    }

    #[test]
    fn prepare_keeps_columns_row_aligned() {
        // Three parallel Vecs are built from one row list; a reorder that touched one and not the
        // others would still sort correctly and still be wrong.
        let p = prepare(&[m(9, 90, 9), m(1, 10, 1), m(5, 50, 5)], 150);
        for i in 0..p.nrows {
            assert_eq!(p.y[i], p.x[i] * 10, "row {i} lost its y");
            assert_eq!(p.theta[i], p.x[i], "row {i} lost its theta");
        }
    }

    #[test]
    fn prepare_empty_and_single() {
        let empty = prepare(&[], 150);
        assert_eq!(empty.nrows, 0);
        assert!(empty.x.is_empty() && empty.y.is_empty() && empty.theta.is_empty());

        let one = prepare(&[m(7, 8, 9)], 150);
        assert_eq!((one.nrows, one.x[0], one.y[0], one.theta[0]), (1, 7, 8, 9));
    }
}
