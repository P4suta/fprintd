// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Analysis and filling of small ridge/valley loops (lakes and islands) in the binary image —
//! faithful port of stock NBIS `mindtct/src/lib/mindtct/loop.c` (renamed `loops`, since `loop` is a
//! Rust keyword).
//!
//! A bifurcation that closes a short contour encircles a small feature — a *lake* (an enclosed
//! valley) or *island* (an enclosed ridge). This module decides which minutiae sit on such loops and
//! either promotes the loop to a pair of minutiae or erases it from the image:
//!
//! * [`on_loop`] traces a minutia's contour clockwise up to a step limit and reports whether it
//!   closed a loop (stock `on_loop`); the false-minutia-removal stages (`remove.rs`) drive it.
//! * [`is_loop_clockwise`] decides a loop contour's winding via its chain code
//!   ([`chain_code_loop`](super::chaincod::chain_code_loop) +
//!   [`is_chain_clockwise`](super::chaincod::is_chain_clockwise)).
//! * [`get_loop_aspect`] measures a loop's widest and narrowest span, the shape test that decides
//!   whether it holds minutiae.
//! * [`process_loop_v2`] turns a qualifying loop into a pair of minutiae, or — failing the shape
//!   test — erases it from the binary image via [`fill_loop`]. [`fill_loop`] and its row helper
//!   [`fill_partial_row`] fill a loop's concave-aware interior using a per-row
//!   [`Shape`](super::shape::Shape) built by [`shape_from_contour`](super::shape::shape_from_contour).
//!
//! The loop-determination and fill scan orders are reproduced step-for-step from the reference so the
//! decisions and image edits are identical. Every routine here is live: [`process_loop_v2`] is wired
//! into `detect_minutiae_V2` (through `adjust_high_curvature_minutia_V2`), and [`on_loop`] /
//! [`fill_loop`] into `remove_false_minutia_V2`. See `docs/mindtct-algorithm.md`.

use super::contour::{line2direction, trace_contour, Contour, ScanDir, TraceResult};
use super::shape::shape_from_contour;
use super::{
    create_minutia, is_minutia_appearing, minutia_type, update_minutiae, DetMinutia,
    HIGH_RELIABILITY, LOOP_ID, MEDIUM_RELIABILITY,
};
use crate::params::LfsParms;
use crate::util::squared_distance;

/// Outcome of [`on_loop`] — the stock `int` return (`IGNORE`/`LOOP_FOUND`/`FALSE`).
///
/// The stock `Negative` (system error) return is unreachable here: [`trace_contour`] cannot fail (its
/// `malloc` paths have no analogue), so this enum is exhaustive over the real outcomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OnLoop {
    /// The minutia's contour closed a qualifying loop (stock `LOOP_FOUND`).
    LoopFound,
    /// The contour was traced but did not close within the step limit (stock `FALSE`).
    NotFound,
    /// The contour could not be traced (stock `IGNORE`); the caller should drop the minutia.
    Ignore,
}

/// Determine whether a minutia lies on a loop of at most `max_loop_len` circumference — port of stock
/// `on_loop` (`loop.c` L192).
///
/// Traces the feature's contour clockwise from the minutia point (using the minutia point itself as
/// the loop-trigger, so a walk back around to the start is detected), for up to `max_loop_len` steps.
/// A [`TraceResult::Loop`] means the minutia sits on a qualifying loop; a full/short
/// [`TraceResult::Traced`] means it does not; a [`TraceResult::Ignore`] means the contour could not be
/// traced. `bdata` is the binary image (`0 == white/valley`, `1 == black/ridge`), `iw`×`ih` pixels.
pub(crate) fn on_loop(
    minutia: &DetMinutia,
    max_loop_len: i32,
    bdata: &[u8],
    iw: i32,
    ih: i32,
) -> OnLoop {
    // PORT L200–L204: trace the contour clockwise, with the minutia point as both start and
    // loop-trigger.
    match trace_contour(
        max_loop_len,
        minutia.x,
        minutia.y,
        minutia.x,
        minutia.y,
        minutia.ex,
        minutia.ey,
        ScanDir::Clockwise,
        bdata,
        iw,
        ih,
    ) {
        // PORT L207–L208: trace impossible → IGNORE.
        TraceResult::Ignore => OnLoop::Ignore,
        // PORT L211–L214: the trace completed a loop → LOOP_FOUND.
        TraceResult::Loop(_) => OnLoop::LoopFound,
        // PORT L218–L221: traced but no loop within the step limit → FALSE.
        TraceResult::Traced(_) => OnLoop::NotFound,
    }
}

/// Decide whether a loop's contour is ordered clockwise — port of stock `is_loop_clockwise`
/// (`loop.c` L492).
///
/// Derives the contour's 8-connected chain code
/// ([`chain_code_loop`](super::chaincod::chain_code_loop)) and folds its turns into a winding
/// decision ([`is_chain_clockwise`](super::chaincod::is_chain_clockwise)). When the contour is too
/// short to produce a chain (three or fewer points, so `chain_code_loop` returns empty), the
/// direction is indeterminate and the caller-supplied `default_ret` is returned. Returns `1` (`TRUE`,
/// clockwise), `0` (`FALSE`, counter-clockwise), or `default_ret`.
pub(crate) fn is_loop_clockwise(contour_x: &[i32], contour_y: &[i32], default_ret: i32) -> i32 {
    // PORT L499–L500: derive the chain code from the contour points.
    let chain = super::chaincod::chain_code_loop(contour_x, contour_y);

    // PORT L504–L510: an empty chain means too few points to tell → the default.
    if chain.is_empty() {
        return default_ret;
    }

    // PORT L515: fold the chain's turns into a winding decision (passing `default_ret` on for the
    // indeterminate net-zero case).
    super::chaincod::is_chain_clockwise(&chain, default_ret)
}

/// Measure a loop's widest and narrowest span — port of stock `get_loop_aspect` (`loop.c` L885).
///
/// Walks opposite points of the loop (index `i` and its antipode `i + halfway`, wrapping) and
/// records the squared distance between them, tracking the running minimum and maximum. An even-length
/// loop only needs half a walk (the second half is exactly redundant); an odd-length loop is walked
/// in full (its halves differ, and that difference "may" be meaningful — verbatim from the reference).
///
/// Returns `(min_fr, min_to, min_dist, max_fr, max_to, max_dist)`: the contour index pairs where the
/// minimum and maximum spans occur and the corresponding squared distances.
pub(crate) fn get_loop_aspect(
    contour_x: &[i32],
    contour_y: &[i32],
) -> (i32, i32, f64, i32, i32, f64) {
    let ncontour = contour_x.len() as i32;

    // PORT L895: half the loop's perimeter.
    let halfway = ncontour >> 1;

    // PORT L900–L904: seed opposite points at index 0 and its antipode; their squared span.
    let mut i = 0;
    let mut j = halfway;
    let mut dist = squared_distance(
        contour_x[i as usize],
        contour_y[i as usize],
        contour_x[j as usize],
        contour_y[j as usize],
    );

    // PORT L907–L912: initialize the running minimum and maximum at that first pair.
    let mut min_dist = dist;
    let mut min_i = i;
    let mut min_j = j;
    let mut max_dist = dist;
    let mut max_i = i;
    let mut max_j = j;

    // PORT L914–L917: advance to the next opposite pair (`j` wraps around the end).
    i += 1;
    j += 1;
    j %= ncontour;

    // PORT L926–L933: odd loop → walk the whole perimeter; even loop → walk only half.
    let limit = if ncontour % 2 != 0 { ncontour } else { halfway };

    // PORT L936: walk until the perimeter limit.
    while i < limit {
        // PORT L937–L939: squared span of the current opposite pair.
        dist = squared_distance(
            contour_x[i as usize],
            contour_y[i as usize],
            contour_x[j as usize],
            contour_y[j as usize],
        );
        // PORT L941–L945: track the running minimum.
        if dist < min_dist {
            min_dist = dist;
            min_i = i;
            min_j = j;
        }
        // PORT L946–L950: track the running maximum.
        if dist > max_dist {
            max_dist = dist;
            max_i = i;
            max_j = j;
        }
        // PORT L951–L955: advance to the next opposite pair (`j` wraps).
        i += 1;
        j += 1;
        j %= ncontour;
    }

    // PORT L959–L964: return the min/max index pairs and their squared distances.
    (min_i, min_j, min_dist, max_i, max_j, max_dist)
}

/// Process a contour known to form a complete loop — port of stock `process_loop_V2` (`loop.c`
/// L707).
///
/// A sufficiently large, narrow/elongated loop yields two minutiae at the ends of its longest span
/// (reliability set from the Low Ridge Flow map); otherwise the loop is assumed spurious and erased
/// from the image via [`fill_loop`].
///
/// `contour` is the loop's traced contour; `bdata` is the binary image (`0 == white/valley`,
/// `1 == black/ridge`), `iw`×`ih` pixels; `plow_flow_map` is the pixelized Low Ridge Flow map.
///
/// PORT: the two candidate minutiae are added with stock **version one** of `update_minutiae`
/// (deliberately, per the reference comment), not the V2 variant the scan drivers use. On IGNORE the
/// stock frees the rejected minutia; here it is simply dropped.
///
/// # Errors
///
/// Propagates the negative stock error codes surfaced by the minutia-update and fill routines.
pub(crate) fn process_loop_v2(
    minutiae: &mut Vec<DetMinutia>,
    contour: &Contour,
    bdata: &mut [u8],
    iw: i32,
    ih: i32,
    plow_flow_map: &[i32],
    lfsparms: &LfsParms,
) -> Result<(), i32> {
    let ncontour = contour.len() as i32;

    // PORT L724–L725: an empty contour has nothing to process.
    if ncontour <= 0 {
        return Ok(());
    }

    // PORT L728: only loops above the minimum perimeter can carry minutiae.
    if ncontour > lfsparms.min_loop_len {
        // PORT L730: interior pixel value of the feature (first contour point).
        let feature_pix = bdata[(contour.y[0] * iw + contour.x[0]) as usize];

        // PORT L736–L738: widest/narrowest spans across the loop.
        let (_min_fr, _min_to, min_dist, max_fr, max_to, max_dist) =
            get_loop_aspect(&contour.x, &contour.y);

        // PORT L742–L743: loop must be sufficiently narrow or elongated.
        if min_dist < lfsparms.min_loop_aspect_dist
            || (max_dist / min_dist) >= lfsparms.min_loop_aspect_ratio
        {
            let max_fr = max_fr as usize;
            let max_to = max_to as usize;

            // PORT L750–L753: the interior midpoint of the widest span must match the feature.
            let mid_x = (contour.x[max_fr] + contour.x[max_to]) >> 1;
            let mid_y = (contour.y[max_fr] + contour.y[max_to]) >> 1;
            let mid_pix = bdata[(mid_y * iw + mid_x) as usize];
            if mid_pix == feature_pix {
                // PORT L758–L840: 1. widest-span endpoint as a candidate minutia.
                let mut idir = line2direction(
                    contour.x[max_fr],
                    contour.y[max_fr],
                    contour.x[max_to],
                    contour.y[max_to],
                    lfsparms.num_directions,
                );
                let kind = minutia_type(feature_pix);
                let appearing = is_minutia_appearing(
                    contour.x[max_fr],
                    contour.y[max_fr],
                    contour.ex[max_fr],
                    contour.ey[max_fr],
                )?;
                let fmapval = plow_flow_map[(contour.y[max_fr] * iw + contour.x[max_fr]) as usize];
                let reliability = if fmapval != 0 {
                    MEDIUM_RELIABILITY
                } else {
                    HIGH_RELIABILITY
                };
                let minutia = create_minutia(
                    contour.x[max_fr],
                    contour.y[max_fr],
                    contour.ex[max_fr],
                    contour.ey[max_fr],
                    idir,
                    reliability,
                    kind,
                    appearing == 1,
                    LOOP_ID,
                );
                // PORT L827: NOTE — deliberately version one of update_minutiae.
                update_minutiae(minutiae, minutia, bdata, iw, ih, lfsparms)?;

                // PORT L845–L848: 2. opposite endpoint, direction flipped 180°.
                idir += lfsparms.num_directions;
                idir %= lfsparms.num_directions << 1;

                let appearing = is_minutia_appearing(
                    contour.x[max_to],
                    contour.y[max_to],
                    contour.ex[max_to],
                    contour.ey[max_to],
                )?;
                let fmapval = plow_flow_map[(contour.y[max_to] * iw + contour.x[max_to]) as usize];
                let reliability = if fmapval != 0 {
                    MEDIUM_RELIABILITY
                } else {
                    HIGH_RELIABILITY
                };
                let minutia = create_minutia(
                    contour.x[max_to],
                    contour.y[max_to],
                    contour.ex[max_to],
                    contour.ey[max_to],
                    idir,
                    reliability,
                    kind,
                    appearing == 1,
                    LOOP_ID,
                );
                // PORT L889: NOTE — deliberately version one of update_minutiae.
                update_minutiae(minutiae, minutia, bdata, iw, ih, lfsparms)?;

                // PORT L897–L898: loop processed successfully.
                return Ok(());
            }
        }
    }

    // PORT L905–L909: otherwise the loop is assumed spurious — erase it from the image.
    fill_loop(contour, bdata, iw, ih)
}

/// Fill a loop's interior in the binary image, honoring concave shapes — port of stock `fill_loop`
/// (`loop.c` L991).
///
/// Builds a per-row [`Shape`](super::shape::Shape) from the loop's contour and fills each row between
/// contour points, skipping the gaps that concavities open up so the flood does not escape the loop.
///
/// `contour` is the loop's traced contour; `bdata` is the binary image (`0 == white/valley`,
/// `1 == black/ridge`), `iw`×`ih` pixels.
///
/// PORT: stock takes `ih` but the fill is row-addressed and never reads it; it is kept in the
/// signature for parity with the reference but unused. A malformed (empty) row makes the stock post a
/// warning and return normally — here it simply returns `Ok(())`.
///
/// # Errors
///
/// Propagates the negative stock error code surfaced by `shape_from_contour`.
pub(crate) fn fill_loop(contour: &Contour, bdata: &mut [u8], iw: i32, ih: i32) -> Result<(), i32> {
    let _ = ih;

    // PORT L1001–L1003: build the per-row shape from the loop's contour.
    let shape = shape_from_contour(&contour.x, &contour.y)?;

    // PORT L1007–L1015: feature (interior) pixel and its opposite (the edge/fill value).
    let feature_pix = bdata[(contour.y[0] * iw + contour.x[0]) as usize];
    let edge_pix: u8 = if feature_pix != 0 { 0 } else { 1 };

    // PORT L1018: foreach row in the shape ...
    for row in &shape.rows {
        // PORT L1020: y-coord of the current row.
        let y = row.y;

        // PORT L1024–L1032: a row is expected to hold at least one contour point; if not, the shape
        // is malformed — preempt the fill and return normally.
        if row.xs.is_empty() {
            return Ok(());
        }

        // PORT L1035–L1041: fill the left-most contour point on the row.
        let mut j = 0usize;
        let mut x = row.xs[0];
        bdata[(y * iw + x) as usize] = edge_pix;
        // PORT L1043: index of the last contour point on the row.
        let lastj = row.xs.len() - 1;

        // PORT L1044: while the last contour point on the row has not been processed ...
        while j < lastj {
            // PORT L1055–L1057: pixel just right of the last filled contour point.
            x += 1;
            let next_pix = bdata[(y * iw + x) as usize];

            // PORT L1060: if it matches the edge value, assume a concavity and skip to the next point.
            if next_pix == edge_pix {
                // PORT L1063–L1067: jump to and fill the next contour point.
                j += 1;
                x = row.xs[j];
                bdata[(y * iw + x) as usize] = edge_pix;
            } else {
                // PORT L1074–L1084: fill from the current pixel through the next contour point.
                j += 1;
                let nx = row.xs[j];
                fill_partial_row(edge_pix, x, nx, y, bdata, iw);
            }
        }
    }

    // PORT L1112: return normally.
    Ok(())
}

/// Fill a contiguous range of pixels on one image row with a value — port of stock `fill_partial_row`
/// (`loop.c` L1116).
///
/// Sets every pixel from `frx` to `tox` (inclusive) on row `y` to `fill_pix`. The coordinates are
/// assumed within the image bounds (the caller guarantees this).
///
/// PORT: stock takes `ih` but never reads it, so it is dropped here (mirroring `fix_edge_pixel_pair`).
/// `fill_pix` is a stock `int` on `[0..255]` but only ever a binary `0`/`1` from the loop routines, so
/// it is a `u8` here.
pub(crate) fn fill_partial_row(
    fill_pix: u8,
    frx: i32,
    tox: i32,
    y: i32,
    bdata: &mut [u8],
    iw: i32,
) {
    // PORT L1127–L1132: fill from `frx` through `tox` inclusive, left to right.
    let mut x = frx;
    while x <= tox {
        bdata[(y * iw + x) as usize] = fill_pix;
        x += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::super::BIFURCATION;
    use super::*;

    /// A 5×5 image whose bottom three rows (`y >= 2`) are ridge (`1`) — a straight horizontal feature
    /// edge along `y == 2`.
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

    /// A 5×5 image with a solid 3×3 ridge block at `x,y ∈ [1,3]` — a closed feature whose interior
    /// edge is an eight-pixel loop.
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

    /// Construct a minutia with the given position/edge/type; the remaining fields are irrelevant to
    /// the loop routines and set to inert defaults.
    fn minutia(x: i32, y: i32, ex: i32, ey: i32, kind: i32) -> DetMinutia {
        DetMinutia {
            x,
            y,
            ex,
            ey,
            direction: 0,
            reliability: 0.0,
            kind,
            appearing: true,
            feature_id: 0,
            nbrs: Vec::new(),
            ridge_counts: Vec::new(),
        }
    }

    #[test]
    fn on_loop_detects_a_loop() {
        let (b, iw, ih) = block_3x3();
        // Top-left interior ridge (1,1) with its north valley edge (1,0): the contour walks clockwise
        // back around to (1,1) → a loop.
        let m = minutia(1, 1, 1, 0, BIFURCATION);
        assert_eq!(on_loop(&m, 20, &b, iw, ih), OnLoop::LoopFound);
    }

    #[test]
    fn on_loop_traces_without_a_loop() {
        let (b, iw, ih) = half_plane();
        // Feature (2,2)/edge (2,1) on a straight edge: the trace runs off the image before closing.
        let m = minutia(2, 2, 2, 1, BIFURCATION);
        assert_eq!(on_loop(&m, 4, &b, iw, ih), OnLoop::NotFound);
    }

    #[test]
    fn on_loop_ignores_a_same_colored_pair() {
        let (b, iw, ih) = block_3x3();
        // Both (2,2) and (2,3) are ridge → not opposite colors → the contour can't be traced.
        let m = minutia(2, 2, 2, 3, BIFURCATION);
        assert_eq!(on_loop(&m, 20, &b, iw, ih), OnLoop::Ignore);
    }

    // A 2×2 pixel square walked clockwise on screen (image coords, `y` down).
    const SQUARE_X: [i32; 8] = [0, 1, 2, 2, 2, 1, 0, 0];
    const SQUARE_Y: [i32; 8] = [0, 0, 0, 1, 2, 2, 2, 1];

    #[test]
    fn is_loop_clockwise_reports_winding() {
        // The screen-clockwise square is clockwise (TRUE == 1).
        assert_eq!(is_loop_clockwise(&SQUARE_X, &SQUARE_Y, -1), 1);
        // Reversed → counter-clockwise (FALSE == 0).
        let mut rx = SQUARE_X;
        let mut ry = SQUARE_Y;
        rx.reverse();
        ry.reverse();
        assert_eq!(is_loop_clockwise(&rx, &ry, -1), 0);
    }

    #[test]
    fn is_loop_clockwise_returns_default_for_short_contours() {
        // Three or fewer points → no chain → the caller's default.
        assert_eq!(is_loop_clockwise(&[0, 1, 2], &[0, 0, 0], 42), 42);
    }

    #[test]
    fn get_loop_aspect_finds_min_and_max_spans() {
        // Even-length loop (8): walk only the first half. Hand-traced against loop.c:
        //   i=0,j=4: (0,0)-(2,2) = 8   (seed → both min and max)
        //   i=1,j=5: (1,0)-(1,2) = 4   (new min)
        //   i=2,j=6: (2,0)-(0,2) = 8   (ties max, not > → no update)
        //   i=3,j=7: (2,1)-(0,1) = 4   (ties min, not < → no update)
        let (min_fr, min_to, min_dist, max_fr, max_to, max_dist) =
            get_loop_aspect(&SQUARE_X, &SQUARE_Y);
        assert_eq!((min_fr, min_to, min_dist), (1, 5, 4.0));
        assert_eq!((max_fr, max_to, max_dist), (0, 4, 8.0));
    }

    #[test]
    fn fill_partial_row_fills_inclusive_range() {
        let iw = 5;
        let mut b = vec![0u8; (iw * 2) as usize];
        fill_partial_row(1, 1, 3, 0, &mut b, iw);
        // Row 0 filled from x=1..=3; row 1 untouched.
        assert_eq!(&b[0..5], &[0, 1, 1, 1, 0]);
        assert_eq!(&b[5..10], &[0, 0, 0, 0, 0]);
    }
}
