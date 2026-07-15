// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Stage 1: the intra-print comparison table (`bz_comp`) and its `bz_find` pruning.
//!
//! For a print whose minutiae are sorted by `(x, y)`, [`comp`] builds one row per compatible pair
//! of minutiae, then sorts the rows by `(distance, beta_min, beta_max)`. [`prune_len`] reproduces
//! `bz_find` + the `FDD` floor to give the pruned "Web" length passed to stage 2.

use crate::consts::{iangle180, sense_neg_pos, DM, DM_SQUARED, FD, FDD, TABLE_OVERFLOW_LIMIT};
use crate::xyt::Prepared;

/// A comparison-table row: `[distance, beta_min, beta_max, k+1, j+1, theta_kj (+400 if swapped)]`.
pub(crate) type CompRow = [i32; 6];

/// `bz_comp`: build and sort the pairwise comparison table for one print.
///
/// Faithful port: same pair-iteration order, opposite-angle skip, `dx > DM` inner-loop `break`,
/// f32 `atanf` edge angle widened to f64 for the `±0.5`/truncate rounding, and the `beta_k < beta_j`
/// row-orientation encoding with the `+400` swap flag. The reference keeps the table sorted by an
/// insertion sort keyed on the first three columns; a stable sort on `(0,1,2)` yields the identical
/// order (ties preserve pair-generation order).
pub(crate) fn comp(p: &Prepared) -> Vec<CompRow> {
    let n = p.nrows;
    let mut rows: Vec<CompRow> = Vec::new();

    'outer: for k in 0..n.saturating_sub(1) {
        let tk = p.theta[k];
        for j in (k + 1)..n {
            let tj = p.theta[j];

            // Opposite-angle skip (edges between anti-parallel ridges carry no useful geometry).
            if tj > 0 {
                if tk == tj - 180 {
                    continue;
                }
            } else if tk == tj + 180 {
                continue;
            }

            let dx = p.x[j] - p.x[k];
            let dy = p.y[j] - p.y[k];
            let distance = dx * dx + dy * dy;
            if distance > DM_SQUARED {
                // The list is x-sorted, so once dx exceeds DM every later j is farther still.
                if dx > DM {
                    break;
                }
                continue;
            }

            let theta_kj = if dx == 0 {
                90
            } else {
                // (180/PI) * atanf(dy/dx) in f32 (default, non-m1 sign), then widen to f64 for the
                // ±0.5 bias and truncate-toward-zero cast — exactly the reference's type ladder.
                let prod: f32 =
                    (180.0_f32 / core::f32::consts::PI) * ((dy as f32) / (dx as f32)).atan();
                let mut dz = prod as f64;
                if dz < 0.0 {
                    dz -= 0.5;
                } else {
                    dz += 0.5;
                }
                dz as i32
            };

            let beta_k = iangle180(theta_kj - tk);
            let beta_j = iangle180(theta_kj - tj + 180);

            let ki = (k + 1) as i32;
            let ji = (j + 1) as i32;
            let row: CompRow = if beta_k < beta_j {
                [distance, beta_k, beta_j, ki, ji, theta_kj]
            } else {
                [distance, beta_j, beta_k, ki, ji, theta_kj + 400]
            };
            rows.push(row);

            if rows.len() == TABLE_OVERFLOW_LIMIT {
                break 'outer;
            }
        }
    }

    rows.sort_by(|a, b| (a[0], a[1], a[2]).cmp(&(b[0], b[1], b[2])));
    rows
}

/// `bz_find` + `FDD` floor: the pruned Web length for a sorted comparison table.
///
/// `bz_find` binary-searches the distance-sorted rows for the `FD` boundary (edges longer than
/// `√FD = 75` are dropped); the `FDD` floor then guarantees at least `min(sim, 500)` edges survive.
pub(crate) fn prune_len(sorted: &[CompRow]) -> usize {
    let sim = sorted.len() as i32;

    // bz_find (binary search for FD in the ascending distance column).
    let mut bottom = 0;
    let mut top = sim + 1;
    let mut midpoint = 1;
    let mut state = -1;
    while top - bottom > 1 {
        midpoint = (bottom + top) / 2;
        let distance = sorted[(midpoint - 1) as usize][0];
        state = sense_neg_pos(FD, distance);
        if state < 0 {
            top = midpoint;
        } else {
            bottom = midpoint;
        }
    }
    if state > -1 {
        midpoint += 1;
    }
    let mut msim = sim;
    if midpoint < msim {
        msim = midpoint;
    }

    // FDD floor: keep a reasonable number of edges when the Web is bigger than the pruned point.
    if msim < FDD {
        msim = if sim > FDD { FDD } else { sim };
    }
    msim as usize
}
