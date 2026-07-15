// SPDX-FileCopyrightText: NIST NBIS — U.S. Government work, public domain (title 17 §105)
//
// SPDX-License-Identifier: LicenseRef-NBIS-PD

//! Straight-line pixel trajectories between two points — port of stock NBIS
//! `mindtct/src/lib/mindtct/line.c` (`line_points`), used to walk the contiguous pixels a contour
//! scan steps along (the basis for ridge counting and free-path tests).
//!
//! The walk is Bresenham-shaped but computed in `f64`: one axis advances by an integer `±1` each
//! step while the other accumulates a fractional `x_factor` / `y_factor`. After every accumulation
//! the running coordinate is quantized through [`trunc_dbl_precision`] at `1/16384`
//! ([`TRUNC_SCALE`]) — this is what makes the trajectory identical on every architecture — and only
//! then rounded to a pixel via the stock `(int)(r + 0.5)` cast. See `docs/mindtct-algorithm.md`
//! §Bit-exactness.

use crate::consts::TRUNC_SCALE;
use crate::num::trunc_dbl_precision;

/// Contiguous pixel coordinates of the line connecting `(x1, y1)` to `(x2, y2)`, endpoints included
/// — port of stock `line_points` (`line.c` L82).
///
/// Returns the trajectory as an ordered `Vec<(x, y)>`; the stock out-parameters `ox_list`/`oy_list`
/// (parallel arrays) and `onum` (count) collapse into this single structured value, with `onum`
/// recovered as `.len()`.
///
/// # Algorithm (verbatim to `line.c`)
///
/// One axis is the *driving* axis (advances by `±1` each iteration); the other is *interpolated*
/// from an `f64` accumulator. `inx`/`iny` select which axis drives (`inx = |dx| > |dy|`,
/// `iny = |dy| > |dx|`; both `0` on the exact diagonal, where the walk steps `(±1, ±1)`), and
/// `intx = 1 - iny` / `inty = 1 - inx` gate the integer vs. interpolated contribution to each new
/// pixel. Per step (`L198`–`L213`):
///
/// 1. `rx += x_factor; ry += y_factor` (`L198`–`L199`).
/// 2. `rx`, `ry` are [`trunc_dbl_precision`]-quantized at [`TRUNC_SCALE`] (`L203`–`L204`), stripping
///    sub-`1/16384` noise so the following round is reproducible.
/// 3. `ix = intx·(ix + x_incr) + iny·(int)(rx + 0.5)` and the mirror for `iy` (`L208`–`L209`). The
///    `(int)(r + 0.5)` cast is a truncate-toward-zero of `r + 0.5` — *not* `sround`; it carries no
///    negative-side `-0.5`, matching the stock exactly (image coordinates are non-negative).
///
/// The loop runs until `(ix, iy) == (x2, y2)`.
///
/// # Errors
///
/// `Err(-412)` if the point count would exceed the stock capacity bound
/// `max(|x2-x1| + 2, |y2-y1| + 2)` — the `line.c` L191 "coord list overflow" guard, preserved
/// verbatim. It is mathematically unreachable for the CASE 1/2/3 walks (each yields at most
/// `max(|dx|, |dy|) + 1` points), but is kept to mirror stock bit-for-bit. The stock `malloc`
/// failures (`-410`/`-411`, `L100`/`L106`) have no analogue here — `Vec` allocation is infallible in
/// this context — and are elided.
// PORT: stock `line_points` returns `int` (0 / negative). Here `Ok(list)` replaces the three out
// parameters and `Err(code)` replaces the negative returns.
pub(crate) fn line_points(x1: i32, y1: i32, x2: i32, y2: i32) -> Result<Vec<(i32, i32)>, i32> {
    // PORT L95: maximum number of points needed to hold the segment (== stock malloc capacity).
    let asize = (i32::abs(x2 - x1) + 2).max(i32::abs(y2 - y1) + 2);

    // PORT L111–L112: delta x and y.
    let dx = x2 - x1;
    let dy = y2 - y1;

    // PORT L115–L123: per-axis step direction (`+1` for a non-negative delta, else `-1`).
    let x_incr = if dx >= 0 { 1 } else { -1 };
    let y_incr = if dy >= 0 { 1 } else { -1 };

    // PORT L126–L127: |DX| and |DY|.
    let adx = i32::abs(dx);
    let ady = i32::abs(dy);

    // PORT L130–L139: axis orientation. `inx` set when X dominates, `iny` when Y dominates; both 0
    // on the exact diagonal (|DX| == |DY|).
    let inx = i32::from(adx > ady);
    let iny = i32::from(ady > adx);

    // PORT L160–L161: gates selecting the integer vs. interpolated contribution per axis.
    let intx = 1 - iny;
    let inty = 1 - inx;

    // PORT L167: x_factor = (inx·x_incr) + (iny · DX / max(1, |DY|)).
    let x_factor =
        f64::from(inx * x_incr) + f64::from(iny) * (f64::from(dx) / f64::from(ady.max(1)));
    // PORT L173: y_factor = (iny·y_incr) + (inx · DY / max(1, |DX|)).
    let y_factor =
        f64::from(iny * y_incr) + f64::from(inx) * (f64::from(dy) / f64::from(adx.max(1)));

    // PORT L176–L180: integer and floating-point running coordinates seeded at the first point.
    let mut ix = x1;
    let mut iy = y1;
    let mut rx = f64::from(x1);
    let mut ry = f64::from(y1);

    // PORT L98/L183–L187: allocate to `asize` and assign the first point.
    let mut list: Vec<(i32, i32)> = Vec::with_capacity(asize.max(0) as usize);
    list.push((x1, y1));

    // PORT L189: walk until the integer coordinate reaches the second endpoint.
    while ix != x2 || iy != y2 {
        // PORT L191–L196: capacity guard (see the doc `Errors` note).
        if list.len() as i32 >= asize {
            return Err(-412);
        }

        // PORT L198–L199: accumulate the fractional step.
        rx += x_factor;
        ry += y_factor;

        // PORT L203–L204: quantize so the round below is architecture-independent.
        rx = trunc_dbl_precision(rx, TRUNC_SCALE);
        ry = trunc_dbl_precision(ry, TRUNC_SCALE);

        // PORT L208–L209: new pixel — driving axis by `±1`, interpolated axis by `(int)(r + 0.5)`.
        ix = intx * (ix + x_incr) + iny * ((rx + 0.5) as i32);
        iy = inty * (iy + y_incr) + inx * ((ry + 0.5) as i32);

        // PORT L212–L213: append the point.
        list.push((ix, iy));
    }

    // PORT L217–L222: structured return (out params + count collapse into `list`).
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_point_when_endpoints_coincide() {
        // Loop never runs: only the seed point is emitted.
        assert_eq!(line_points(4, 7, 4, 7), Ok(vec![(4, 7)]));
    }

    #[test]
    fn horizontal_line_steps_by_one_in_x() {
        assert_eq!(
            line_points(0, 2, 3, 2),
            Ok(vec![(0, 2), (1, 2), (2, 2), (3, 2)])
        );
    }

    #[test]
    fn vertical_line_steps_by_one_in_y() {
        assert_eq!(
            line_points(5, 0, 5, 3),
            Ok(vec![(5, 0), (5, 1), (5, 2), (5, 3)])
        );
    }

    #[test]
    fn exact_diagonal_steps_by_one_in_both() {
        // |DX| == |DY|: CASE 3, both axes driven by their increments.
        assert_eq!(
            line_points(0, 0, 3, 3),
            Ok(vec![(0, 0), (1, 1), (2, 2), (3, 3)])
        );
    }

    #[test]
    fn reversed_diagonal_walks_backwards() {
        assert_eq!(
            line_points(3, 3, 0, 0),
            Ok(vec![(3, 3), (2, 2), (1, 1), (0, 0)])
        );
    }

    #[test]
    fn x_dominant_interpolates_y() {
        // |DX| > |DY|: X drives by +1, Y interpolated via `(int)(ry + 0.5)`.
        // Hand-traced against line.c: y_factor = 0.5.
        assert_eq!(
            line_points(0, 0, 4, 2),
            Ok(vec![(0, 0), (1, 1), (2, 1), (3, 2), (4, 2)])
        );
    }

    #[test]
    fn endpoints_are_always_the_first_and_last_points() {
        for &(x1, y1, x2, y2) in &[(2, 3, 9, 5), (9, 5, 2, 3), (-1, -2, 4, 7), (7, 1, 1, 8)] {
            let pts = line_points(x1, y1, x2, y2).expect("no overflow for these segments");
            assert_eq!(pts.first(), Some(&(x1, y1)));
            assert_eq!(pts.last(), Some(&(x2, y2)));
        }
    }
}
