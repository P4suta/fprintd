// SPDX-FileCopyrightText: NIST NBIS — U.S. Government work, public domain (title 17 §105)
//
// SPDX-License-Identifier: LicenseRef-NBIS-PD

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
