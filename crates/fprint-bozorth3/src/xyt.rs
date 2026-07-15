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
