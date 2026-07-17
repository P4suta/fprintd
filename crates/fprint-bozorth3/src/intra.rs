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

            // `wrapping_sub`, matching the reference's `int` subtraction: for in-contract image
            // coordinates it never wraps, and for an out-of-contract extreme (`match_score` accepts
            // any `i32` xyt) it wraps exactly as the C does rather than panicking under
            // overflow-checks — upholding the crate's "never panics on any input" contract.
            let dx = p.x[j].wrapping_sub(p.x[k]);
            let dy = p.y[j].wrapping_sub(p.y[k]);
            // Squared in i64 so an arbitrarily large separation cannot overflow the multiply; the
            // guard below rejects anything past DM_SQUARED, so a kept edge fits i32 losslessly.
            let distance = i64::from(dx) * i64::from(dx) + i64::from(dy) * i64::from(dy);
            if distance > i64::from(DM_SQUARED) {
                // The list is x-sorted, so once dx exceeds DM every later j is farther still.
                if dx > DM {
                    break;
                }
                continue;
            }
            let distance = distance as i32;

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

            // `wrapping_*`, matching the reference's `int` angle arithmetic: `tk`/`tj` are the raw
            // input thetas, which `match_score` accepts across the whole `i32` range. In-contract
            // thetas (`0..360`) never wrap, so scores are unchanged; an extreme theta wraps as the C
            // does instead of panicking under overflow-checks. `iangle180` folds any `i32` safely.
            let beta_k = iangle180(theta_kj.wrapping_sub(tk));
            let beta_j = iangle180(theta_kj.wrapping_sub(tj).wrapping_add(180));

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

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-built [`Prepared`]. Callers here supply the columns directly so a test can state an
    /// unsorted print, which `xyt::prepare` cannot produce — see
    /// [`comp_breaks_the_inner_loop_once_dx_exceeds_dm`].
    fn prepared(x: &[i32], y: &[i32], theta: &[i32]) -> Prepared {
        assert_eq!(x.len(), y.len());
        assert_eq!(x.len(), theta.len());
        Prepared {
            nrows: x.len(),
            x: x.to_vec(),
            y: y.to_vec(),
            theta: theta.to_vec(),
        }
    }

    /// `sorted`-shaped rows carrying only a distance; the other columns do not affect `prune_len`.
    fn web(distances: &[i32]) -> Vec<CompRow> {
        distances.iter().map(|&d| [d, 0, 0, 1, 2, 0]).collect()
    }

    #[test]
    fn comp_skips_opposite_angles() {
        // tj > 0: skipped when tk == tj - 180.
        let skipped = comp(&prepared(&[0, 10], &[0, 0], &[-90, 90]));
        assert!(skipped.is_empty(), "anti-parallel pair must carry no edge");
        // tj <= 0: the other arm, skipped when tk == tj + 180.
        let skipped = comp(&prepared(&[0, 10], &[0, 0], &[90, -90]));
        assert!(skipped.is_empty(), "the tj <= 0 arm must skip too");
        // One degree off anti-parallel is not a skip, so the arms above are doing the work and not
        // something else about this geometry.
        let kept = comp(&prepared(&[0, 10], &[0, 0], &[-89, 90]));
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn comp_theta_kj_is_90_for_vertical_edge() {
        // dx == 0 takes the branch that never calls atan.
        let rows = comp(&prepared(&[7, 7], &[0, 10], &[0, 0]));
        assert_eq!(rows.len(), 1);
        // With both ridges at 0, beta_k = iangle180(90) = 90 and beta_j = iangle180(270) = -90, so
        // `beta_k < beta_j` is false and the row is emitted swapped — column 5 carries
        // theta_kj + 400. A vertical edge is therefore always a swapped row.
        assert_eq!((rows[0][1], rows[0][2]), (-90, 90), "betas, min before max");
        assert_eq!(rows[0][5], 490, "theta_kj = 90, plus the swap flag");
    }

    #[test]
    fn comp_keeps_edge_at_exactly_dm_squared() {
        // `distance > DM_SQUARED` drops, so the boundary itself is kept. dx = DM exactly, and
        // `dx > DM` is likewise false, so no break either.
        let kept = comp(&prepared(&[0, DM], &[0, 0], &[0, 0]));
        assert_eq!(kept.len(), 1, "distance == DM_SQUARED must be kept");
        assert_eq!(kept[0][0], DM_SQUARED);
        // One further out is dropped, and `dx > DM` is still false, so this is the distance guard
        // and not the break.
        let dropped = comp(&prepared(&[0, DM], &[0, 1], &[0, 0]));
        assert!(dropped.is_empty(), "distance == DM_SQUARED + 1 must drop");
    }

    #[test]
    fn comp_breaks_the_inner_loop_once_dx_exceeds_dm() {
        // The same three minutiae, sorted and unsorted. Sorted, the near pair (0, 10) is found.
        let sorted = comp(&prepared(&[0, 10, 200], &[0; 3], &[0; 3]));
        assert_eq!(sorted.len(), 1, "the near pair is one edge");

        // Unsorted, x = [0, 200, 10]: at k = 0 the pair (0, 200) has dx = 200 > DM, so the loop
        // breaks and never reaches x = 10 — an edge the sorted print does find. This is what
        // `xyt`'s "the sort is load-bearing" means, and it is only observable from inside the
        // crate, because `xyt::prepare` cannot hand `comp` an unsorted print.
        let unsorted = comp(&prepared(&[0, 200, 10], &[0; 3], &[0; 3]));
        assert!(
            unsorted.is_empty(),
            "the break must skip the rest of the row, losing the near pair"
        );
    }

    #[test]
    fn comp_ids_are_one_based_and_unswapped_rows_keep_theta_kj() {
        let rows = comp(&prepared(&[0, 10], &[0, 0], &[0, 0]));
        assert_eq!(rows.len(), 1);
        let [distance, beta_min, beta_max, ki, ji, theta_kj] = rows[0];
        assert_eq!(distance, 100);
        // theta_kj = ROUND(180/pi * atan(0/10)) = 0; beta_k = 0, beta_j = iangle180(180) = 180.
        assert_eq!((beta_min, beta_max), (0, 180));
        assert_eq!((ki, ji), (1, 2), "minutia ids are 1-based");
        assert_eq!(theta_kj, 0, "an unswapped row carries theta_kj as-is");
    }

    #[test]
    fn comp_encodes_swap_with_plus_400() {
        // theta_kj = 0, so beta_k = iangle180(170) = 170 and beta_j = iangle180(160) = 160.
        // beta_k < beta_j is false, so the row is emitted swapped.
        let rows = comp(&prepared(&[0, 10], &[0, 0], &[-170, 20]));
        assert_eq!(rows.len(), 1);
        let [_, beta_min, beta_max, _, _, theta_kj] = rows[0];
        assert_eq!(
            (beta_min, beta_max),
            (160, 170),
            "a swapped row still orders beta_min before beta_max"
        );
        assert_eq!(theta_kj, 400, "the swap flag is theta_kj + 400");
    }

    #[test]
    fn comp_rows_sorted_by_distance_beta_min_beta_max() {
        let rows = comp(&prepared(&[0, 3, 40, 90], &[0, 4, 0, 0], &[0, 0, 0, 0]));
        assert!(rows.len() > 1, "need several rows to observe an order");
        let keys: Vec<_> = rows.iter().map(|r| (r[0], r[1], r[2])).collect();
        let mut want = keys.clone();
        want.sort();
        assert_eq!(
            keys, want,
            "rows must be ascending by (distance, beta_min, beta_max)"
        );
    }

    /// The squared distance is computed before the length guard, and the `dx > DM` break sits
    /// *inside* that guard — so nothing bounds `dx` before it is squared. The i64 multiply carries
    /// any separation, including the point where an i32 `dx * dx` would overflow (`46340² =
    /// 2_147_395_600 <= i32::MAX`) and one pixel past it, without panic or wrap: each is a far pair
    /// the guard drops.
    #[test]
    fn comp_handles_a_separation_that_would_overflow_an_i32_square() {
        for dx in [46_340, 46_341, i32::MAX] {
            let rows = comp(&prepared(&[0, dx], &[0, 0], &[0, 0]));
            assert!(
                rows.is_empty(),
                "dx = {dx} is far apart, so the edge is dropped"
            );
        }
    }

    #[test]
    fn prune_len_empty_is_zero() {
        // The binary search never indexes, so an empty Web is a length rather than a panic.
        assert_eq!(prune_len(&[]), 0);
    }

    #[test]
    fn prune_len_never_exceeds_input_len() {
        for sim in [0usize, 1, 2, 9, 10, 499, 500, 501, 1200] {
            for distance in [0, FD - 1, FD, FD + 1, DM_SQUARED, 100_000] {
                let rows = web(&vec![distance; sim]);
                let len = prune_len(&rows);
                assert!(
                    len <= sim,
                    "prune_len({sim} rows at distance {distance}) = {len}, longer than its input"
                );
            }
        }
    }

    #[test]
    fn prune_len_floors_at_fdd_so_a_small_web_is_never_pruned() {
        // Every edge is far longer than FD, yet a Web at or under the floor keeps all of it: the
        // FDD floor, not the search, decides here.
        for sim in [1usize, 10, FDD as usize] {
            let rows = web(&vec![100_000; sim]);
            assert_eq!(prune_len(&rows), sim, "a Web of {sim} must survive whole");
        }
    }

    #[test]
    fn prune_len_keeps_edges_at_exactly_fd() {
        // Above the floor, the search decides — and `sense_neg_pos(FD, distance)` collapses
        // equality to +1, so an edge at exactly FD counts as short and is kept.
        let all_at_fd = web(&vec![FD; 1200]);
        assert_eq!(prune_len(&all_at_fd), 1200, "edges at exactly FD are short");

        let all_past_fd = web(&vec![FD + 1; 1200]);
        assert_eq!(
            prune_len(&all_past_fd),
            FDD as usize,
            "edges past FD prune down to the floor"
        );
    }
}
