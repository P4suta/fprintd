// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Shared arithmetic and geometry helpers ported from stock NBIS `mindtct/src/lib/mindtct/util.c`.
//!
//! These small routines have a single stock origin (`util.c`) but several consumers across the
//! pipeline — detection ([`squared_distance`] in `detect/loops.rs`), false-minutia removal
//! ([`distance`], [`squared_distance`], [`closest_dir_dist`], [`minmaxs`] in `remove.rs`) and
//! ridge counting ([`squared_distance`], [`angle2line`], [`find_incr_position_dbl`] in `ridges.rs`).
//! Collecting them here keeps that one origin in one place rather than re-copied per consumer.
//!
//! Each is a verbatim transcription: the `f64` arithmetic (subtraction, squaring, `atan2`, `sqrt`)
//! matches the reference bit-for-bit. See `docs/mindtct-algorithm.md` §Bit-exactness.

/// Stock `INVALID_DIR` (`lfs.h` L320): a direction that could not be determined.
const INVALID_DIR: i32 = -1;

/// Stock `MIN_SLOPE_DELTA` (`lfs.h` L701): below this per-axis delta a line is treated as having zero
/// slope in [`angle2line`], avoiding a degenerate `atan2(0, 0)`.
const MIN_SLOPE_DELTA: f64 = 0.5;

/// Euclidean distance between two integer points — port of stock `distance` (`util.c` L358).
pub(crate) fn distance(x1: i32, y1: i32, x2: i32, y2: i32) -> f64 {
    // PORT L363–L369: sqrt of the squared distance.
    let dx = f64::from(x1 - x2);
    let dy = f64::from(y1 - y2);
    ((dx * dx) + (dy * dy)).sqrt()
}

/// Squared Euclidean distance between two integer points — port of stock `squared_distance`
/// (`util.c` L388).
pub(crate) fn squared_distance(x1: i32, y1: i32, x2: i32, y2: i32) -> f64 {
    // PORT L393–L397: (x1-x2)^2 + (y1-y2)^2, computed in `f64`.
    let dx = f64::from(x1 - x2);
    let dy = f64::from(y1 - y2);
    (dx * dx) + (dy * dy)
}

/// Inner (wrap-aware) distance between two integer directions — port of stock `closest_dir_dist`
/// (`util.c` L602).
///
/// Returns [`INVALID_DIR`] if either direction is invalid (`< 0`), else the smaller of the direct and
/// wrap-around distances on a circle of `ndirs` directions.
pub(crate) fn closest_dir_dist(dir1: i32, dir2: i32, ndirs: i32) -> i32 {
    // PORT L607–L618: only defined for two valid directions.
    if dir1 >= 0 && dir2 >= 0 {
        let d1 = (dir2 - dir1).abs();
        let d2 = ndirs - d1;
        d1.min(d2)
    } else {
        INVALID_DIR
    }
}

/// Compute the angle (radians) of the line from `(fx, fy)` to `(tx, ty)` — port of stock `angle2line`
/// (`util.c` L519).
///
/// The slope is measured as `dy = fy - ty`, `dx = tx - fx` (the reference's asymmetric subtraction
/// order, verbatim); when both deltas fall below [`MIN_SLOPE_DELTA`] the angle is `0.0`.
pub(crate) fn angle2line(fx: i32, fy: i32, tx: i32, ty: i32) -> f64 {
    // PORT L523–L525: slope components (mixed subtraction order is intentional).
    let dy = f64::from(fy - ty);
    let dx = f64::from(tx - fx);
    // PORT L527–L532: sufficiently flat → 0.0, else the arctangent of the slope.
    if dx.abs() < MIN_SLOPE_DELTA && dy.abs() < MIN_SLOPE_DELTA {
        0.0
    } else {
        dy.atan2(dx)
    }
}

/// Insertion point for `val` in an increasing-sorted `list` — port of stock `find_incr_position_dbl`
/// (`util.c` L485).
///
/// Returns the first index whose value exceeds `val` (preserving increasing order), or `list.len()`
/// when `val` is `>=` every element.
pub(crate) fn find_incr_position_dbl(val: f64, list: &[f64]) -> usize {
    // PORT L489–L499: first slot whose value is strictly greater than `val`.
    for (i, &v) in list.iter().enumerate() {
        if val < v {
            return i;
        }
    }
    // PORT L503: never smaller → append at the end.
    list.len()
}

/// The three parallel outputs of [`minmaxs`] — value, type (`-1` minima / `+1` maxima) and index of
/// each relative extremum.
pub(crate) struct MinMaxs {
    pub(crate) val: Vec<i32>,
    pub(crate) kind: Vec<i32>,
    pub(crate) idx: Vec<i32>,
}

/// Locate relative minima and maxima in a vector of integers — port of stock `minmaxs` (`util.c`
/// L158).
///
/// Walks the run-length structure of the sequence, recording each turning point at the midpoint of the
/// level run that precedes it. Fewer than three items yields no extrema.
pub(crate) fn minmaxs(items: &[i32]) -> MinMaxs {
    let num = items.len() as i32;

    // PORT L168–L174: fewer than three items → no min/max possible.
    if num < 3 {
        return MinMaxs {
            val: Vec::new(),
            kind: Vec::new(),
            idx: Vec::new(),
        };
    }

    let mut val = Vec::new();
    let mut kind = Vec::new();
    let mut idx = Vec::new();

    // PORT L204–L219: initial state from the first pair; start location at the first item.
    let mut i: i32 = 0;
    let diff = items[1] - items[0];
    let mut state = if diff > 0 {
        1
    } else if diff < 0 {
        -1
    } else {
        0
    };
    let mut start: i32 = 0;
    i += 1;

    // PORT L222–L332: fold each successive item pair into the running state.
    while i < num - 1 {
        let diff = items[(i + 1) as usize] - items[i as usize];
        if diff > 0 {
            // PORT L227–L275: increasing.
            if state == 1 {
                start = i;
            } else if state == -1 {
                // PORT L234–L248: a minima at the midpoint of the preceding decline.
                let loc = (start + i) / 2;
                val.push(items[loc as usize]);
                kind.push(-1);
                idx.push(loc);
                state = 1;
                start = i;
            } else {
                // PORT L251–L274: previously level (only at the list head).
                if i - start > 1 {
                    let loc = (start + i) / 2;
                    val.push(items[loc as usize]);
                    kind.push(-1);
                    idx.push(loc);
                }
                state = 1;
                start = i;
            }
        } else if diff < 0 {
            // PORT L278–L326: decreasing.
            if state == -1 {
                start = i;
            } else if state == 1 {
                // PORT L285–L298: a maxima at the midpoint of the preceding rise.
                let loc = (start + i) / 2;
                val.push(items[loc as usize]);
                kind.push(1);
                idx.push(loc);
                state = -1;
                start = i;
            } else {
                // PORT L302–L325: previously level (only at the list head).
                if i - start > 1 {
                    let loc = (start + i) / 2;
                    val.push(items[loc as usize]);
                    kind.push(1);
                    idx.push(loc);
                }
                state = -1;
                start = i;
            }
        }
        // PORT L328: level items just advance.
        i += 1;
    }

    MinMaxs { val, kind, idx }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_and_squared_distance_agree() {
        assert_eq!(squared_distance(0, 0, 3, 4), 25.0);
        assert_eq!(distance(0, 0, 3, 4), 5.0);
        assert_eq!(distance(2, 2, 2, 2), 0.0);
    }

    #[test]
    fn squared_distance_matches_stock() {
        assert_eq!(squared_distance(0, 0, 3, 4), 25.0);
        assert_eq!(squared_distance(1, 1, 1, 1), 0.0);
        assert_eq!(squared_distance(-2, 0, 1, 0), 9.0);
    }

    #[test]
    fn closest_dir_dist_wraps_and_guards() {
        // 16-direction semicircle: 1 vs 15 is 2 the short way (wrap), not 14.
        assert_eq!(closest_dir_dist(1, 15, 16), 2);
        assert_eq!(closest_dir_dist(3, 5, 16), 2);
        // Either direction invalid → INVALID_DIR.
        assert_eq!(closest_dir_dist(-1, 5, 16), INVALID_DIR);
    }

    #[test]
    fn angle2line_flat_and_axis_aligned() {
        // Both deltas below MIN_SLOPE_DELTA → defined as 0.0.
        assert_eq!(angle2line(4, 4, 4, 4), 0.0);
        // dy = fy-ty = 0, dx = tx-fx = 3 → atan2(0, 3) = 0.
        assert_eq!(angle2line(0, 0, 3, 0), 0.0);
        // dy = 0-0, dx = 0-0 for x... use a vertical line: fx=0,fy=0,tx=0,ty=3 → dy=-3,dx=0 → -pi/2.
        assert!((angle2line(0, 0, 0, 3) - (-std::f64::consts::FRAC_PI_2)).abs() < 1e-12);
    }

    #[test]
    fn find_incr_position_dbl_finds_slot() {
        let list = [1.0, 3.0, 5.0];
        assert_eq!(find_incr_position_dbl(0.0, &list), 0);
        assert_eq!(find_incr_position_dbl(2.0, &list), 1);
        assert_eq!(find_incr_position_dbl(4.0, &list), 2);
        assert_eq!(find_incr_position_dbl(9.0, &list), 3);
        // Equal to an element is NOT strictly less → inserts after it.
        assert_eq!(find_incr_position_dbl(3.0, &list), 2);
    }

    #[test]
    fn minmaxs_finds_a_single_minimum() {
        // A V-shape: descend then ascend → exactly one minima of type -1. The reference resets
        // `start` on every continued-decrease step, so a strict decline reports the extremum at the
        // midpoint of the last tracked pair — here index 1 (value 3), not the true trough at index 2.
        let items = [5, 3, 1, 3, 5];
        let mm = minmaxs(&items);
        assert_eq!(mm.idx.len(), 1);
        assert_eq!(mm.kind[0], -1);
        assert_eq!(mm.idx[0], 1);
        assert_eq!(mm.val[0], 3);
    }

    #[test]
    fn minmaxs_ignores_short_sequences() {
        assert_eq!(minmaxs(&[1, 2]).idx.len(), 0);
    }
}
