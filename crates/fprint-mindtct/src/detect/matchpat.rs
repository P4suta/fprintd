// SPDX-FileCopyrightText: NIST NBIS — U.S. Government work, public domain (title 17 §105)
//
// SPDX-License-Identifier: LicenseRef-NBIS-PD

//! The fixed 2×3 pixel-pair feature patterns and the routines that match them against the binary
//! image to flag ridge endings and bifurcations — port of stock NBIS
//! `mindtct/src/lib/mindtct/matchpat.c` (`match_{1st,2nd,3rd}_pair`,
//! `skip_repeated_{horizontal,vertical}_pair`) and the `feature_patterns[]` table from `globals.c`.
//!
//! A candidate minutia is a run of three consecutive pixel *pairs* along a scan (two adjacent rows
//! for a horizontal scan, two adjacent columns for a vertical one). Each of the ten
//! [`FEATURE_PATTERNS`] fixes the top/left and bottom/right pixel of the first, second and third
//! pair; a location matches a pattern only when all three pairs agree. The scan narrows the
//! candidate set one pair at a time — [`match_1st_pair`] seeds it from the first pair,
//! [`match_2nd_pair`] and [`match_3rd_pair`] filter it — while
//! [`skip_repeated_horizontal_pair`]/[`skip_repeated_vertical_pair`] slide across runs of the
//! repeated second pair so the third pair is tested at the far edge of the feature. See
//! `docs/mindtct-algorithm.md`.

use super::{BIFURCATION, RIDGE_ENDING};

/// Number of feature patterns — stock `NFEATURES` (`lfs.h` L151).
pub(crate) const NFEATURES: usize = 10;

/// One entry of the stock `feature_patterns[]` table — the port's analogue of `FEATURE_PATTERN`
/// (`lfs.h` L178–L184), one field per member.
///
/// A pattern fixes three ordered pixel pairs (`first`, `second`, `third`). Each pair is
/// `[top_or_left, bottom_or_right]` and each pixel is a *binary* value in stock detect convention
/// (`0 == valley/white`, `1 == ridge/black`) — the same domain as the `bdata` the scan reads
/// (`detect.c` L614 `gray2bin(1, 1, 0)`), so the stored `u8`s compare directly against scan pixels.
///
/// The stock struct carries `int type` (`RIDGE_ENDING`/`BIFURCATION`) and `int appearing`
/// (`APPEARING`/`DISAPPEARING`); here `type` becomes [`kind`](Self::kind) (a Rust keyword) and
/// `appearing` becomes a `bool` (`APPEARING == true`), mirroring the fields of
/// [`DetMinutia`](super::DetMinutia) that a match copies out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FeaturePattern {
    /// Feature class, [`RIDGE_ENDING`](super::RIDGE_ENDING) or
    /// [`BIFURCATION`](super::BIFURCATION) (stock `type`).
    pub kind: i32,
    /// Ridge-scan polarity: `true` = `APPEARING`, `false` = `DISAPPEARING` (stock `appearing`).
    pub appearing: bool,
    /// First pixel pair `[top_or_left, bottom_or_right]` (stock `first[2]`).
    pub first: [u8; 2],
    /// Second pixel pair (stock `second[2]`).
    pub second: [u8; 2],
    /// Third pixel pair (stock `third[2]`).
    pub third: [u8; 2],
}

/// The fixed table of minutia feature patterns — verbatim from stock `feature_patterns[]`
/// (`globals.c` L255–L314). The ten entries are the three vertical-pair definitions for a ridge
/// ending (appearing/disappearing) followed by eight bifurcation variants; a matched location's
/// index into this table becomes the stock `feature_id` (and thus `possible[0]`).
///
// PORT: `APPEARING`/`DISAPPEARING` (`lfs.h` L154–L155, `1`/`0`) collapse into the `appearing`
// `bool`; the `{0,1}` pixel literals become `u8`. The table is a `const` (read-only, no interior
// mutability) rather than the C file-scope array; order is preserved so indices match the reference.
pub(crate) const FEATURE_PATTERNS: [FeaturePattern; NFEATURES] = [
    // a. Ridge Ending (appearing).
    FeaturePattern {
        kind: RIDGE_ENDING,
        appearing: true,
        first: [0, 0],
        second: [0, 1],
        third: [0, 0],
    },
    // b. Ridge Ending (disappearing).
    FeaturePattern {
        kind: RIDGE_ENDING,
        appearing: false,
        first: [0, 0],
        second: [1, 0],
        third: [0, 0],
    },
    // c. Bifurcation (disappearing).
    FeaturePattern {
        kind: BIFURCATION,
        appearing: false,
        first: [1, 1],
        second: [0, 1],
        third: [1, 1],
    },
    // d. Bifurcation (appearing).
    FeaturePattern {
        kind: BIFURCATION,
        appearing: true,
        first: [1, 1],
        second: [1, 0],
        third: [1, 1],
    },
    // e. Bifurcation (disappearing).
    FeaturePattern {
        kind: BIFURCATION,
        appearing: false,
        first: [1, 0],
        second: [0, 1],
        third: [1, 1],
    },
    // f. Bifurcation (disappearing).
    FeaturePattern {
        kind: BIFURCATION,
        appearing: false,
        first: [1, 1],
        second: [0, 1],
        third: [1, 0],
    },
    // g. Bifurcation (appearing).
    FeaturePattern {
        kind: BIFURCATION,
        appearing: true,
        first: [1, 1],
        second: [1, 0],
        third: [0, 1],
    },
    // h. Bifurcation (appearing).
    FeaturePattern {
        kind: BIFURCATION,
        appearing: true,
        first: [0, 1],
        second: [1, 0],
        third: [1, 1],
    },
    // i. Bifurcation (disappearing).
    FeaturePattern {
        kind: BIFURCATION,
        appearing: false,
        first: [1, 0],
        second: [0, 1],
        third: [1, 0],
    },
    // j. Bifurcation (appearing).
    FeaturePattern {
        kind: BIFURCATION,
        appearing: true,
        first: [0, 1],
        second: [1, 0],
        third: [0, 1],
    },
];

/// Feature-pattern indices whose **first** pixel pair equals `(p1, p2)` — port of stock
/// `match_1st_pair` (`matchpat.c` L82).
///
/// Seeds the candidate set for a scan location. The stock out-parameters `possible[]`/`nposs`
/// collapse into the returned `Vec` (stock `nposs` is `.len()`); the returned indices are in
/// ascending table order, exactly as the reference fills `possible[]`.
pub(crate) fn match_1st_pair(p1: u8, p2: u8) -> Vec<usize> {
    // PORT L88–L100: `nposs = 0`, then keep every feature whose first pair matches the scan pair.
    let mut possible = Vec::new();
    for (i, fp) in FEATURE_PATTERNS.iter().enumerate() {
        if p1 == fp.first[0] && p2 == fp.first[1] {
            possible.push(i);
        }
    }
    // PORT L102–L103: return the accumulated possibilities.
    possible
}

/// Subset of `possible` whose **second** pixel pair equals `(p1, p2)` — port of stock
/// `match_2nd_pair` (`matchpat.c` L122).
///
/// Filters the candidate set produced by [`match_1st_pair`], preserving order. A pair whose two
/// pixels are equal can never be a second pair (every pattern's `second` differs across the pair),
/// so it short-circuits to empty.
pub(crate) fn match_2nd_pair(p1: u8, p2: u8, possible: &[usize]) -> Vec<usize> {
    // PORT L134–L136: equal pixels cannot form a second pair — return no possibilities.
    if p1 == p2 {
        return Vec::new();
    }

    // PORT L131/L138–L148: reset output, then retain candidates whose second pair matches.
    let mut out = Vec::new();
    for &i in possible {
        if p1 == FEATURE_PATTERNS[i].second[0] && p2 == FEATURE_PATTERNS[i].second[1] {
            out.push(i);
        }
    }
    // PORT L150–L151: return the surviving possibilities.
    out
}

/// Subset of `possible` whose **third** pixel pair equals `(p1, p2)` — port of stock
/// `match_3rd_pair` (`matchpat.c` L170).
///
/// The final filter of the candidate set; the surviving indices identify the matched feature(s),
/// with `possible[0]` the accepted `feature_id`. Unlike [`match_2nd_pair`] there is no equal-pixel
/// guard (a ridge-ending pattern's third pair is `{0, 0}`).
pub(crate) fn match_3rd_pair(p1: u8, p2: u8, possible: &[usize]) -> Vec<usize> {
    // PORT L179/L181–L191: reset output, then retain candidates whose third pair matches.
    let mut out = Vec::new();
    for &i in possible {
        if p1 == FEATURE_PATTERNS[i].third[0] && p2 == FEATURE_PATTERNS[i].third[1] {
            out.push(i);
        }
    }
    // PORT L193–L194: return the surviving possibilities.
    out
}

/// Slide a horizontal pixel-pair cursor rightward across repeats of the current pair — port of stock
/// `skip_repeated_horizontal_pair` (`matchpat.c` L216).
///
/// `p1`/`p2` are byte indices into `bdata` for the top and bottom pixel of the pair (the stock
/// `p1ptr`/`p2ptr`), and `cx` is their shared X-coordinate. The cursor is bumped one column right,
/// then advanced while the pair keeps repeating the *starting* pair, stopping at the first differing
/// pair or when `cx` reaches the region's right edge `ex`. On return `cx`, `p1` and `p2` mark where
/// the skip terminated. This lets the scan test the third pair against the pixels immediately past
/// the run of identical second pairs.
///
// PORT: the stock signature also takes `iw`/`ih`; both are unused in the horizontal skip (the step
// is `+1`), so they are omitted here. The `unsigned char **p1ptr`/`**p2ptr` pointer-to-pointer
// out-parameters become `&mut usize` byte indices into `bdata`; the terminal index is bumped one
// past the last compared pixel but never dereferenced (the `cx < ex` guard), matching the C.
pub(crate) fn skip_repeated_horizontal_pair(
    cx: &mut i32,
    ex: i32,
    p1: &mut usize,
    p2: &mut usize,
    bdata: &[u8],
) {
    // PORT L222–L223: remember the starting pixel pair.
    let old1 = bdata[*p1];
    let old2 = bdata[*p2];

    // PORT L226–L229: bump horizontally to the next pixel pair.
    *cx += 1;
    *p1 += 1;
    *p2 += 1;

    // PORT L231–L242: advance while the pair keeps repeating and the region is not exhausted.
    while *cx < ex {
        if bdata[*p1] != old1 || bdata[*p2] != old2 {
            return;
        }
        *cx += 1;
        *p1 += 1;
        *p2 += 1;
    }
}

/// Slide a vertical pixel-pair cursor downward across repeats of the current pair — port of stock
/// `skip_repeated_vertical_pair` (`matchpat.c` L264).
///
/// The vertical analogue of [`skip_repeated_horizontal_pair`]: `p1`/`p2` are byte indices for the
/// left and right pixel of the pair and `cy` their shared Y-coordinate. Each step advances by one
/// image row (`iw` bytes) rather than one column, stopping at the first differing pair or when `cy`
/// reaches the region's bottom edge `ey`.
///
// PORT: the stock signature also takes `ih` (unused; the row step is `iw`), so it is omitted. As in
// the horizontal case the pointer-to-pointer out-parameters become `&mut usize` byte indices; the
// per-step increment is `iw` (`p1ptr += iw`).
pub(crate) fn skip_repeated_vertical_pair(
    cy: &mut i32,
    ey: i32,
    p1: &mut usize,
    p2: &mut usize,
    iw: i32,
    bdata: &[u8],
) {
    // PORT L270–L271: remember the starting pixel pair.
    let old1 = bdata[*p1];
    let old2 = bdata[*p2];

    // PORT L274–L277: bump vertically (one row = `iw` bytes) to the next pixel pair.
    let step = iw as usize;
    *cy += 1;
    *p1 += step;
    *p2 += step;

    // PORT L279–L290: advance while the pair keeps repeating and the region is not exhausted.
    while *cy < ey {
        if bdata[*p1] != old1 || bdata[*p2] != old2 {
            return;
        }
        *cy += 1;
        *p1 += step;
        *p2 += step;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_matches_reference() {
        // Length and the two anchor entries transcribed straight from globals.c.
        assert_eq!(FEATURE_PATTERNS.len(), NFEATURES);
        assert_eq!(
            FEATURE_PATTERNS[0],
            FeaturePattern {
                kind: RIDGE_ENDING,
                appearing: true,
                first: [0, 0],
                second: [0, 1],
                third: [0, 0],
            }
        );
        assert_eq!(
            FEATURE_PATTERNS[9],
            FeaturePattern {
                kind: BIFURCATION,
                appearing: true,
                first: [0, 1],
                second: [1, 0],
                third: [0, 1],
            }
        );
    }

    #[test]
    fn first_pair_seeds_in_table_order() {
        // `{0,0}` is the first pair of the two ridge-ending patterns (indices 0, 1), in order.
        assert_eq!(match_1st_pair(0, 0), vec![0, 1]);
        // `{1,1}` is the first pair of patterns c, d, f, g (indices 2, 3, 5, 6).
        assert_eq!(match_1st_pair(1, 1), vec![2, 3, 5, 6]);
        // `{1,0}` seeds e and i; `{0,1}` seeds h and j.
        assert_eq!(match_1st_pair(1, 0), vec![4, 8]);
        assert_eq!(match_1st_pair(0, 1), vec![7, 9]);
    }

    #[test]
    fn full_match_narrows_to_ridge_ending_appearing() {
        // Walk pattern a end to end: first {0,0} -> second {0,1} -> third {0,0}.
        let p = match_1st_pair(0, 0);
        assert_eq!(p, vec![0, 1]);
        let p = match_2nd_pair(0, 1, &p);
        assert_eq!(p, vec![0]);
        let p = match_3rd_pair(0, 0, &p);
        assert_eq!(p, vec![0]);
        assert_eq!(FEATURE_PATTERNS[p[0]].kind, RIDGE_ENDING);
        assert!(FEATURE_PATTERNS[p[0]].appearing);
    }

    #[test]
    fn second_pair_rejects_equal_pixels() {
        // A pair of identical pixels can never be a valid second pair.
        assert!(match_2nd_pair(0, 0, &[0, 1, 2]).is_empty());
        assert!(match_2nd_pair(1, 1, &[2, 3]).is_empty());
    }

    #[test]
    fn third_pair_disambiguates_bifurcations() {
        // e and f share first {1,1}?  No: seed {1,0} then {0,1} keeps e (idx 4) and i (idx 8);
        // third {1,1} accepts only e, third {1,0} only i.
        let seed = match_1st_pair(1, 0);
        assert_eq!(seed, vec![4, 8]);
        let second = match_2nd_pair(0, 1, &seed);
        assert_eq!(second, vec![4, 8]);
        assert_eq!(match_3rd_pair(1, 1, &second), vec![4]);
        assert_eq!(match_3rd_pair(1, 0, &second), vec![8]);
    }

    #[test]
    fn horizontal_skip_stops_at_first_differing_pair() {
        // iw = 4. Row 0: 0 0 0 1 ; row 1: 1 1 1 1.  Pair (top,bottom) is (0,1) at x=0,1,2 then
        // (1,1) at x=3.  Starting at x=0 the skip slides to x=3 where the top pixel changes.
        let bdata = [0u8, 0, 0, 1, /* row 1 */ 1, 1, 1, 1];
        let mut cx = 0i32;
        let mut p1 = 0usize; // (row 0, col 0)
        let mut p2 = 4usize; // (row 1, col 0)
        skip_repeated_horizontal_pair(&mut cx, 4, &mut p1, &mut p2, &bdata);
        assert_eq!((cx, p1, p2), (3, 3, 7));
    }

    #[test]
    fn horizontal_skip_terminates_at_region_edge() {
        // Every pair identical: the skip runs to the right edge `ex` and stops there.
        let bdata = [0u8, 0, 0, 0, /* row 1 */ 1, 1, 1, 1];
        let mut cx = 0i32;
        let mut p1 = 0usize;
        let mut p2 = 4usize;
        skip_repeated_horizontal_pair(&mut cx, 4, &mut p1, &mut p2, &bdata);
        assert_eq!(cx, 4);
    }

    #[test]
    fn vertical_skip_stops_at_first_differing_pair() {
        // iw = 2, 4 rows.  Left/right columns:
        //   row 0: 0 1   row 1: 0 1   row 2: 0 1   row 3: 1 1
        // Pair (left,right) is (0,1) for rows 0..2 then (1,1) at row 3.
        let bdata = [
            0u8, 1, /* r1 */ 0, 1, /* r2 */ 0, 1, /* r3 */ 1, 1,
        ];
        let mut cy = 0i32;
        let mut p1 = 0usize; // (row 0, col 0)
        let mut p2 = 1usize; // (row 0, col 1)
        skip_repeated_vertical_pair(&mut cy, 4, &mut p1, &mut p2, 2, &bdata);
        assert_eq!((cy, p1, p2), (3, 6, 7));
    }
}
