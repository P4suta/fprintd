// SPDX-FileCopyrightText: NIST NBIS — U.S. Government work, public domain (title 17 §105)
//
// SPDX-License-Identifier: LicenseRef-NBIS-PD

//! Bit-exact numeric primitives shared across the port: `sround` (round-half-away-from-zero),
//! `trunc_dbl_precision` (fixed-scale quantization), and the stable bubble-sort group the stock C
//! relies on wherever ordering matters. All arithmetic is `f64` (the sole `f32` exception,
//! `xytreps`'s `degrees_per_unit`, lives in `xyt`). See `docs/mindtct-algorithm.md` §Bit-exactness.
//!
//! These reproduce the corresponding `lfs.h` macros and `sort.c` routines *verbatim*: the same
//! rounding direction, the same truncating `(int)` casts, and the same strict-`>` / strict-`<`
//! comparisons — the latter being what makes every sort **stable** (equal keys never swap, so their
//! input order is preserved). Nothing here allocates beyond the index vectors the `sort_indices_*`
//! helpers are defined to return.

/// Round half away from zero, then truncate to `i32` — the `lfs.h` `sround` macro.
///
/// Stock: `((int)(((x)<0) ? (x)-0.5 : (x)+0.5))`. A positive value gets `+0.5`, a negative value
/// `-0.5`, and the `(int)` cast then truncates toward zero — so `2.5 -> 3` and `-2.5 -> -3` (half
/// rounds *away* from zero, not to-even). Equivalent to `(x + copysign(0.5, x)).trunc()` for every
/// value except `-0.0`, where stock's `(x)<0` test is false and takes the `+0.5` branch, matched
/// here. The `as i32` truncation mirrors C's `(int)` cast over the ranges MINDTCT feeds it.
pub(crate) fn sround(x: f64) -> i32 {
    (if x < 0.0 { x - 0.5 } else { x + 0.5 }) as i32
}

/// Quantize `x` to a multiple of `1.0 / scale`, matching the `lfs.h` `trunc_dbl_precision` macro.
///
/// Stock: `((double)(((x)<0.0) ? ((int)((x)*(scale)-0.5))/(scale) : ((int)((x)*(scale)+0.5))/(scale)))`.
/// Multiply by `scale`, round half away from zero via the signed `±0.5`, truncate to `int`, then
/// divide back out as a `double`. MINDTCT calls this with `scale = TRUNC_SCALE` (16384) to strip
/// sub-`1/16384` noise so intermediate `f64` results are reproducible bit-for-bit.
pub(crate) fn trunc_dbl_precision(x: f64, scale: f64) -> f64 {
    let scaled = x * scale;
    let n = if x < 0.0 {
        (scaled - 0.5) as i32
    } else {
        (scaled + 0.5) as i32
    };
    f64::from(n) / scale
}

/// Stable bubble-sort core shared by the `*_2` routines: sort `ranks` ascending (or by whatever
/// `should_swap` decides) while carrying `items` along in lockstep.
///
/// Faithful to `sort.c`: an outer "did anything swap?" loop with a shrinking `n`, an inner
/// adjacent-pair scan, and a swap guarded by a *strict* comparison so equal keys are left untouched
/// — the property that keeps the sort stable. `should_swap(ranks[p], ranks[p+1])` returns whether
/// the pair is out of order.
fn bubble_2<R: Copy>(ranks: &mut [R], items: &mut [i32], should_swap: impl Fn(R, R) -> bool) {
    debug_assert_eq!(ranks.len(), items.len());
    let mut n = ranks.len();
    let mut done = false;
    while !done {
        done = true;
        for p in 0..n.saturating_sub(1) {
            if should_swap(ranks[p], ranks[p + 1]) {
                ranks.swap(p, p + 1);
                items.swap(p, p + 1);
                done = false;
            }
        }
        n = n.saturating_sub(1);
    }
}

/// `bubble_sort_int_inc_2`: sort integer `ranks` into increasing order, carrying `items`.
pub(crate) fn bubble_sort_int_inc_2(ranks: &mut [i32], items: &mut [i32]) {
    bubble_2(ranks, items, |a, b| a > b);
}

/// `bubble_sort_double_inc_2`: sort double `ranks` into increasing order, carrying `items`.
pub(crate) fn bubble_sort_double_inc_2(ranks: &mut [f64], items: &mut [i32]) {
    bubble_2(ranks, items, |a, b| a > b);
}

/// `bubble_sort_double_dec_2`: sort double `ranks` into decreasing order, carrying `items`.
pub(crate) fn bubble_sort_double_dec_2(ranks: &mut [f64], items: &mut [i32]) {
    bubble_2(ranks, items, |a, b| a < b);
}

/// `bubble_sort_int_inc`: sort integer `ranks` into increasing order in place (no carried items).
pub(crate) fn bubble_sort_int_inc(ranks: &mut [i32]) {
    let mut n = ranks.len();
    let mut done = false;
    while !done {
        done = true;
        for p in 0..n.saturating_sub(1) {
            if ranks[p] > ranks[p + 1] {
                ranks.swap(p, p + 1);
                done = false;
            }
        }
        n = n.saturating_sub(1);
    }
}

/// `sort_indices_int_inc`: sort `ranks` ascending in place and return the permutation of original
/// indices (`0..len`) that produces that order — the stock C's `order` output.
pub(crate) fn sort_indices_int_inc(ranks: &mut [i32]) -> Vec<i32> {
    let mut order: Vec<i32> = (0..ranks.len()).map(|i| i as i32).collect();
    bubble_sort_int_inc_2(ranks, &mut order);
    order
}

/// `sort_indices_double_inc`: sort `ranks` ascending in place and return the permutation of original
/// indices (`0..len`) that produces that order — the stock C's `order` output.
///
/// `dead_code`: the `double` twin of the wired [`sort_indices_int_inc`]. The V2 pipeline sorts only
/// integer ranks, so this variant has no caller; transcribed for fidelity and exercised by the tests
/// below. Targeted per-item allow — the minimal suppression.
#[allow(dead_code)]
pub(crate) fn sort_indices_double_inc(ranks: &mut [f64]) -> Vec<i32> {
    let mut order: Vec<i32> = (0..ranks.len()).map(|i| i as i32).collect();
    bubble_sort_double_inc_2(ranks, &mut order);
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- sround: round half away from zero, then truncate toward zero -------------------------
    #[test]
    fn sround_matches_stock_macro() {
        assert_eq!(sround(0.0), 0);
        assert_eq!(sround(0.4), 0);
        assert_eq!(sround(0.5), 1);
        assert_eq!(sround(0.6), 1);
        assert_eq!(sround(1.49), 1);
        assert_eq!(sround(2.5), 3); // half rounds AWAY from zero, not to-even

        // Negative side: -0.5 branch, still truncating toward zero.
        assert_eq!(sround(-0.4), 0);
        assert_eq!(sround(-0.5), -1);
        assert_eq!(sround(-0.6), -1);
        assert_eq!(sround(-2.5), -3);
        // -0.0 takes the (x)<0 == false / +0.5 branch, exactly like the C macro.
        assert_eq!(sround(-0.0), 0);
    }

    // --- trunc_dbl_precision: quantize to 1/scale --------------------------------------------
    #[test]
    fn trunc_dbl_precision_quantizes_to_scale() {
        let s = 16384.0_f64; // TRUNC_SCALE

        // Exactly representable multiples pass through unchanged.
        assert_eq!(trunc_dbl_precision(0.0, s), 0.0);
        assert_eq!(trunc_dbl_precision(1.0, s), 1.0);
        assert_eq!(trunc_dbl_precision(-1.0, s), -1.0);
        assert_eq!(trunc_dbl_precision(0.5, s), 0.5);
        assert_eq!(trunc_dbl_precision(-0.5, s), -0.5);

        // A value landing on x*scale == 1.5 rounds half away from zero to 2 units.
        assert_eq!(trunc_dbl_precision(3.0 / 32768.0, s), 2.0 / 16384.0);
        assert_eq!(trunc_dbl_precision(-3.0 / 32768.0, s), -2.0 / 16384.0);

        // Result is always an integer multiple of 1/scale.
        let q = trunc_dbl_precision(0.123_456_789, s);
        assert_eq!((q * s).round(), q * s);
    }

    // --- stable bubble sorts ------------------------------------------------------------------
    #[test]
    fn bubble_sort_int_inc_sorts_ascending() {
        let mut v = [3, 1, 2, 1, 0];
        bubble_sort_int_inc(&mut v);
        assert_eq!(v, [0, 1, 1, 2, 3]);
    }

    #[test]
    fn bubble_sort_int_inc_2_is_stable() {
        let mut ranks = [2, 1, 1, 0];
        let mut items = [0, 1, 2, 3];
        bubble_sort_int_inc_2(&mut ranks, &mut items);
        assert_eq!(ranks, [0, 1, 1, 2]);
        // The two equal keys (rank 1) keep their input order: item 1 before item 2.
        assert_eq!(items, [3, 1, 2, 0]);
    }

    #[test]
    fn bubble_sort_double_inc_2_is_stable() {
        let mut ranks = [2.0, 1.0, 1.0, 0.0];
        let mut items = [0, 1, 2, 3];
        bubble_sort_double_inc_2(&mut ranks, &mut items);
        assert_eq!(ranks, [0.0, 1.0, 1.0, 2.0]);
        assert_eq!(items, [3, 1, 2, 0]);
    }

    #[test]
    fn bubble_sort_double_dec_2_is_stable() {
        let mut ranks = [1.0, 3.0, 2.0, 3.0];
        let mut items = [0, 1, 2, 3];
        bubble_sort_double_dec_2(&mut ranks, &mut items);
        assert_eq!(ranks, [3.0, 3.0, 2.0, 1.0]);
        // The two equal keys (rank 3.0) keep their input order: item 1 before item 3.
        assert_eq!(items, [1, 3, 2, 0]);
    }

    #[test]
    fn sort_indices_int_inc_returns_permutation() {
        let mut ranks = [30, 10, 20];
        let order = sort_indices_int_inc(&mut ranks);
        assert_eq!(ranks, [10, 20, 30]);
        assert_eq!(order, [1, 2, 0]);
    }

    #[test]
    fn sort_indices_double_inc_returns_permutation() {
        let mut ranks = [30.0, 10.0, 20.0];
        let order = sort_indices_double_inc(&mut ranks);
        assert_eq!(ranks, [10.0, 20.0, 30.0]);
        assert_eq!(order, [1, 2, 0]);
    }

    // Empty and single-element inputs must not loop forever or panic.
    #[test]
    fn sorts_handle_degenerate_lengths() {
        let mut empty: [i32; 0] = [];
        bubble_sort_int_inc(&mut empty);
        let mut one = [42];
        bubble_sort_int_inc(&mut one);
        assert_eq!(one, [42]);

        let mut r0: [i32; 0] = [];
        assert_eq!(sort_indices_int_inc(&mut r0), Vec::<i32>::new());
    }
}
