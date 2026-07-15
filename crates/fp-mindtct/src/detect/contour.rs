// SPDX-FileCopyrightText: NIST NBIS — U.S. Government work, public domain (title 17 §105)
//
// SPDX-License-Identifier: LicenseRef-NBIS-PD

//! Ridge/valley contour tracing around a candidate minutia point — faithful port of stock NBIS
//! `mindtct/src/lib/mindtct/contour.c`.
//!
//! A minutia "feature" is a run of interior pixels of one color (ridge = black = `1`, valley =
//! white = `0`); each interior *contour* pixel is paired with an adjacent *edge* pixel of the
//! opposite color that sits just outside the feature. [`trace_contour`] walks that interior edge one
//! 8-connected step at a time — [`next_contour_pixel`] scans the eight neighbors of the current
//! feature pixel, clockwise or counter-clockwise, for the next feature/edge pair, skipping "exposed"
//! corner pixels — collecting parallel lists of contour and edge coordinates. The higher-level
//! [`get_centered_contour`]/[`get_high_curvature_contour`] extract a fixed-length contour centered on
//! the feature point by tracing one half clockwise and the other counter-clockwise and concatenating
//! them, and [`min_contour_theta`] finds the point of highest curvature along such a contour.
//!
//! The 8-neighbor scan order, the clockwise/counter-clockwise rotation, and the corner-exposure rule
//! are reproduced verbatim from the reference so a traced contour is identical pixel-for-pixel.

// `too_many_arguments`: every public routine here is a verbatim transcription of a stock `contour.c`
// function whose interface is fixed by the reference — the binary image (`bdata`, `iw`, `ih`) plus
// two or three coordinate pairs (a feature point, its edge pixel, and sometimes a loop/search
// target). Bundling those into an ad-hoc struct would obscure the one-to-one correspondence with the
// C; the reference-side arity is the justification, so the lint is suppressed for the whole file.
#![allow(clippy::too_many_arguments)]

use crate::consts::TRUNC_SCALE;
use crate::num::{sround, trunc_dbl_precision};

// PORT: stock `nbr8_dx`/`nbr8_dy` (`globals.c` L245–L246) — per-axis offsets of the eight neighbors
// of a pixel, indexed 0..8 clockwise starting due north:
//
// ```text
//   7 0 1        NW N NE
//   6 . 2   ==    W . E
//   5 4 3        SW S SE
// ```
const NBR8_DX: [i32; 8] = [0, 1, 1, 1, 0, -1, -1, -1];
const NBR8_DY: [i32; 8] = [-1, -1, 0, 1, 1, 1, 0, -1];

// PORT: stock cardinal neighbor indices into `nbr8_dx`/`nbr8_dy` (`lfs.h` L667–L670). These name the
// four non-diagonal positions that [`start_scan_nbr`] can return.
const NORTH: i32 = 0;
const EAST: i32 = 2;
const SOUTH: i32 = 4;
const WEST: i32 = 6;

// PORT: stock `INVALID_DIR` (`lfs.h` L320) — returned by [`start_scan_nbr`] for an input that is
// neither N/S/E/W aligned. The reference notes this is unreachable for the aligned pairs the tracer
// feeds it ("Should never reach here. Added to remove compiler warning.").
const INVALID_DIR: i32 = -1;

// PORT: stock `MIN_SLOPE_DELTA` (`lfs.h` L701) — below this per-axis delta a line is treated as
// having zero slope in [`angle2line`], avoiding a degenerate `atan2(0, 0)`.
const MIN_SLOPE_DELTA: f64 = 0.5;

/// Direction in which the eight neighbors of a feature pixel are scanned for the next contour pixel —
/// port of the stock `scan_clock` flag (`SCAN_CLOCKWISE == 0` / `SCAN_COUNTER_CLOCKWISE == 1`,
/// `lfs.h` L496–L497). Modeled as an enum since it is only ever one of the two directions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScanDir {
    /// Advance the neighbor index one position clockwise (`SCAN_CLOCKWISE`).
    Clockwise,
    /// Advance the neighbor index one position counter-clockwise (`SCAN_COUNTER_CLOCKWISE`).
    CounterClockwise,
}

/// A traced feature contour — the port's analogue of stock's four parallel coordinate lists
/// (`contour_x`, `contour_y`, `contour_ex`, `contour_ey`) plus their shared count `ncontour`.
///
/// The first two vectors are the 8-connected *contour points* interior to the feature (guaranteed
/// adjacent and the feature's color); the second two are the corresponding *edge points* just
/// outside the feature (opposite color, not guaranteed 8-connected). All four are kept in lock-step,
/// so their shared length ([`len`](Self::len)) is the stock `ncontour`. Exposing `x`/`y` as slices
/// lets them feed [`min_contour_theta`] and the chain-code routines directly.
///
/// PORT: stock `allocate_contour`/`free_contour` (`contour.c` L105/L179) collapse into `Vec`
/// ownership; the `-180..=-183` malloc-failure paths are unreachable here (allocation aborts rather
/// than returning an error code) and are elided.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Contour {
    /// X coordinates of the interior contour points (stock `contour_x`).
    pub x: Vec<i32>,
    /// Y coordinates of the interior contour points (stock `contour_y`).
    pub y: Vec<i32>,
    /// X coordinates of the corresponding exterior edge points (stock `contour_ex`).
    pub ex: Vec<i32>,
    /// Y coordinates of the corresponding exterior edge points (stock `contour_ey`).
    pub ey: Vec<i32>,
}

impl Contour {
    /// An empty contour with room for `n` points reserved in each list.
    fn with_capacity(n: usize) -> Self {
        Contour {
            x: Vec::with_capacity(n),
            y: Vec::with_capacity(n),
            ex: Vec::with_capacity(n),
            ey: Vec::with_capacity(n),
        }
    }

    /// Number of contour points (stock `ncontour`); all four coordinate lists share this length.
    pub fn len(&self) -> usize {
        self.x.len()
    }

    /// Append one contour/edge point pair to the parallel lists.
    fn push(&mut self, x: i32, y: i32, ex: i32, ey: i32) {
        self.x.push(x);
        self.y.push(y);
        self.ex.push(ex);
        self.ey.push(ey);
    }

    /// Append `src`'s points in **reverse** order (last point first).
    ///
    /// A clockwise half-contour is collected outward from the feature point, so reversing it makes
    /// its far end the start of the concatenated contour — the stock `for(i, j=n-1; ...; j--)` copy.
    fn extend_rev(&mut self, src: &Contour) {
        let mut j = src.len();
        while j > 0 {
            j -= 1;
            self.push(src.x[j], src.y[j], src.ex[j], src.ey[j]);
        }
    }

    /// Append `src`'s points in **forward** order — the stock `for(i, j=n+1; ...; j++)` copy.
    fn extend_fwd(&mut self, src: &Contour) {
        let mut i = 0;
        while i < src.len() {
            self.push(src.x[i], src.y[i], src.ex[i], src.ey[i]);
            i += 1;
        }
    }
}

/// Outcome of [`trace_contour`] — the stock `int` return with its allocated buffers folded in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TraceResult {
    /// The trace completed normally (stock return `0`): a contour of up to `max_len` points, possibly
    /// cut short where no further contour pixel was found.
    Traced(Contour),
    /// The trace walked back around to the loop-trigger point (stock `LOOP_FOUND`); the contour holds
    /// the points collected up to — but not including — that point.
    Loop(Contour),
    /// The trace was impossible because the feature and edge pixels were not opposite colors (stock
    /// `IGNORE`); no contour is produced.
    Ignore,
}

/// Outcome of [`get_centered_contour`] — stock returns `0`/`LOOP_FOUND`/`IGNORE`/`INCOMPLETE`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CenteredContour {
    /// A complete, centered contour of `2 * half_contour + 1` points (stock return `0`).
    Ok(Contour),
    /// A loop was detected while tracing either half (stock `LOOP_FOUND`); no contour is returned.
    Loop,
    /// A half-trace was impossible due to the starting conditions (stock `IGNORE`).
    Ignore,
    /// A half-contour could not reach the requested length (stock `INCOMPLETE`).
    Incomplete,
}

/// Outcome of [`get_high_curvature_contour`] — stock returns `0` (with an empty *or* full contour)
/// or `LOOP_FOUND` (with a contour).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HighCurvatureContour {
    /// The trace could not extract a full-length contour (stock return `0` with `oncontour == 0`);
    /// nothing is returned.
    Empty,
    /// A complete contour of `2 * half_contour + 1` points (stock return `0` with a contour).
    Ok(Contour),
    /// The contour forms a complete loop (stock `LOOP_FOUND`); the loop's contour is returned for
    /// further processing.
    Loop(Contour),
}

/// Compute the angle (radians) of the line from `(fx, fy)` to `(tx, ty)` — port of stock
/// `angle2line` (`util.c` L519).
///
/// PORT: `angle2line` lives in stock `util.c`, not `contour.c`; it is defined here as a file-private
/// helper because [`min_contour_theta`] is its only port-side consumer so far. Relocate to a shared
/// `util` module when that file is ported. The slope is measured as `dy = fy - ty`, `dx = tx - fx`
/// (note the asymmetry, verbatim from the reference); when both deltas fall below [`MIN_SLOPE_DELTA`]
/// the angle is defined as `0.0`.
fn angle2line(fx: i32, fy: i32, tx: i32, ty: i32) -> f64 {
    // PORT L524–L525: slope components (the reference's mixed subtraction order is intentional).
    let dy = f64::from(fy - ty);
    let dx = f64::from(tx - fx);
    // PORT L527–L532: sufficiently flat → 0.0, else the arctangent of the slope.
    if dx.abs() < MIN_SLOPE_DELTA && dy.abs() < MIN_SLOPE_DELTA {
        0.0
    } else {
        dy.atan2(dx)
    }
}

/// Convert the line connecting two points to an integer direction on the full circle — port of stock
/// `line2direction` (`util.c` L552).
///
/// The point coordinates are swapped and their order reversed (as in the reference) so that direction
/// `0` is vertical and positive directions run clockwise. The angle from [`angle2line`] is made
/// positive, converted from radians to an integer direction on `0..=2*ndirs` (`full_ndirs` units per
/// circle), quantized through [`trunc_dbl_precision`]/[`sround`] for architecture-independent
/// rounding, and reduced onto the range `0..2*ndirs`.
///
/// PORT: `line2direction` lives in stock `util.c`; it is defined here as a `pub(crate)` helper (beside
/// [`angle2line`], its only dependency) because the detection stage
/// ([`adjust_high_curvature_minutia_v2`](super::adjust_high_curvature_minutia_v2) and
/// [`process_loop_v2`](super::loops::process_loop_v2)) is its only consumer. Relocate to a shared
/// `util` module when that file is ported. `fmod` becomes `rem_euclid` (the argument is always
/// positive here, so the two agree); the trailing `%` likewise stays non-negative.
pub(crate) fn line2direction(fx: i32, fy: i32, tx: i32, ty: i32, ndirs: i32) -> i32 {
    // PORT L557: static `pi2 = 2*PI`.
    let pi2 = std::f64::consts::PI * 2.0;

    // PORT L563: coordinates swapped and points reversed so 0 is vertical, positive is clockwise.
    let mut theta = angle2line(ty, tx, fy, fx);

    // PORT L566–L567: make the angle positive and fold onto `[0, 2*PI)`.
    theta += pi2;
    theta = theta.rem_euclid(pi2);

    // PORT L572–L576: radians → integer-direction units on the full circle.
    let full_ndirs = ndirs << 1;
    let pi_factor = f64::from(full_ndirs) / pi2;
    theta *= pi_factor;

    // PORT L579–L581: quantize precision, then round to the nearest integer direction.
    theta = trunc_dbl_precision(theta, TRUNC_SCALE);
    let idir = sround(theta);

    // PORT L583: reduce onto `[0..2*ndirs]` (always non-negative here).
    idir.rem_euclid(full_ndirs)
}

/// Position of the second pixel relative to the first for an N/S/E/W-aligned pair — port of stock
/// `start_scan_nbr` (`contour.c` L1020).
///
/// Returns the [`NORTH`]/[`SOUTH`]/[`EAST`]/[`WEST`] neighbor index of `(x_next, y_next)` as seen
/// from `(x_prev, y_prev)`, or [`INVALID_DIR`] for a non-aligned (e.g. diagonal) pair. This does not
/// account for diagonal positions — the tracer guarantees the edge pixel is cardinally adjacent to
/// the feature pixel (see [`fix_edge_pixel_pair`]).
fn start_scan_nbr(x_prev: i32, y_prev: i32, x_next: i32, y_next: i32) -> i32 {
    // PORT L1023–L1030: cardinal-direction dispatch.
    if x_prev == x_next && y_next > y_prev {
        SOUTH
    } else if x_prev == x_next && y_next < y_prev {
        NORTH
    } else if x_next > x_prev && y_prev == y_next {
        EAST
    } else if x_next < x_prev && y_prev == y_next {
        WEST
    } else {
        // PORT L1032–L1034: unreachable for aligned inputs; present to mirror the reference.
        INVALID_DIR
    }
}

/// Advance an 8-connected neighbor index one position in the given scan direction — port of stock
/// `next_scan_nbr` (`contour.c` L1049).
///
/// Clockwise adds one modulo 8; counter-clockwise adds seven modulo 8 (a wrapping decrement).
fn next_scan_nbr(nbr_i: i32, scan: ScanDir) -> i32 {
    // PORT L1053–L1063: `+1 % 8` clockwise, `+7 % 8` counter-clockwise.
    match scan {
        ScanDir::Clockwise => (nbr_i + 1) % 8,
        ScanDir::CounterClockwise => (nbr_i + 7) % 8,
    }
}

/// Locate the next pixel on a feature's interior contour — port of stock `next_contour_pixel`
/// (`contour.c` L876).
///
/// Scans the eight neighbors of the current feature pixel `(cur_x_loc, cur_y_loc)` in the `scan`
/// direction, starting from the current edge pixel's position, looking for the first adjacent pair
/// whose leading pixel has the feature's color and whose trailing (previous) pixel has the edge's
/// color. That pair becomes the next contour point (returned) and edge point. An "exposed" corner —
/// a diagonal neighbor whose own next neighbor is *not* the feature color — is skipped so the walk
/// stays on the 8-connected interior edge.
///
/// Returns `Some((next_x_loc, next_y_loc, next_x_edge, next_y_edge))` on success (stock `TRUE`), or
/// `None` (stock `FALSE`) when a neighbor falls outside the image or no valid pair exists among the
/// eight neighbors (an isolated pixel).
fn next_contour_pixel(
    cur_x_loc: i32,
    cur_y_loc: i32,
    cur_x_edge: i32,
    cur_y_edge: i32,
    scan: ScanDir,
    bdata: &[u8],
    iw: i32,
    ih: i32,
) -> Option<(i32, i32, i32, i32)> {
    // PORT L890–L892: the feature pixel's value and its edge pixel's value.
    let feature_pix = bdata[(cur_y_loc * iw + cur_x_loc) as usize];
    let edge_pix = bdata[(cur_y_edge * iw + cur_x_edge) as usize];

    // PORT L901: neighbor index of the edge pixel relative to the feature pixel — the scan seed.
    let mut nbr_i = start_scan_nbr(cur_x_loc, cur_y_loc, cur_x_edge, cur_y_edge);

    // PORT L904–L906: seed the "current" scan neighbor at the feature's edge pixel.
    let mut cur_nbr_x = cur_x_edge;
    let mut cur_nbr_y = cur_y_edge;
    let mut cur_nbr_pix = edge_pix;

    // PORT L909: scan the (up to) eight neighbors of the feature pixel.
    let mut i = 0;
    while i < 8 {
        // PORT L912–L914: the just-scanned neighbor becomes the "previous" one.
        let prev_nbr_x = cur_nbr_x;
        let prev_nbr_y = cur_nbr_y;
        let prev_nbr_pix = cur_nbr_pix;

        // PORT L917: step the neighbor index clockwise / counter-clockwise.
        nbr_i = next_scan_nbr(nbr_i, scan);

        // PORT L922–L923: the new neighbor's coordinates, around the feature point.
        cur_nbr_x = cur_x_loc + NBR8_DX[nbr_i as usize];
        cur_nbr_y = cur_y_loc + NBR8_DY[nbr_i as usize];

        // PORT L926–L929: a neighbor outside the image ends the trace (failure).
        if cur_nbr_x < 0 || cur_nbr_x >= iw || cur_nbr_y < 0 || cur_nbr_y >= ih {
            return None;
        }

        // PORT L932: the new neighbor's pixel value.
        cur_nbr_pix = bdata[(cur_nbr_y * iw + cur_nbr_x) as usize];

        // PORT L938: a feature-colored pixel preceded by an edge-colored one is a candidate.
        if cur_nbr_pix == feature_pix && prev_nbr_pix == edge_pix {
            // PORT L943: corners sit at the odd neighbor indices; test them for exposure.
            if nbr_i % 2 != 0 {
                // PORT L945–L947: look ahead one more neighbor.
                let ni = next_scan_nbr(nbr_i, scan);
                let nx = cur_x_loc + NBR8_DX[ni as usize];
                let ny = cur_y_loc + NBR8_DY[ni as usize];
                // PORT L949–L952: look-ahead neighbor out of bounds ends the trace (failure).
                if nx < 0 || nx >= iw || ny < 0 || ny >= ih {
                    return None;
                }
                let npix = bdata[(ny * iw + nx) as usize];

                // PORT L957: the corner is NOT exposed → accept it as the next contour point.
                if npix == feature_pix {
                    return Some((cur_nbr_x, cur_nbr_y, prev_nbr_x, prev_nbr_y));
                }
                // PORT L966–L978: exposed corner → skip it by adopting the look-ahead neighbor and
                // advancing both the neighbor index and the scan count an extra step.
                cur_nbr_x = nx;
                cur_nbr_y = ny;
                cur_nbr_pix = npix;
                nbr_i = ni;
                i += 1;
            }
            // PORT L980–L989: non-corner neighbor → accept it as the next contour point.
            else {
                return Some((cur_nbr_x, cur_nbr_y, prev_nbr_x, prev_nbr_y));
            }
        }

        i += 1;
    }

    // PORT L998: no valid pair among the eight neighbors (an isolated pixel) → failure.
    None
}

/// Trace a feature's interior contour up to `max_len` points in one direction — port of stock
/// `trace_contour` (`contour.c` L653).
///
/// Starts at feature point `(x_loc, y_loc)` with adjacent edge pixel `(x_edge, y_edge)` and walks the
/// contour in the `scan` direction, one [`next_contour_pixel`] step at a time. If the walk reaches
/// the loop-trigger point `(x_loop, y_loop)`, it stops and reports a [`TraceResult::Loop`] — passing
/// that point independently lets successive half-traces from a common start detect a loop spanning
/// both. `bdata` is the binary image (`0 == white/valley`, `1 == black/ridge`), `iw`×`ih` pixels.
pub(crate) fn trace_contour(
    max_len: i32,
    x_loop: i32,
    y_loop: i32,
    x_loc: i32,
    y_loc: i32,
    x_edge: i32,
    y_edge: i32,
    scan: ScanDir,
    bdata: &[u8],
    iw: i32,
    ih: i32,
) -> TraceResult {
    // PORT L669–L672: the feature and edge pixels must be opposite colors, else the trace can't work.
    if bdata[(y_loc * iw + x_loc) as usize] == bdata[(y_edge * iw + x_edge) as usize] {
        return TraceResult::Ignore;
    }

    // PORT L675/L682: allocate the contour buffers and zero the point counter (implicit in `Vec`).
    let mut contour = Contour::with_capacity(max_len.max(0) as usize);

    // PORT L685–L688: seed the "current" feature/edge pair at the starting point.
    let mut cur_x_loc = x_loc;
    let mut cur_y_loc = y_loc;
    let mut cur_x_edge = x_edge;
    let mut cur_y_edge = y_edge;

    // PORT L691: collect up to `max_len` contour points.
    let mut i = 0;
    while i < max_len {
        // PORT L693–L697: find the next contour pixel.
        if let Some((next_x_loc, next_y_loc, next_x_edge, next_y_edge)) = next_contour_pixel(
            cur_x_loc, cur_y_loc, cur_x_edge, cur_y_edge, scan, bdata, iw, ih,
        ) {
            // PORT L701–L710: reached the loop-trigger point → return the contour so far as a loop.
            if next_x_loc == x_loop && next_y_loc == y_loop {
                return TraceResult::Loop(contour);
            }

            // PORT L714–L719: store the new contour point.
            contour.push(next_x_loc, next_y_loc, next_x_edge, next_y_edge);

            // PORT L722–L725: advance the current pair.
            cur_x_loc = next_x_loc;
            cur_y_loc = next_y_loc;
            cur_x_edge = next_x_edge;
            cur_y_edge = next_y_edge;
        } else {
            // PORT L728–L738: no further contour pixel → stop short, returning what was found.
            return TraceResult::Traced(contour);
        }

        i += 1;
    }

    // PORT L745–L752: collected the full `max_len` points.
    TraceResult::Traced(contour)
}

/// Walk a feature's contour looking for a specific point — port of stock `search_contour`
/// (`contour.c` L787).
///
/// Steps up to `search_len` points along the contour from `(x_loc, y_loc)`/`(x_edge, y_edge)` in the
/// `scan` direction, returning `true` (stock `FOUND`) the moment the traced contour point equals
/// `(x_search, y_search)`, or `false` (stock `NOT_FOUND`) if the point is not met within
/// `search_len` steps or the trace terminates early.
pub(crate) fn search_contour(
    x_search: i32,
    y_search: i32,
    search_len: i32,
    x_loc: i32,
    y_loc: i32,
    x_edge: i32,
    y_edge: i32,
    scan: ScanDir,
    bdata: &[u8],
    iw: i32,
    ih: i32,
) -> bool {
    // PORT L801–L804: seed the current feature/edge pair.
    let mut cur_x_loc = x_loc;
    let mut cur_y_loc = y_loc;
    let mut cur_x_edge = x_edge;
    let mut cur_y_edge = y_edge;

    // PORT L807: walk up to `search_len` contour points.
    let mut i = 0;
    while i < search_len {
        // PORT L809–L813: find the next contour pixel.
        if let Some((next_x_loc, next_y_loc, next_x_edge, next_y_edge)) = next_contour_pixel(
            cur_x_loc, cur_y_loc, cur_x_edge, cur_y_edge, scan, bdata, iw, ih,
        ) {
            // PORT L816–L819: found the searched-for point.
            if next_x_loc == x_search && next_y_loc == y_search {
                return true;
            }

            // PORT L821–L825: advance the current pair.
            cur_x_loc = next_x_loc;
            cur_y_loc = next_y_loc;
            cur_x_edge = next_x_edge;
            cur_y_edge = next_y_edge;
        } else {
            // PORT L828–L831: trace ended early → not found.
            return false;
        }

        i += 1;
    }

    // PORT L836: exhausted the search length without finding the point.
    false
}

/// Extract a fixed-length contour centered on a feature point — port of stock `get_centered_contour`
/// (`contour.c` L457).
///
/// Traces one half clockwise and the other counter-clockwise (each up to `half_contour` points),
/// using the far end of the first half as the second half's loop-trigger. On success both halves are
/// concatenated — first half reversed (far end first), then the feature point, then the second half
/// forward — into a contour of `2 * half_contour + 1` points ([`CenteredContour::Ok`]). Any loop,
/// impossible trace, or short half yields the corresponding non-`Ok` variant with no contour.
pub(crate) fn get_centered_contour(
    half_contour: i32,
    x_loc: i32,
    y_loc: i32,
    x_edge: i32,
    y_edge: i32,
    bdata: &[u8],
    iw: i32,
    ih: i32,
) -> CenteredContour {
    // PORT L478–L480: first half contour, traced clockwise from the feature point.
    let half1 = match trace_contour(
        half_contour,
        x_loc,
        y_loc,
        x_loc,
        y_loc,
        x_edge,
        y_edge,
        ScanDir::Clockwise,
        bdata,
        iw,
        ih,
    ) {
        // PORT L489–L491: trace impossible.
        TraceResult::Ignore => return CenteredContour::Ignore,
        // PORT L494–L499: first half looped → discard and report.
        TraceResult::Loop(_) => return CenteredContour::Loop,
        TraceResult::Traced(c) => c,
    };

    // PORT L502–L507: first half must reach the requested length.
    if (half1.len() as i32) < half_contour {
        return CenteredContour::Incomplete;
    }

    // PORT L513–L516: second half, traced counter-clockwise; the first half's far end is the
    // loop-trigger so a loop spanning both halves is detected.
    let last = half1.len() - 1;
    let half2 = match trace_contour(
        half_contour,
        half1.x[last],
        half1.y[last],
        x_loc,
        y_loc,
        x_edge,
        y_edge,
        ScanDir::CounterClockwise,
        bdata,
        iw,
        ih,
    ) {
        // PORT L525–L530: second trace impossible.
        TraceResult::Ignore => return CenteredContour::Ignore,
        // PORT L533–L539: second half looped → discard and report.
        TraceResult::Loop(_) => return CenteredContour::Loop,
        TraceResult::Traced(c) => c,
    };

    // PORT L542–L548: second half must reach the requested length.
    if (half2.len() as i32) < half_contour {
        return CenteredContour::Incomplete;
    }

    // PORT L555/L573–L598: concatenate — first half reversed, feature point, second half forward.
    let max_contour = (half_contour << 1) + 1;
    let mut contour = Contour::with_capacity(max_contour.max(0) as usize);
    contour.extend_rev(&half1);
    contour.push(x_loc, y_loc, x_edge, y_edge);
    contour.extend_fwd(&half2);

    // PORT L603–L611: return the centered contour.
    CenteredContour::Ok(contour)
}

/// Extract a fixed-length contour of a high-curvature feature edge — port of stock
/// `get_high_curvature_contour` (`contour.c` L228).
///
/// Like [`get_centered_contour`] but tolerant of loops: if the first half forms a loop the loop's
/// contour (feature point followed by the first half reversed) is returned as
/// [`HighCurvatureContour::Loop`]; if the second half loops the two halves are still concatenated and
/// returned as a loop. A full non-loop trace yields [`HighCurvatureContour::Ok`]; a trace that cannot
/// reach full length yields [`HighCurvatureContour::Empty`] (no contour).
pub(crate) fn get_high_curvature_contour(
    half_contour: i32,
    x_loc: i32,
    y_loc: i32,
    x_edge: i32,
    y_edge: i32,
    bdata: &[u8],
    iw: i32,
    ih: i32,
) -> HighCurvatureContour {
    // PORT L243: maximum full contour length (two halves + the feature point).
    let max_contour = (half_contour << 1) + 1;

    // PORT L249–L251: first half contour, traced clockwise from the feature point.
    let half1 = match trace_contour(
        half_contour,
        x_loc,
        y_loc,
        x_loc,
        y_loc,
        x_edge,
        y_edge,
        ScanDir::Clockwise,
        bdata,
        iw,
        ih,
    ) {
        // PORT L254–L256: trace impossible → empty result.
        TraceResult::Ignore => return HighCurvatureContour::Empty,
        // PORT L259–L301: first half looped → return the feature point then the half reversed.
        TraceResult::Loop(loop_half) => {
            let mut contour = Contour::with_capacity(loop_half.len() + 1);
            contour.push(x_loc, y_loc, x_edge, y_edge);
            contour.extend_rev(&loop_half);
            return HighCurvatureContour::Loop(contour);
        }
        TraceResult::Traced(c) => c,
    };

    // PORT L310–L315: first half not complete → empty result.
    if (half1.len() as i32) < half_contour {
        return HighCurvatureContour::Empty;
    }

    // PORT L321–L324: second half, traced counter-clockwise, with the first half's far end as the
    // loop-trigger. `looped` records whether it closed a loop (stock's `ret == LOOP_FOUND`).
    let last = half1.len() - 1;
    let (half2, looped) = match trace_contour(
        half_contour,
        half1.x[last],
        half1.y[last],
        x_loc,
        y_loc,
        x_edge,
        y_edge,
        ScanDir::CounterClockwise,
        bdata,
        iw,
        ih,
    ) {
        // PORT L327–L332: second trace impossible → empty result.
        TraceResult::Ignore => return HighCurvatureContour::Empty,
        TraceResult::Loop(c) => (c, true),
        TraceResult::Traced(c) => (c, false),
    };

    // PORT L344–L350: a non-loop second half that is too short → empty result.
    if !looped && (half2.len() as i32) < half_contour {
        return HighCurvatureContour::Empty;
    }

    // PORT L359/L377–L402: concatenate — first half reversed, feature point, second half forward.
    let mut contour = Contour::with_capacity(max_contour.max(0) as usize);
    contour.extend_rev(&half1);
    contour.push(x_loc, y_loc, x_edge, y_edge);
    contour.extend_fwd(&half2);

    // PORT L414–L416: a looping second half returns a loop, otherwise a complete contour.
    if looped {
        HighCurvatureContour::Loop(contour)
    } else {
        HighCurvatureContour::Ok(contour)
    }
}

/// Locate the point of highest curvature (minimum interior angle) along a contour — port of stock
/// `min_contour_theta` (`contour.c` L1097).
///
/// At each interior point a left and right segment extend `angle_edge` points either side; the angle
/// between the center→left and center→right lines is measured (via [`angle2line`]), reduced to its
/// inner value `min(dθ, 2π − dθ)`, and quantized through [`trunc_dbl_precision`] at [`TRUNC_SCALE`]
/// so the comparison is architecture-independent. The smallest such angle and its center index are
/// returned; a perfectly flat contour (no strict minimum) reports the contour's midpoint.
///
/// Returns `None` (stock `IGNORE`) when the contour is shorter than `2 * angle_edge + 1` points, else
/// `Some((min_i, min_theta))`.
pub(crate) fn min_contour_theta(
    angle_edge: i32,
    contour_x: &[i32],
    contour_y: &[i32],
) -> Option<(i32, f64)> {
    let ncontour = contour_x.len() as i32;

    // PORT L1107–L1109: too short to analyze → ignore.
    if ncontour < (angle_edge << 1) + 1 {
        return None;
    }

    // PORT L1112–L1116: running minimum seeded at π (quantized for reproducibility).
    let mut min_theta = trunc_dbl_precision(std::f64::consts::PI, TRUNC_SCALE);
    // PORT L1117: no minimum found yet.
    let mut min_i: i32 = -1;

    // PORT L1119–L1123: left / center / right sample indices, `angle_edge` apart.
    let mut pleft = 0;
    let mut pcenter = angle_edge;
    let mut pright = pcenter + angle_edge;

    // PORT L1126: slide the triple along the contour until the right point runs off the end.
    while pright < ncontour {
        // PORT L1128–L1132: angles from the center point to the left and right sample points.
        let theta1 = angle2line(
            contour_x[pcenter as usize],
            contour_y[pcenter as usize],
            contour_x[pleft as usize],
            contour_y[pleft as usize],
        );
        let theta2 = angle2line(
            contour_x[pcenter as usize],
            contour_y[pcenter as usize],
            contour_x[pright as usize],
            contour_y[pright as usize],
        );

        // PORT L1136–L1140: inner angle between them, quantized.
        let mut dtheta = (theta2 - theta1).abs();
        dtheta = dtheta.min(std::f64::consts::PI * 2.0 - dtheta);
        dtheta = trunc_dbl_precision(dtheta, TRUNC_SCALE);

        // PORT L1143–L1146: track the running minimum.
        if dtheta < min_theta {
            min_i = pcenter;
            min_theta = dtheta;
        }

        // PORT L1149–L1151: advance all three sample points.
        pleft += 1;
        pcenter += 1;
        pright += 1;
    }

    // PORT L1156–L1164: a flat contour (no strict minimum) reports its midpoint.
    if min_i == -1 {
        Some((ncontour >> 1, min_theta))
    } else {
        Some((min_i, min_theta))
    }
}

/// Bounding box of a contour's interior points — port of stock `contour_limits` (`contour.c` L1185).
///
/// Returns `(xmin, ymin, xmax, ymax)`. As in the reference (which calls `minv`/`maxv`), the contour
/// is assumed non-empty.
///
/// # Panics
///
/// Panics on an empty contour, matching the stock precondition ("The list is assumed to be NOT
/// empty").
pub(crate) fn contour_limits(contour_x: &[i32], contour_y: &[i32]) -> (i32, i32, i32, i32) {
    // PORT L1189–L1195: per-axis minima and maxima (stock `minv`/`maxv`).
    let xmin = *contour_x.iter().min().expect("contour is non-empty");
    let ymin = *contour_y.iter().min().expect("contour is non-empty");
    let xmax = *contour_x.iter().max().expect("contour is non-empty");
    let ymax = *contour_y.iter().max().expect("contour is non-empty");
    (xmin, ymin, xmax, ymax)
}

/// Adjust a diagonal feature/edge pixel pair to a cardinal one — port of stock `fix_edge_pixel_pair`
/// (`contour.c` L1222).
///
/// The contour tracer requires the edge pixel to sit N/S/E/W of the feature pixel. If the supplied
/// pair are diagonal neighbors, one of the two intervening pixels (both retaining the required
/// colors) is chosen so the resulting pair is cardinally adjacent: the edge pixel is shifted onto
/// whichever intervening pixel is *not* the feature's color, or — if both are — the feature pixel
/// itself is shifted onto the diagonal (an exposed corner). Non-diagonal pairs are returned
/// unchanged. Returns the adjusted `(feat_x, feat_y, edge_x, edge_y)`.
///
// PORT: stock takes `ih` but never reads it (only `iw` indexes `bdata`), so it is dropped here. The
// four in/out pointer parameters become one returned tuple. As in the reference, the `p1`/`p2`
// look-ups are not bounds-checked — callers supply an interior feature pixel whose diagonal
// intervening pixels lie within the image.
pub(crate) fn fix_edge_pixel_pair(
    feat_x: i32,
    feat_y: i32,
    edge_x: i32,
    edge_y: i32,
    bdata: &[u8],
    iw: i32,
) -> (i32, i32, i32, i32) {
    // PORT L1230: the feature pixel's color — the value the retained pixels must match.
    let feature_pix = bdata[(feat_y * iw + feat_x) as usize];

    // PORT L1233–L1236: current (feature) and previous (edge) points.
    let cx = feat_x;
    let mut cy = feat_y;
    let mut px = edge_x;
    let mut py = edge_y;

    // PORT L1239–L1240: deltas from feature to edge.
    let dx = px - cx;
    let dy = py - cy;

    // PORT L1249: only diagonal neighbors need fixing.
    if dx.abs() == 1 && dy.abs() == 1 {
        // PORT L1272–L1273: if the pixel where x changes (p1 = px-dx, py) is NOT the feature color,
        // move the edge x-coord onto it.
        if bdata[(py * iw + (px - dx)) as usize] != feature_pix {
            px -= dx;
        }
        // PORT L1276–L1278: else if the pixel where y changes (p2 = px, py-dy) is NOT the feature
        // color, move the edge y-coord onto it.
        else if bdata[((py - dy) * iw + px) as usize] != feature_pix {
            py -= dy;
        }
        // PORT L1279–L1284: else the feature pixel is exposed on a corner — shift it onto the
        // diagonal (which carries the feature color).
        else {
            cy += dy;
        }
    }

    // PORT L1287–L1290 (and the unchanged non-diagonal case): the resulting pair.
    (cx, cy, px, py)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 5×5 image whose bottom three rows (`y >= 2`) are ridge (`1`) and top two rows are valley
    /// (`0`) — a straight horizontal feature edge along `y == 2`.
    fn half_plane() -> (Vec<u8>, i32, i32) {
        let iw = 5;
        let ih = 5;
        let mut b = vec![0u8; (iw * ih) as usize];
        for y in 2..ih {
            for x in 0..iw {
                b[(y * iw + x) as usize] = 1;
            }
        }
        (b, iw, ih)
    }

    /// A 5×5 image with a solid 3×3 ridge block at `x,y ∈ [1,3]`, valley elsewhere — a closed
    /// feature whose interior edge is an eight-pixel loop.
    fn block_3x3() -> (Vec<u8>, i32, i32) {
        let iw = 5;
        let ih = 5;
        let mut b = vec![0u8; (iw * ih) as usize];
        for y in 1..=3 {
            for x in 1..=3 {
                b[(y * iw + x) as usize] = 1;
            }
        }
        (b, iw, ih)
    }

    #[test]
    fn scan_nbr_cardinal_directions() {
        // Second pixel due S / N / E / W of the first.
        assert_eq!(start_scan_nbr(2, 2, 2, 3), SOUTH);
        assert_eq!(start_scan_nbr(2, 2, 2, 1), NORTH);
        assert_eq!(start_scan_nbr(2, 2, 3, 2), EAST);
        assert_eq!(start_scan_nbr(2, 2, 1, 2), WEST);
        // A diagonal pair is not aligned.
        assert_eq!(start_scan_nbr(2, 2, 3, 3), INVALID_DIR);
    }

    #[test]
    fn next_scan_nbr_wraps_both_ways() {
        assert_eq!(next_scan_nbr(0, ScanDir::Clockwise), 1);
        assert_eq!(next_scan_nbr(7, ScanDir::Clockwise), 0);
        assert_eq!(next_scan_nbr(0, ScanDir::CounterClockwise), 7);
        assert_eq!(next_scan_nbr(7, ScanDir::CounterClockwise), 6);
    }

    #[test]
    fn next_contour_pixel_walks_along_a_straight_edge() {
        let (b, iw, ih) = half_plane();
        // Feature (2,2) with edge due north (2,1): clockwise steps east, counter-clockwise west.
        assert_eq!(
            next_contour_pixel(2, 2, 2, 1, ScanDir::Clockwise, &b, iw, ih),
            Some((3, 2, 3, 1))
        );
        assert_eq!(
            next_contour_pixel(2, 2, 2, 1, ScanDir::CounterClockwise, &b, iw, ih),
            Some((1, 2, 1, 1))
        );
    }

    #[test]
    fn next_contour_pixel_none_for_isolated_pixel() {
        let iw = 5;
        let ih = 5;
        let mut b = vec![0u8; (iw * ih) as usize];
        b[(2 * iw + 2) as usize] = 1; // a single ridge pixel surrounded by valley
        assert_eq!(
            next_contour_pixel(2, 2, 2, 1, ScanDir::Clockwise, &b, iw, ih),
            None
        );
    }

    #[test]
    fn trace_contour_ignores_same_colored_pair() {
        let (b, iw, ih) = half_plane();
        // Both (2,2) and (2,3) are ridge → not opposite → IGNORE.
        assert_eq!(
            trace_contour(4, 2, 2, 2, 2, 2, 3, ScanDir::Clockwise, &b, iw, ih),
            TraceResult::Ignore
        );
    }

    #[test]
    fn trace_contour_stops_short_at_image_edge() {
        let (b, iw, ih) = half_plane();
        // Walk east along the edge; it runs off the right side after two points.
        let res = trace_contour(4, 2, 2, 2, 2, 2, 1, ScanDir::Clockwise, &b, iw, ih);
        let TraceResult::Traced(c) = res else {
            panic!("expected a short traced contour, got {res:?}");
        };
        assert_eq!(c.x, vec![3, 4]);
        assert_eq!(c.y, vec![2, 2]);
        assert_eq!(c.ex, vec![3, 4]);
        assert_eq!(c.ey, vec![1, 1]);
    }

    #[test]
    fn trace_contour_detects_a_loop_around_a_block() {
        let (b, iw, ih) = block_3x3();
        // Start at the block's top-left interior pixel (1,1) with its north edge (1,0); the trace
        // walks clockwise around the eight-pixel interior edge back to (1,1).
        let res = trace_contour(20, 1, 1, 1, 1, 1, 0, ScanDir::Clockwise, &b, iw, ih);
        let TraceResult::Loop(c) = res else {
            panic!("expected a loop, got {res:?}");
        };
        // Seven points collected before stepping back onto the start point.
        assert_eq!(c.x, vec![2, 3, 3, 3, 2, 1, 1]);
        assert_eq!(c.y, vec![1, 1, 2, 3, 3, 3, 2]);
    }

    #[test]
    fn search_contour_finds_and_misses() {
        let (b, iw, ih) = block_3x3();
        // (3,1) is two clockwise steps from the start and IS on the contour.
        assert!(search_contour(
            3,
            1,
            8,
            1,
            1,
            1,
            0,
            ScanDir::Clockwise,
            &b,
            iw,
            ih
        ));
        // (4,4) is off the feature entirely and is never met.
        assert!(!search_contour(
            4,
            4,
            8,
            1,
            1,
            1,
            0,
            ScanDir::Clockwise,
            &b,
            iw,
            ih
        ));
    }

    #[test]
    fn angle2line_flat_and_axis_aligned() {
        // Coincident points → both deltas below MIN_SLOPE_DELTA → 0.0.
        assert_eq!(angle2line(3, 3, 3, 3), 0.0);
        // Horizontal to the right: dy = 0, dx = +1 → atan2(0, 1) == 0.
        assert_eq!(angle2line(0, 0, 1, 0), 0.0);
        // Straight up in image coords (to smaller y): dy = fy-ty = 0-(-1)? use (0,0)->(0,1):
        // dy = 0-1 = -1, dx = 0 → atan2(-1, 0) == -π/2.
        assert!((angle2line(0, 0, 0, 1) + std::f64::consts::FRAC_PI_2).abs() < 1e-12);
    }

    #[test]
    fn min_contour_theta_ignores_short_contours() {
        // ncontour (2) < 2*angle_edge+1 (3) → IGNORE.
        assert_eq!(min_contour_theta(1, &[0, 1], &[0, 0]), None);
    }

    #[test]
    fn min_contour_theta_flat_reports_midpoint() {
        // A collinear contour has a constant angle of π everywhere → no strict minimum → midpoint.
        let x = [0, 1, 2, 3, 4];
        let y = [0, 0, 0, 0, 0];
        let expected_theta = trunc_dbl_precision(std::f64::consts::PI, TRUNC_SCALE);
        assert_eq!(min_contour_theta(1, &x, &y), Some((2, expected_theta)));
    }

    #[test]
    fn min_contour_theta_finds_right_angle() {
        // A 90-degree turn at index 1: left (0,0), center (1,0), right (1,1) → inner angle π/2.
        let x = [0, 1, 1];
        let y = [0, 0, 1];
        let expected_theta = trunc_dbl_precision(std::f64::consts::FRAC_PI_2, TRUNC_SCALE);
        assert_eq!(min_contour_theta(1, &x, &y), Some((1, expected_theta)));
    }

    #[test]
    fn contour_limits_bounds_the_points() {
        let x = [3, 1, 4, 1, 5];
        let y = [2, 7, 1, 8, 2];
        assert_eq!(contour_limits(&x, &y), (1, 1, 5, 8));
    }

    #[test]
    fn fix_edge_pixel_pair_leaves_cardinal_pairs_untouched() {
        let (b, iw, _) = half_plane();
        // Edge already due north of the feature → unchanged.
        assert_eq!(fix_edge_pixel_pair(2, 2, 2, 1, &b, iw), (2, 2, 2, 1));
    }

    #[test]
    fn fix_edge_pixel_pair_shifts_edge_off_the_diagonal() {
        // Lone ridge pixel at (2,2); the diagonal edge (3,1) is valley. p1 = (px-dx, py) = (2,1) is
        // valley (≠ feature) → the edge x-coord moves onto it, giving a due-north pair.
        let iw = 5;
        let ih = 5;
        let mut b = vec![0u8; (iw * ih) as usize];
        b[(2 * iw + 2) as usize] = 1;
        assert_eq!(fix_edge_pixel_pair(2, 2, 3, 1, &b, iw), (2, 2, 2, 1));
        let _ = ih;
    }

    #[test]
    fn fix_edge_pixel_pair_shifts_feature_on_exposed_corner() {
        // Feature (2,2)=1 with a diagonal edge at (3,1). Both intervening pixels p1=(2,1) and
        // p2=(3,2) are ridge (== feature), so the feature pixel itself shifts by dy onto the
        // diagonal: cy += dy = 2 + (-1) = 1.
        let iw = 5;
        let ih = 5;
        let mut b = vec![0u8; (iw * ih) as usize];
        for &(x, y) in &[(2, 2), (2, 1), (3, 2)] {
            b[(y * iw + x) as usize] = 1;
        }
        let _ = ih;
        assert_eq!(fix_edge_pixel_pair(2, 2, 3, 1, &b, iw), (2, 1, 3, 1));
    }

    #[test]
    fn get_centered_contour_concatenates_two_halves() {
        // On the straight edge a centered contour of half_contour=1 needs one point either side of
        // the feature. Feature (2,2)/edge (2,1): clockwise half → (3,2), ccw half → (1,2).
        let (b, iw, ih) = half_plane();
        let res = get_centered_contour(1, 2, 2, 2, 1, &b, iw, ih);
        let CenteredContour::Ok(c) = res else {
            panic!("expected a centered contour, got {res:?}");
        };
        // Reversed first half (single point), feature point, forward second half.
        assert_eq!(c.x, vec![3, 2, 1]);
        assert_eq!(c.y, vec![2, 2, 2]);
        assert_eq!(c.len(), 3);
    }

    #[test]
    fn get_centered_contour_incomplete_when_edge_too_close() {
        // half_contour=3 cannot fit either side before the 5-wide image edge → INCOMPLETE.
        let (b, iw, ih) = half_plane();
        assert_eq!(
            get_centered_contour(3, 2, 2, 2, 1, &b, iw, ih),
            CenteredContour::Incomplete
        );
    }

    #[test]
    fn get_centered_contour_ignores_same_colored_pair() {
        let (b, iw, ih) = half_plane();
        assert_eq!(
            get_centered_contour(1, 2, 2, 2, 3, &b, iw, ih),
            CenteredContour::Ignore
        );
    }
}
