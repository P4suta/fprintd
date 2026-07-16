// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Stage 2: the inter-print compatibility table (`bz_match`).
//!
//! Walk the two sorted stage-1 Webs and emit one **compatible edge pair** per (probe row, gallery
//! row) that agrees in edge length (the `TK` distance band, in f32) and in both relative angles
//! (the `TXS`/`CTXS` integer band). Each output row is
//! `[ΔθKJ, probe_k, probe_j, gallery_a, gallery_b]`, sorted by `(probe_k, gallery_a, probe_j)`.

use crate::consts::{iangle180, CTXS, TABLE_OVERFLOW_LIMIT, TK, TXS};
use crate::intra::CompRow;

/// A compatibility-table row: `[delta_theta, probe_k, probe_j, gallery_a, gallery_b]`.
pub(crate) type ColpRow = [i32; 5];

/// `bz_match`: build the sorted compatibility list from the two pruned Webs.
///
/// `probe` and `gallery` are the sorted stage-1 tables already truncated to their pruned lengths
/// (`prune_len`), so their slice lengths are the `*_ptrlist_len` values. The persistent `st` cursor
/// and the asymmetric loop bounds (`k` up to `probe.len()-1`, `j` up to `gallery.len()`) are
/// reproduced exactly.
pub(crate) fn bz_match(probe: &[CompRow], gallery: &[CompRow]) -> Vec<ColpRow> {
    let probe_len = probe.len();
    let gallery_len = gallery.len();
    let mut rot: Vec<ColpRow> = Vec::new();
    let mut st: usize = 1;

    'k: for k in 1..probe_len {
        let ss = &probe[k - 1];
        let mut j = st;
        while j <= gallery_len {
            let ff = &gallery[j - 1];

            // --- distance-agreement test (the sole f32 decision in stage 2) ---
            let dz = (ff[0] - ss[0]) as f32;
            let fi = (2.0_f32 * TK) * ((ff[0] + ss[0]) as f32);
            if dz * dz > fi * fi {
                if dz < 0.0 {
                    // Gallery edge shorter than probe edge: advance the persistent start cursor.
                    st = j + 1;
                    j += 1;
                    continue;
                }
                // Gallery edge longer and out of band: no later (longer) j can match.
                break;
            }

            // --- relative-angle test (integer, exact) ---
            let mut incompatible = false;
            for i in 1..3usize {
                let d = ss[i] - ff[i];
                let d2 = d * d;
                if d2 > TXS && d2 < CTXS {
                    incompatible = true;
                    break;
                }
            }
            if incompatible {
                j += 1;
                continue;
            }

            // --- relative rotation (decode the +400 swap flags, then IANGLE180 the difference) ---
            let (mut p1, n) = if ss[5] >= 220 {
                (ss[5] - 580, 1)
            } else {
                (ss[5], 0)
            };
            let (p2, b) = if ff[5] >= 220 {
                (ff[5] - 580, 1)
            } else {
                (ff[5], 0)
            };
            p1 -= p2;
            p1 = iangle180(p1);

            // --- endpoint pairing (flip the gallery pair when the swap flags differ) ---
            let row: ColpRow = if n != b {
                [p1, ss[3], ss[4], ff[4], ff[3]]
            } else {
                [p1, ss[3], ss[4], ff[3], ff[4]]
            };
            rot.push(row);
            if rot.len() == TABLE_OVERFLOW_LIMIT {
                break 'k;
            }
            j += 1;
        }
    }

    // Sort key precedence {1, 3, 2}: probe_k, gallery_a, probe_j. Stable → equal keys keep
    // generation order, matching the reference's insertion sort.
    rot.sort_by(|a, b| (a[1], a[3], a[2]).cmp(&(b[1], b[3], b[2])));
    rot
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One stage-1 row: `[distance, beta_min, beta_max, k+1, j+1, theta_kj]`.
    fn row(distance: i32, betas: (i32, i32), ids: (i32, i32), theta_kj: i32) -> CompRow {
        [distance, betas.0, betas.1, ids.0, ids.1, theta_kj]
    }

    /// A row that agrees with any other `plain` row of the same distance: betas equal, no swap.
    fn plain(distance: i32, ids: (i32, i32)) -> CompRow {
        row(distance, (0, 0), ids, 0)
    }

    #[test]
    fn bz_match_empty_and_single_row_webs() {
        // `1..probe_len` is empty below two rows, so there is nothing to pair and nothing to index.
        assert!(bz_match(&[], &[]).is_empty());
        assert!(bz_match(&[], &[plain(100, (1, 2))]).is_empty());
        assert!(bz_match(&[plain(100, (1, 2))], &[]).is_empty());
        assert!(bz_match(&[plain(100, (1, 2))], &[plain(100, (3, 4))]).is_empty());
    }

    #[test]
    fn bz_match_advances_st_across_probe_rows() {
        // `st` is carried across `k`: once a gallery row is passed as too short, no later probe row
        // reconsiders it. Sound only because both Webs are distance-sorted, and observable only
        // from a probe that is not — which stage 1 cannot emit.
        let gallery = [plain(100, (11, 12)), plain(1000, (13, 14))];

        // Probe descending: the long row first pushes `st` past gallery[0], then the short row
        // starts at gallery[1] and breaks immediately.
        let descending = [plain(1000, (1, 2)), plain(100, (3, 4)), plain(100, (5, 6))];
        let rows = bz_match(&descending, &gallery);
        assert_eq!(rows.len(), 1, "only the long/long pair survives");
        assert_eq!(rows[0][1], 1, "and it is probe row 1");
        assert!(
            !rows.iter().any(|r| r[1] == 3),
            "probe row 3 must find nothing: st has already passed the gallery row it matches"
        );

        // The control: the same probe row 3, same gallery, examined from st = 1 — it matches. So
        // the absence above is the cursor, not the geometry.
        let short_first = [plain(100, (3, 4)), plain(100, (5, 6))];
        let rows = bz_match(&short_first, &gallery);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0][1], rows[0][3]),
            (3, 11),
            "probe row 3 pairs with gallery row 11 when st has not passed it"
        );
    }

    #[test]
    fn bz_match_breaks_when_gallery_edge_longer() {
        // A gallery edge longer than the probe edge and out of band ends the row: every later j is
        // longer still. The long gallery row after it is never paired.
        let probe = [plain(100, (1, 2)), plain(100, (3, 4))];
        let gallery = [plain(10_000, (11, 12)), plain(100, (13, 14))];
        assert!(
            bz_match(&probe, &gallery).is_empty(),
            "the break must skip gallery row 13, despite its matching distance"
        );
    }

    #[test]
    fn bz_match_angle_band_boundaries() {
        // Incompatible when `d*d > TXS && d*d < CTXS` — both strict, so both bounds are kept.
        for (delta, compatible) in [(11, true), (12, false), (348, false), (349, true)] {
            let probe = [row(100, (delta, 0), (1, 2), 0), plain(100, (3, 4))];
            let gallery = [plain(100, (11, 12))];
            let rows = bz_match(&probe, &gallery);
            assert_eq!(
                !rows.is_empty(),
                compatible,
                "beta delta {delta} (d*d = {}) should be {}",
                delta * delta,
                if compatible { "kept" } else { "rejected" }
            );
        }
    }

    #[test]
    fn bz_match_decodes_swap_flag_at_220() {
        let gallery = [plain(100, (11, 12))];
        let paired = |theta_kj| {
            let probe = [row(100, (0, 0), (1, 2), theta_kj), plain(100, (3, 4))];
            bz_match(&probe, &gallery)[0]
        };

        // Below the threshold: theta_kj is taken as-is, and the gallery pair keeps its order.
        let unswapped = paired(219);
        assert_eq!(unswapped[0], iangle180(219), "219 is not a swap flag");
        assert_eq!((unswapped[3], unswapped[4]), (11, 12));

        // At it: theta_kj decodes as `- 580`, and the differing flags flip the gallery pair.
        let swapped = paired(220);
        assert_eq!(swapped[0], iangle180(220 - 580), "220 is a swap flag");
        assert_eq!(
            (swapped[3], swapped[4]),
            (12, 11),
            "one side swapped, so the gallery endpoints flip"
        );
    }

    #[test]
    fn bz_match_keeps_the_gallery_pair_when_both_sides_are_swapped() {
        // Flags equal, so no flip — the flip is about disagreement, not about being swapped.
        let probe = [row(100, (0, 0), (1, 2), 400), plain(100, (3, 4))];
        let gallery = [row(100, (0, 0), (11, 12), 400)];
        let rows = bz_match(&probe, &gallery);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0][3], rows[0][4]), (11, 12));
    }

    #[test]
    fn bz_match_sorts_by_probe_k_then_gallery_a_then_probe_j() {
        // The key precedence is {1, 3, 2}, not (1, 2, 3): gallery_a outranks probe_j. A "tidying"
        // refactor to the natural column order would reorder the table the cluster stage walks.
        let probe = [plain(100, (1, 9)), plain(100, (1, 5)), plain(100, (7, 7))];
        let gallery = [plain(100, (30, 31)), plain(100, (20, 21))];
        let rows = bz_match(&probe, &gallery);
        assert!(rows.len() > 2, "need several rows to observe an order");
        let keys: Vec<_> = rows.iter().map(|r| (r[1], r[3], r[2])).collect();
        let mut want = keys.clone();
        want.sort();
        assert_eq!(
            keys, want,
            "rows must ascend by (probe_k, gallery_a, probe_j)"
        );
    }

    #[test]
    fn bz_match_stops_at_table_overflow_limit() {
        // Unlike stage 1's identical guard, this one is reachable from `match_score`: the pair
        // count is bounded by the two Webs' lengths, not by the minutia cap.
        let probe: Vec<CompRow> = (0..201).map(|i| plain(100, (i, i))).collect();
        let gallery: Vec<CompRow> = (0..101).map(|i| plain(100, (i, i))).collect();
        assert_eq!(bz_match(&probe, &gallery).len(), TABLE_OVERFLOW_LIMIT);
    }
}
