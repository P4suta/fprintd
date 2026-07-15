// SPDX-FileCopyrightText: NIST NBIS — U.S. Government work, public domain (title 17 §105)
//
// SPDX-License-Identifier: LicenseRef-NBIS-PD

//! False-minutia removal (`remove_false_minutia_V2`): the sequence of island/lake, hole,
//! invalid-block, side, hook, overlap, malformation and pore filters that prune spurious candidates —
//! faithful port of stock NBIS `mindtct/src/lib/mindtct/remove.c` (the ten-stage `_V2` driver at
//! L172), plus the small supporting routines the stage needs from `loop.c` (`on_hook`,
//! `on_island_lake`), `imgutil.c` (`free_path`, `search_in_direction`), `util.c` (`distance`,
//! `squared_distance`, `closest_dir_dist`, `minmaxs`), `maps.c` (`num_valid_8nbrs`) and `minutia.c`
//! (`sort_minutiae_y_x`).
//!
//! The ten stages run in the exact reference order, each walking the minutiae list in the reference's
//! scan direction so that `remove_minutia`'s in-place slide keeps subsequent indices consistent. Some
//! stages edit the binary image in place (island/lake filling); that is reproduced so later stages see
//! the same pixels the reference does — only the pruned minutiae list is exposed.

// `too_many_arguments`: every routine here is a verbatim transcription of a stock `remove.c` (or
// supporting `loop.c`/`imgutil.c`/`util.c`) function whose interface is fixed by the reference — the
// binary image (`bdata`, `iw`, `ih`), one or more block maps (`map`, `mw`, `mh`), coordinate pairs,
// and the parameter block. Bundling those into ad-hoc structs would obscure the one-to-one
// correspondence with the C; the reference-side arity is the justification, so the lint is suppressed
// for the whole file.
#![allow(clippy::too_many_arguments)]

use crate::consts::TRUNC_SCALE;
use crate::detect::contour::{
    fix_edge_pixel_pair, get_centered_contour, line2direction, trace_contour, CenteredContour,
    Contour, ScanDir, TraceResult,
};
use crate::detect::line::line_points;
use crate::detect::loops::{fill_loop, on_loop, OnLoop};
use crate::detect::{DetMinutia, BIFURCATION};
use crate::num::{sort_indices_int_inc, sround, trunc_dbl_precision};
use crate::params::LfsParms;

/// Stock `INVALID_DIR` (`lfs.h` L320): a block whose ridge-flow direction could not be determined.
const INVALID_DIR: i32 = -1;

// ===========================================================================
// Supporting routines ported from util.c / maps.c / imgutil.c / loop.c
// ===========================================================================

/// Euclidean distance between two integer points — port of stock `distance` (`util.c` L358).
fn distance(x1: i32, y1: i32, x2: i32, y2: i32) -> f64 {
    // PORT L363–L369: sqrt of the squared distance.
    let dx = f64::from(x1 - x2);
    let dy = f64::from(y1 - y2);
    ((dx * dx) + (dy * dy)).sqrt()
}

/// Squared Euclidean distance between two integer points — port of stock `squared_distance`
/// (`util.c` L388).
fn squared_distance(x1: i32, y1: i32, x2: i32, y2: i32) -> f64 {
    // PORT L393–L397: (x1-x2)^2 + (y1-y2)^2.
    let dx = f64::from(x1 - x2);
    let dy = f64::from(y1 - y2);
    (dx * dx) + (dy * dy)
}

/// Inner (wrap-aware) distance between two integer directions — port of stock `closest_dir_dist`
/// (`util.c` L602).
///
/// Returns [`INVALID_DIR`] if either direction is invalid (`< 0`), else the smaller of the direct and
/// wrap-around distances on a circle of `ndirs` directions.
fn closest_dir_dist(dir1: i32, dir2: i32, ndirs: i32) -> i32 {
    // PORT L607–L618: only defined for two valid directions.
    if dir1 >= 0 && dir2 >= 0 {
        let d1 = (dir2 - dir1).abs();
        let d2 = ndirs - d1;
        d1.min(d2)
    } else {
        INVALID_DIR
    }
}

/// Count the valid (`>= 0`) blocks among the eight neighbors of a block — port of stock
/// `num_valid_8nbrs` (`maps.c` L2044).
///
/// The eight neighbors are tested in the stock order NW, N, NE, E, SE, S, SW, W, each only when its
/// coordinates lie within the map.
fn num_valid_8nbrs(imap: &[i32], mx: i32, my: i32, mw: i32, mh: i32) -> i32 {
    let mut nvalid = 0;

    // PORT L2054–L2057: neighbor coordinate indices.
    let e_ind = mx + 1;
    let w_ind = mx - 1;
    let n_ind = my - 1;
    let s_ind = my + 1;

    // PORT L2059–L2091: eight guarded neighbor tests in stock order.
    if w_ind >= 0 && n_ind >= 0 && imap[(n_ind * mw + w_ind) as usize] >= 0 {
        nvalid += 1;
    }
    if n_ind >= 0 && imap[(n_ind * mw + mx) as usize] >= 0 {
        nvalid += 1;
    }
    if n_ind >= 0 && e_ind < mw && imap[(n_ind * mw + e_ind) as usize] >= 0 {
        nvalid += 1;
    }
    if e_ind < mw && imap[(my * mw + e_ind) as usize] >= 0 {
        nvalid += 1;
    }
    if e_ind < mw && s_ind < mh && imap[(s_ind * mw + e_ind) as usize] >= 0 {
        nvalid += 1;
    }
    if s_ind < mh && imap[(s_ind * mw + mx) as usize] >= 0 {
        nvalid += 1;
    }
    if w_ind >= 0 && s_ind < mh && imap[(s_ind * mw + w_ind) as usize] >= 0 {
        nvalid += 1;
    }
    if w_ind >= 0 && imap[(my * mw + w_ind) as usize] >= 0 {
        nvalid += 1;
    }

    nvalid
}

/// Whether a "free path" exists between two pixels — port of stock `free_path` (`imgutil.c` L341).
///
/// Walks the straight-line trajectory ([`line_points`]) between the two points and counts pixel-value
/// transitions; the path is free (`true`) iff no more than `lfsparms.maxtrans` transitions occur.
fn free_path(
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    bdata: &[u8],
    iw: i32,
    lfsparms: &LfsParms,
) -> bool {
    // PORT L350: points along the segment (its capacity guard is unreachable, so `Err` → not free).
    let list = match line_points(x1, y1, x2, y2) {
        Ok(l) => l,
        Err(_) => return false,
    };

    // PORT L354–L356: count transitions, seeded at the first point's value.
    let mut trans = 0;
    let mut preval = bdata[(y1 * iw + x1) as usize];

    // PORT L359–L381: a transition past `maxtrans` means no free path.
    for &(x, y) in list.iter().skip(1) {
        let nextval = bdata[(y * iw + x) as usize];
        if nextval != preval {
            trans += 1;
            if trans > lfsparms.maxtrans {
                return false;
            }
            preval = nextval;
        }
    }

    // PORT L388: within the transition budget → free path.
    true
}

/// Step in a direction until a pixel of value `pix` is found — port of stock `search_in_direction`
/// (`imgutil.c` L419).
///
/// Advances `maxsteps` times by `(delta_x, delta_y)` from `(strt_x, strt_y)` (rounding each step via
/// [`sround`]); on finding a `pix` pixel, adjusts the found/previous pair to be 4-connected
/// ([`fix_edge_pixel_pair`]) and returns `Some((x, y, ex, ey))`. Returns `None` (stock `FALSE`) if the
/// walk leaves the image or exhausts its steps.
fn search_in_direction(
    pix: u8,
    strt_x: i32,
    strt_y: i32,
    delta_x: f64,
    delta_y: f64,
    maxsteps: i32,
    bdata: &[u8],
    iw: i32,
    ih: i32,
) -> Option<(i32, i32, i32, i32)> {
    // PORT L429–L433: previous point and floating accumulators seeded at the start.
    let mut px = strt_x;
    let mut py = strt_y;
    let mut fx = f64::from(strt_x);
    let mut fy = f64::from(strt_y);

    // PORT L436: take up to `maxsteps` steps.
    for _ in 0..maxsteps {
        // PORT L438–L443: accumulate and round to the next pixel.
        fx += delta_x;
        fy += delta_y;
        let x = sround(fx);
        let y = sround(fy);

        // PORT L445–L454: stepped off the image → not found.
        if x < 0 || x >= iw || y < 0 || y >= ih {
            return None;
        }

        // PORT L456–L473: found the target pixel → make the pair 4-connected and return it.
        if bdata[(y * iw + x) as usize] == pix {
            let (ax, ay, aex, aey) = fix_edge_pixel_pair(x, y, px, py, bdata, iw);
            return Some((ax, ay, aex, aey));
        }

        // PORT L477–L479: advance the previous point and step again.
        px = x;
        py = y;
    }

    // PORT L482–L487: exhausted the steps → not found.
    None
}

/// The three parallel outputs of [`minmaxs`] — value, type (`-1` minima / `+1` maxima) and index of
/// each relative extremum.
struct MinMaxs {
    val: Vec<i32>,
    kind: Vec<i32>,
    idx: Vec<i32>,
}

/// Locate relative minima and maxima in a vector of integers — port of stock `minmaxs` (`util.c`
/// L158).
///
/// Walks the run-length structure of the sequence, recording each turning point at the midpoint of the
/// level run that precedes it. Fewer than three items yields no extrema.
fn minmaxs(items: &[i32]) -> MinMaxs {
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

/// Order the minutiae top-to-bottom, left-to-right — port of stock `sort_minutiae_y_x`
/// (`minutia.c` L615).
///
/// Sorts by the 1-D pixel offset `y * iw + x` using the port's stable index sort, so equal keys keep
/// their input order exactly as the reference's stable sort does.
///
/// PORT: `ih` is taken by the reference but never read, so it is dropped here.
fn sort_minutiae_y_x(minutiae: &mut Vec<DetMinutia>, iw: i32) {
    // PORT L629–L637: 1-D pixel offsets, then their sorted permutation.
    let mut ranks: Vec<i32> = minutiae.iter().map(|m| (m.y * iw) + m.x).collect();
    let order = sort_indices_int_inc(&mut ranks);

    // PORT L648–L655: rebuild the list in sorted order.
    let newlist: Vec<DetMinutia> = order
        .into_iter()
        .map(|o| minutiae[o as usize].clone())
        .collect();
    *minutiae = newlist;
}

/// Outcome of [`on_hook`] — the stock `int` return (`HOOK_FOUND`/`FALSE`/`IGNORE`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum OnHook {
    /// The two minutiae lie on a common qualifying hook (stock `HOOK_FOUND`).
    HookFound,
    /// They do not (stock `FALSE`).
    NotFound,
    /// The contour could not be traced (stock `IGNORE`).
    Ignore,
}

/// Whether two opposite-type minutiae lie on a common hook — port of stock `on_hook` (`loop.c`
/// L398).
///
/// Traces the feature contour from minutia 1's *edge* pixel toward minutia 2, first clockwise then (if
/// that misses) counter-clockwise, each up to `max_hook_len` steps. A trace that walks onto minutia 2
/// is a hook.
fn on_hook(
    m1x: i32,
    m1y: i32,
    m1ex: i32,
    m1ey: i32,
    m2x: i32,
    m2y: i32,
    max_hook_len: i32,
    bdata: &[u8],
    iw: i32,
    ih: i32,
) -> OnHook {
    // PORT L413–L417: trace from minutia1's edge point clockwise, loop-trigger at minutia2.
    match trace_contour(
        max_hook_len,
        m2x,
        m2y,
        m1ex,
        m1ey,
        m1x,
        m1y,
        ScanDir::Clockwise,
        bdata,
        iw,
        ih,
    ) {
        TraceResult::Ignore => return OnHook::Ignore,
        TraceResult::Loop(_) => return OnHook::HookFound,
        TraceResult::Traced(_) => {}
    }

    // PORT L443–L447: retry counter-clockwise.
    match trace_contour(
        max_hook_len,
        m2x,
        m2y,
        m1ex,
        m1ey,
        m1x,
        m1y,
        ScanDir::CounterClockwise,
        bdata,
        iw,
        ih,
    ) {
        TraceResult::Ignore => OnHook::Ignore,
        TraceResult::Loop(_) => OnHook::HookFound,
        TraceResult::Traced(_) => OnHook::NotFound,
    }
}

/// Outcome of [`on_island_lake`] — the stock `int` return (`LOOP_FOUND` with a combined loop contour,
/// `FALSE`, or `IGNORE`).
enum IslandLake {
    /// The pair encloses a loop (stock `LOOP_FOUND`); the combined loop contour is returned.
    Found(Contour),
    /// The pair does not enclose a loop (stock `FALSE`).
    NotFound,
    /// A contour could not be traced (stock `IGNORE`).
    Ignore,
}

/// Whether two same-type minutiae bound a common island or lake — port of stock `on_island_lake`
/// (`loop.c` L252).
///
/// Traces the feature contour from minutia 1 toward minutia 2 and, if it arrives, from minutia 2 back
/// toward minutia 1 (both clockwise, each up to `max_half_loop` steps). If both halves close, they are
/// concatenated — minutia 1, the first half, minutia 2, the second half — into the loop's contour.
fn on_island_lake(
    m1x: i32,
    m1y: i32,
    m1ex: i32,
    m1ey: i32,
    m2x: i32,
    m2y: i32,
    m2ex: i32,
    m2ey: i32,
    max_half_loop: i32,
    bdata: &[u8],
    iw: i32,
    ih: i32,
) -> IslandLake {
    // PORT L266–L270: first half, from minutia1 with loop-trigger at minutia2.
    let c1 = match trace_contour(
        max_half_loop,
        m2x,
        m2y,
        m1x,
        m1y,
        m1ex,
        m1ey,
        ScanDir::Clockwise,
        bdata,
        iw,
        ih,
    ) {
        // PORT L273–L274: trace impossible.
        TraceResult::Ignore => return IslandLake::Ignore,
        // PORT L370–L373: 1st trace did not reach minutia2 → not a loop.
        TraceResult::Traced(_) => return IslandLake::NotFound,
        // PORT L277: reached minutia2.
        TraceResult::Loop(c) => c,
    };

    // PORT L283–L287: second half, from minutia2 with loop-trigger at minutia1.
    let c2 = match trace_contour(
        max_half_loop,
        m1x,
        m1y,
        m2x,
        m2y,
        m2ex,
        m2ey,
        ScanDir::Clockwise,
        bdata,
        iw,
        ih,
    ) {
        // PORT L290–L292: trace impossible.
        TraceResult::Ignore => return IslandLake::Ignore,
        // PORT L354–L359: 2nd trace did not reach minutia1 → not a loop.
        TraceResult::Traced(_) => return IslandLake::NotFound,
        // PORT L296: reached minutia1.
        TraceResult::Loop(c) => c,
    };

    // PORT L299–L334: combine — minutia1, first half, minutia2, second half.
    let nloop = c1.len() + c2.len() + 2;
    let mut x = Vec::with_capacity(nloop);
    let mut y = Vec::with_capacity(nloop);
    let mut ex = Vec::with_capacity(nloop);
    let mut ey = Vec::with_capacity(nloop);

    x.push(m1x);
    y.push(m1y);
    ex.push(m1ex);
    ey.push(m1ey);
    x.extend_from_slice(&c1.x);
    y.extend_from_slice(&c1.y);
    ex.extend_from_slice(&c1.ex);
    ey.extend_from_slice(&c1.ey);
    x.push(m2x);
    y.push(m2y);
    ex.push(m2ex);
    ey.push(m2ey);
    x.extend_from_slice(&c2.x);
    y.extend_from_slice(&c2.y);
    ex.extend_from_slice(&c2.ex);
    ey.extend_from_slice(&c2.ey);

    // PORT L347–L348: island/lake found.
    IslandLake::Found(Contour { x, y, ex, ey })
}

// ===========================================================================
// The ten removal stages
// ===========================================================================

/// Remove minutia pairs bounding a lake or island, filling the enclosed loop — port of stock
/// `remove_islands_and_lakes` (`remove.c` L851).
fn remove_islands_and_lakes(
    minutiae: &mut Vec<DetMinutia>,
    bdata: &mut [u8],
    iw: i32,
    ih: i32,
    lfsparms: &LfsParms,
) {
    let dist_thresh = lfsparms.max_rmtest_dist;
    let half_loop = lfsparms.max_half_loop;

    // PORT L871–L889: removal flags and the join-direction thresholds.
    let mut to_remove = vec![false; minutiae.len()];
    let full_ndirs = lfsparms.num_directions << 1;
    let qtr_ndirs = lfsparms.num_directions >> 2;
    let min_deltadir = (3 * qtr_ndirs) - 1;

    // PORT L892: foreach primary minutia (bar the last).
    let mut f = 0;
    while f + 1 < minutiae.len() {
        // PORT L896: skip a first minutia already flagged.
        if !to_remove[f] {
            let m1 = minutiae[f].clone();
            // PORT L904: foreach secondary minutia to its right.
            let mut s = f + 1;
            while s < minutiae.len() {
                let m2 = minutiae[s].clone();

                // PORT L910: only same-type pairs are considered.
                if m2.kind == m1.kind {
                    // PORT L923–L928: first minutia's pixel changed → next first.
                    if bdata[(m1.y * iw + m1.x) as usize] != m1.kind as u8 {
                        break;
                    }
                    // PORT L932–L934: second minutia's pixel changed → flag it.
                    if bdata[(m2.y * iw + m2.x) as usize] != m2.kind as u8 {
                        to_remove[s] = true;
                    }

                    // PORT L937: only proceed if the second is not flagged.
                    if !to_remove[s] {
                        // PORT L940–L942: vertical gap gate.
                        let delta_y = m2.y - m1.y;
                        if delta_y <= dist_thresh {
                            // PORT L948–L952: Euclidean distance gate.
                            let dist = distance(m1.x, m1.y, m2.x, m2.y);
                            if dist <= f64::from(dist_thresh) {
                                // PORT L958–L970: direction-difference gate.
                                let deltadir =
                                    closest_dir_dist(m1.direction, m2.direction, full_ndirs);
                                if deltadir > min_deltadir {
                                    // PORT L979–L982: island/lake test.
                                    match on_island_lake(
                                        m1.x, m1.y, m1.ex, m1.ey, m2.x, m2.y, m2.ex, m2.ey,
                                        half_loop, bdata, iw, ih,
                                    ) {
                                        // PORT L984–L1002: fill the loop and flag both.
                                        IslandLake::Found(loop_c) => {
                                            let _ = fill_loop(&loop_c, bdata, iw, ih);
                                            to_remove[f] = true;
                                            to_remove[s] = true;
                                        }
                                        // PORT L1004–L1012: IGNORE → flag the first, next first.
                                        IslandLake::Ignore => {
                                            to_remove[f] = true;
                                            break;
                                        }
                                        // PORT L1019–L1020: not on a loop → next second.
                                        IslandLake::NotFound => {}
                                    }
                                }
                            }
                        } else {
                            // PORT L1030–L1035: 2nd too far below → next first.
                            break;
                        }
                    }
                }

                // PORT L1043: next second minutia.
                s += 1;
            }
        }
        // PORT L1049: next first minutia.
        f += 1;
    }

    // PORT L1053–L1065: remove flagged minutiae in reverse index order.
    remove_flagged(minutiae, &to_remove);
}

/// Remove bifurcations sitting on small holes (short valley loops) — port of stock `remove_holes`
/// (`remove.c` L261).
fn remove_holes(
    minutiae: &mut Vec<DetMinutia>,
    bdata: &[u8],
    iw: i32,
    ih: i32,
    lfsparms: &LfsParms,
) {
    // PORT L270: walk the list; a removal does not advance `i`.
    let mut i = 0;
    while i < minutiae.len() {
        // PORT L276: only bifurcations.
        if minutiae[i].kind == BIFURCATION {
            // PORT L278: on a small loop?
            match on_loop(&minutiae[i], lfsparms.small_loop_len, bdata, iw, ih) {
                // PORT L280–L291: on a loop or IGNORE → remove.
                OnLoop::LoopFound | OnLoop::Ignore => {
                    minutiae.remove(i);
                }
                // PORT L293–L296: not on a loop → advance.
                OnLoop::NotFound => {
                    i += 1;
                }
            }
        } else {
            // PORT L304–L307: ridge-ending → advance.
            i += 1;
        }
    }
}

/// Remove minutiae pointing (opposite their direction) at a nearby invalid block — port of stock
/// `remove_pointing_invblock_V2` (`remove.c` L1863).
fn remove_pointing_invblock_v2(
    minutiae: &mut Vec<DetMinutia>,
    direction_map: &[i32],
    mw: i32,
    mh: i32,
    lfsparms: &LfsParms,
) {
    // PORT L1877: integer-direction → radians factor.
    let pi_factor = std::f64::consts::PI / f64::from(lfsparms.num_directions);

    // PORT L1879: walk the list; a removal does not advance `i`.
    let mut i = 0;
    while i < minutiae.len() {
        let m = &minutiae[i];
        // PORT L1885–L1894: translate opposite the minutia direction by `trans_dir_pix`.
        let theta = f64::from(m.direction) * pi_factor;
        let mut dx = theta.sin() * f64::from(lfsparms.trans_dir_pix);
        let mut dy = theta.cos() * f64::from(lfsparms.trans_dir_pix);
        dx = trunc_dbl_precision(dx, TRUNC_SCALE);
        dy = trunc_dbl_precision(dy, TRUNC_SCALE);
        let delta_x = sround(dx);
        let delta_y = sround(dy);
        let nx = m.x - delta_x;
        let ny = m.y + delta_y;

        // PORT L1898–L1907: block coords, clamped to the map.
        let mut bx = nx / lfsparms.blocksize;
        let mut by = ny / lfsparms.blocksize;
        bx = bx.clamp(0, mw - 1);
        by = by.clamp(0, mh - 1);

        // PORT L1910–L1926: invalid block → remove, else advance.
        if direction_map[(by * mw + bx) as usize] == INVALID_DIR {
            minutiae.remove(i);
        } else {
            i += 1;
        }
    }
}

/// Remove minutiae in the margin of a nearby invalid block or the image edge — port of stock
/// `remove_near_invblock_V2` (`remove.c` L1541).
fn remove_near_invblock_v2(
    minutiae: &mut Vec<DetMinutia>,
    direction_map: &[i32],
    mw: i32,
    mh: i32,
    lfsparms: &LfsParms,
) {
    // PORT L1591–L1610: neighbor-index LUTs (indexed by iy*3+ix) and per-neighbor coord offsets.
    const STARTBLK: [i32; 9] = [6, 0, 0, 6, -1, 2, 4, 4, 2];
    const ENDBLK: [i32; 9] = [8, 0, 2, 6, -1, 2, 6, 4, 4];
    const BLKDX: [i32; 9] = [0, 1, 1, 1, 0, -1, -1, -1, 0];
    const BLKDY: [i32; 9] = [-1, -1, 0, 1, 1, 1, 0, -1, -1];

    // PORT L1624–L1625: margin boundaries within a block.
    let lo_margin = lfsparms.inv_block_margin;
    let hi_margin = lfsparms.blocksize - lfsparms.inv_block_margin - 1;

    // PORT L1627: walk the list; a removal does not advance `i`.
    let mut i = 0;
    while i < minutiae.len() {
        let m = &minutiae[i];
        // PORT L1634–L1646: block coords and pixel offset within the block.
        let bx = m.x / lfsparms.blocksize;
        let by = m.y / lfsparms.blocksize;
        let px = m.x % lfsparms.blocksize;
        let py = m.y % lfsparms.blocksize;

        // PORT L1648–L1668: classify each axis offset as low margin / middle / high margin.
        let ix = if px < lo_margin {
            0
        } else if px > hi_margin {
            2
        } else {
            1
        };
        let iy = if py < lo_margin {
            0
        } else if py > hi_margin {
            2
        } else {
            1
        };

        let mut removed = false;

        // PORT L1674: only act when the point is in a margin.
        if ix != 1 || iy != 1 {
            // PORT L1677–L1679: neighbor index range for this (ix, iy).
            let sbi = STARTBLK[(iy * 3 + ix) as usize];
            let ebi = ENDBLK[(iy * 3 + ix) as usize];

            // PORT L1682: examine each neighbor in range.
            let mut ni = sbi;
            while ni <= ebi {
                let nbx = bx + BLKDX[ni as usize];
                let nby = by + BLKDY[ni as usize];

                // PORT L1688–L1709: neighbor off the map (adjacent to the image edge) → remove.
                if nbx < 0 || nbx >= mw || nby < 0 || nby >= mh {
                    minutiae.remove(i);
                    removed = true;
                    break;
                }
                // PORT L1710–L1733: invalid neighbor with too few valid neighbors → remove.
                if direction_map[(nby * mw + nbx) as usize] == INVALID_DIR {
                    let nvalid = num_valid_8nbrs(direction_map, nbx, nby, mw, mh);
                    if nvalid < lfsparms.rm_valid_nbr_min {
                        minutiae.remove(i);
                        removed = true;
                        break;
                    }
                }
                ni += 1;
            }
        }

        // PORT L1741–L1745: advance only if the point survived.
        if !removed {
            i += 1;
        }
    }
}

/// Remove or reposition minutiae sitting on the side of a ridge or valley — port of stock
/// `remove_or_adjust_side_minutiae_V2` (`remove.c` L3180).
///
/// Extracts a centered contour; a point on an incomplete/looping contour is dropped. Otherwise the
/// contour's rotated y-coords are analyzed for relative extrema: a single minimum (or a min-max-min
/// triple) relocates the minutia onto the deepest minimum's contour point, dropping it instead if that
/// lands in an invalid block; any other extremum pattern drops it.
fn remove_or_adjust_side_minutiae_v2(
    minutiae: &mut Vec<DetMinutia>,
    bdata: &[u8],
    iw: i32,
    ih: i32,
    direction_map: &[i32],
    mw: i32,
    lfsparms: &LfsParms,
) {
    // PORT L3206: integer-direction → radians factor.
    let pi_factor = std::f64::consts::PI / f64::from(lfsparms.num_directions);

    // PORT L3208: walk the list; a removal does not advance `i`.
    let mut i = 0;
    while i < minutiae.len() {
        let m = minutiae[i].clone();

        // PORT L3216–L3220: extract a contour centered on the minutia point.
        let contour = match get_centered_contour(
            lfsparms.side_half_contour,
            m.x,
            m.y,
            m.ex,
            m.ey,
            bdata,
            iw,
            ih,
        ) {
            // PORT L3232–L3247: loop / impossible / short → remove.
            CenteredContour::Loop | CenteredContour::Ignore | CenteredContour::Incomplete => {
                minutiae.remove(i);
                continue;
            }
            CenteredContour::Ok(c) => c,
        };

        // PORT L3272–L3286: rotate the contour y-coords by the negative feature direction.
        let theta = f64::from(m.direction) * pi_factor;
        let sin_theta = theta.sin();
        let cos_theta = theta.cos();
        let mut rot_y = Vec::with_capacity(contour.len());
        for j in 0..contour.len() {
            let mut drot_y =
                (f64::from(contour.x[j]) * sin_theta) - (f64::from(contour.y[j]) * cos_theta);
            drot_y = trunc_dbl_precision(drot_y, TRUNC_SCALE);
            rot_y.push(sround(drot_y));
        }

        // PORT L3290: relative minima/maxima of the rotated y-coords.
        let mm = minmaxs(&rot_y);
        let minmax_num = mm.idx.len();

        // PORT L3302–L3341: exactly one minimum → relocate (or drop if now invalid).
        if minmax_num == 1 && mm.kind[0] == -1 {
            let loc = mm.idx[0] as usize;
            let side_minutia_adjust =
                adjust_side_minutia(&mut minutiae[i], &contour, loc, direction_map, mw, lfsparms);
            match side_minutia_adjust {
                SideAdjust::Removed => {}
                SideAdjust::Kept => i += 1,
            }
        }
        // PORT L3343–L3386: min-max-min → relocate onto the deeper minimum (or drop if invalid).
        else if minmax_num == 3 && mm.kind[0] == -1 {
            let loc = if mm.val[0] < mm.val[2] {
                mm.idx[0] as usize
            } else {
                mm.idx[2] as usize
            };
            match adjust_side_minutia(&mut minutiae[i], &contour, loc, direction_map, mw, lfsparms)
            {
                SideAdjust::Removed => {}
                SideAdjust::Kept => i += 1,
            }
        }
        // PORT L3388–L3407: any other pattern → remove.
        else {
            minutiae.remove(i);
        }
    }
}

/// Whether an adjusted side minutia was kept (advance) or removed (slide).
enum SideAdjust {
    Kept,
    Removed,
}

/// Relocate a side minutia onto contour point `loc` and drop it if that lands in an invalid block —
/// the shared tail of [`remove_or_adjust_side_minutiae_v2`]'s two adjustment cases (stock
/// `remove.c` L3307–L3339 / L3353–L3385).
///
/// Returns whether the minutia was removed (so the caller must not advance) or kept.
fn adjust_side_minutia(
    minutia: &mut DetMinutia,
    contour: &Contour,
    loc: usize,
    direction_map: &[i32],
    mw: i32,
    lfsparms: &LfsParms,
) -> SideAdjust {
    // PORT L3308–L3311: reset the minutia location to the extremum's contour point.
    minutia.x = contour.x[loc];
    minutia.y = contour.y[loc];
    minutia.ex = contour.ex[loc];
    minutia.ey = contour.ey[loc];

    // PORT L3314–L3334: drop it if the adjusted point is now in an invalid block.
    let bx = minutia.x / lfsparms.blocksize;
    let by = minutia.y / lfsparms.blocksize;
    if direction_map[(by * mw + bx) as usize] == INVALID_DIR {
        SideAdjust::Removed
    } else {
        SideAdjust::Kept
    }
}

/// Remove minutia pairs forming a hook on the side of a ridge or valley — port of stock
/// `remove_hooks` (`remove.c` L332).
fn remove_hooks(
    minutiae: &mut Vec<DetMinutia>,
    bdata: &[u8],
    iw: i32,
    ih: i32,
    lfsparms: &LfsParms,
) {
    // PORT L347–L364: removal flags and the join-direction thresholds.
    let mut to_remove = vec![false; minutiae.len()];
    let full_ndirs = lfsparms.num_directions << 1;
    let qtr_ndirs = lfsparms.num_directions >> 2;
    let min_deltadir = (3 * qtr_ndirs) - 1;

    // PORT L366: foreach primary minutia (bar the last).
    let mut f = 0;
    while f + 1 < minutiae.len() {
        // PORT L371: skip a first minutia already flagged.
        if !to_remove[f] {
            let m1 = minutiae[f].clone();
            // PORT L378: foreach secondary minutia to its right.
            let mut s = f + 1;
            while s < minutiae.len() {
                let m2 = minutiae[s].clone();

                // PORT L393–L397: first minutia's pixel changed → next first.
                if bdata[(m1.y * iw + m1.x) as usize] != m1.kind as u8 {
                    break;
                }
                // PORT L400–L402: second minutia's pixel changed → flag it.
                if bdata[(m2.y * iw + m2.x) as usize] != m2.kind as u8 {
                    to_remove[s] = true;
                }

                // PORT L405: only proceed if the second is not flagged.
                if !to_remove[s] {
                    // PORT L408–L410: vertical gap gate.
                    let delta_y = m2.y - m1.y;
                    if delta_y <= lfsparms.max_rmtest_dist {
                        // PORT L415–L418: Euclidean distance gate.
                        let dist = distance(m1.x, m1.y, m2.x, m2.y);
                        if dist <= f64::from(lfsparms.max_rmtest_dist) {
                            // PORT L424–L435: direction-difference gate.
                            let deltadir = closest_dir_dist(m1.direction, m2.direction, full_ndirs);
                            if deltadir > min_deltadir {
                                // PORT L440: only opposite-type pairs form a hook.
                                if m1.kind != m2.kind {
                                    // PORT L444–L446: hook test.
                                    match on_hook(
                                        m1.x,
                                        m1.y,
                                        m1.ex,
                                        m1.ey,
                                        m2.x,
                                        m2.y,
                                        lfsparms.max_hook_len,
                                        bdata,
                                        iw,
                                        ih,
                                    ) {
                                        // PORT L449–L457: hook found → flag both.
                                        OnHook::HookFound => {
                                            to_remove[f] = true;
                                            to_remove[s] = true;
                                        }
                                        // PORT L459–L468: IGNORE → flag the first, next first.
                                        OnHook::Ignore => {
                                            to_remove[f] = true;
                                            break;
                                        }
                                        // PORT L474–L477: no hook → next second.
                                        OnHook::NotFound => {}
                                    }
                                }
                            }
                        }
                    } else {
                        // PORT L489–L497: 2nd too far below → next first.
                        break;
                    }
                }

                // PORT L503: next second minutia.
                s += 1;
            }
        }
        // PORT L509: next first minutia.
        f += 1;
    }

    // PORT L513–L525: remove flagged minutiae in reverse index order.
    remove_flagged(minutiae, &to_remove);
}

/// Remove minutia pairs sitting on opposite sides of an overlap — port of stock `remove_overlaps`
/// (`remove.c` L1953).
///
/// Unlike the island/lake stage this does not edit the binary image.
fn remove_overlaps(minutiae: &mut Vec<DetMinutia>, bdata: &[u8], iw: i32, lfsparms: &LfsParms) {
    // PORT L1969–L1988: removal flags and the join-direction thresholds.
    let mut to_remove = vec![false; minutiae.len()];
    let full_ndirs = lfsparms.num_directions << 1;
    let qtr_ndirs = lfsparms.num_directions >> 2;
    let half_ndirs = lfsparms.num_directions >> 1;
    let min_deltadir = (3 * qtr_ndirs) - 1;

    // PORT L1990: foreach primary minutia (bar the last).
    let mut f = 0;
    while f + 1 < minutiae.len() {
        // PORT L1995: skip a first minutia already flagged.
        if !to_remove[f] {
            let m1 = minutiae[f].clone();
            // PORT L2002: foreach secondary minutia to its right.
            let mut s = f + 1;
            while s < minutiae.len() {
                let m2 = minutiae[s].clone();

                // PORT L2017–L2021: first minutia's pixel changed → next first.
                if bdata[(m1.y * iw + m1.x) as usize] != m1.kind as u8 {
                    break;
                }
                // PORT L2024–L2026: second minutia's pixel changed → flag it.
                if bdata[(m2.y * iw + m2.x) as usize] != m2.kind as u8 {
                    to_remove[s] = true;
                }

                // PORT L2029: only proceed if the second is not flagged.
                if !to_remove[s] {
                    // PORT L2032–L2034: vertical gap gate.
                    let delta_y = m2.y - m1.y;
                    if delta_y <= lfsparms.max_overlap_dist {
                        // PORT L2039–L2042: Euclidean distance gate.
                        let dist = distance(m1.x, m1.y, m2.x, m2.y);
                        if dist <= f64::from(lfsparms.max_overlap_dist) {
                            // PORT L2048–L2059: direction-difference gate.
                            let deltadir = closest_dir_dist(m1.direction, m2.direction, full_ndirs);
                            if deltadir > min_deltadir {
                                // PORT L2064: only same-type pairs form an overlap.
                                if m1.kind == m2.kind {
                                    // PORT L2071–L2082: joining direction relative to minutia1's
                                    // opposite direction.
                                    let mut joindir = line2direction(
                                        m1.x,
                                        m1.y,
                                        m2.x,
                                        m2.y,
                                        lfsparms.num_directions,
                                    );
                                    let opp1dir =
                                        (m1.direction + lfsparms.num_directions) % full_ndirs;
                                    joindir = (opp1dir - joindir).abs();
                                    joindir = joindir.min(full_ndirs - joindir);

                                    // PORT L2089–L2102: shallow join angle or close pair, with a
                                    // free path → overlap, flag both.
                                    if (joindir <= half_ndirs
                                        || dist <= f64::from(lfsparms.max_overlap_join_dist))
                                        && free_path(m1.x, m1.y, m2.x, m2.y, bdata, iw, lfsparms)
                                    {
                                        to_remove[f] = true;
                                        to_remove[s] = true;
                                    }
                                }
                            }
                        }
                    } else {
                        // PORT L2118–L2126: 2nd too far below → next first.
                        break;
                    }
                }

                // PORT L2132: next second minutia.
                s += 1;
            }
        }
        // PORT L2138: next first minutia.
        f += 1;
    }

    // PORT L2142–L2154: remove flagged minutiae in reverse index order.
    remove_flagged(minutiae, &to_remove);
}

/// Remove "irregularly" shaped minutiae — port of stock `remove_malformations` (`remove.c` L1105).
///
/// For each minutia (walked in reverse) two `malformation_steps_2`-long contours are traced either
/// way; a point on an incomplete/looping contour is dropped. Otherwise the across-feature spans at the
/// two step marks are measured: a zero span, an over-long span in a low-flow block, or a large
/// span-length ratio where the far span crosses opposite-colored pixels marks the minutia malformed.
fn remove_malformations(
    minutiae: &mut Vec<DetMinutia>,
    bdata: &[u8],
    iw: i32,
    ih: i32,
    low_flow_map: &[i32],
    mw: i32,
    lfsparms: &LfsParms,
) {
    let steps1 = lfsparms.malformation_steps_1;
    let steps2 = lfsparms.malformation_steps_2;

    // PORT L1122: walk the list in reverse (removal keeps lower indices valid).
    let mut i = minutiae.len() as i32 - 1;
    while i >= 0 {
        let idx = i as usize;
        let m = minutiae[idx].clone();

        // PORT L1124–L1129: first contour, counter-clockwise.
        let (ax1, ay1, bx1, by1) = match trace_contour(
            steps2,
            m.x,
            m.y,
            m.x,
            m.y,
            m.ex,
            m.ey,
            ScanDir::CounterClockwise,
            bdata,
            iw,
            ih,
        ) {
            // PORT L1139–L1154: loop / impossible / short → remove.
            TraceResult::Ignore => {
                minutiae.remove(idx);
                i -= 1;
                continue;
            }
            TraceResult::Loop(_) => {
                minutiae.remove(idx);
                i -= 1;
                continue;
            }
            TraceResult::Traced(c) => {
                if (c.len() as i32) < steps2 {
                    minutiae.remove(idx);
                    i -= 1;
                    continue;
                }
                // PORT L1157–L1163: 'A1' and 'B1' contour points.
                (
                    c.x[(steps1 - 1) as usize],
                    c.y[(steps1 - 1) as usize],
                    c.x[(steps2 - 1) as usize],
                    c.y[(steps2 - 1) as usize],
                )
            }
        };

        // PORT L1168–L1173: second contour, clockwise.
        let (ax2, ay2, bx2, by2) = match trace_contour(
            steps2,
            m.x,
            m.y,
            m.x,
            m.y,
            m.ex,
            m.ey,
            ScanDir::Clockwise,
            bdata,
            iw,
            ih,
        ) {
            // PORT L1183–L1198: loop / impossible / short → remove.
            TraceResult::Ignore => {
                minutiae.remove(idx);
                i -= 1;
                continue;
            }
            TraceResult::Loop(_) => {
                minutiae.remove(idx);
                i -= 1;
                continue;
            }
            TraceResult::Traced(c) => {
                if (c.len() as i32) < steps2 {
                    minutiae.remove(idx);
                    i -= 1;
                    continue;
                }
                // PORT L1201–L1207: 'A2' and 'B2' contour points.
                (
                    c.x[(steps1 - 1) as usize],
                    c.y[(steps1 - 1) as usize],
                    c.x[(steps2 - 1) as usize],
                    c.y[(steps2 - 1) as usize],
                )
            }
        };

        // PORT L1213–L1218: span lengths at the two step marks and the minutia's block.
        let a_dist = distance(ax1, ay1, ax2, ay2);
        let b_dist = distance(bx1, by1, bx2, by2);
        let blk_x = m.x / lfsparms.blocksize;
        let blk_y = m.y / lfsparms.blocksize;

        let mut removed = false;

        // PORT L1222–L1230: a zero-length span is malformed.
        if a_dist == 0.0 || b_dist == 0.0 {
            minutiae.remove(idx);
            removed = true;
        }

        // PORT L1232–L1247: low-flow block cursory test on the far span.
        if !removed {
            let fmapval = low_flow_map[(blk_y * mw + blk_x) as usize];
            if fmapval != 0 && b_dist > f64::from(lfsparms.max_malformation_dist) {
                minutiae.remove(idx);
                removed = true;
            }
        }

        // PORT L1249–L1284: span-ratio test along the far span line.
        if !removed {
            if let Ok(pts) = line_points(bx1, by1, bx2, by2) {
                for (x, y) in pts {
                    // PORT L1257: far span crosses a pixel opposite the minutia type.
                    if bdata[(y * iw + x) as usize] != m.kind as u8 {
                        // PORT L1259–L1262: quantized span ratio.
                        let mut ratio = b_dist / a_dist;
                        ratio = trunc_dbl_precision(ratio, TRUNC_SCALE);
                        // PORT L1264–L1277: far span sufficiently longer → malformed.
                        if ratio > lfsparms.min_malformation_ratio {
                            minutiae.remove(idx);
                            break;
                        }
                    }
                }
            }
        }

        i -= 1;
    }
}

/// Remove minutiae located on pore-shaped valleys/ridges in unreliable regions — port of stock
/// `remove_pores_V2` (`remove.c` L2572).
///
/// Only tests minutiae in LOW RIDGE FLOW or HIGH CURVATURE blocks with a valid direction. For each, a
/// reference point R is placed just behind the minutia and the two white transitions P and Q either
/// side of it are found; short contours walked from P and Q give the four points A/B/C/D. If any step
/// fails, or the A-B / C-D squared-distance ratio is small enough, the minutia is a pore and is
/// removed.
fn remove_pores_v2(
    minutiae: &mut Vec<DetMinutia>,
    bdata: &[u8],
    iw: i32,
    ih: i32,
    direction_map: &[i32],
    low_flow_map: &[i32],
    high_curve_map: &[i32],
    mw: i32,
    lfsparms: &LfsParms,
) {
    // PORT L2623: integer-direction → radians factor.
    let pi_factor = std::f64::consts::PI / f64::from(lfsparms.num_directions);

    // PORT L2626: walk the list; a removal does not advance `i`.
    let mut i = 0;
    while i < minutiae.len() {
        let m = minutiae[i].clone();

        // PORT L2636–L2643: only test unreliable blocks with a valid direction.
        let blk_x = m.x / lfsparms.blocksize;
        let blk_y = m.y / lfsparms.blocksize;
        let bi = (blk_y * mw + blk_x) as usize;
        let unreliable = low_flow_map[bi] != 0 || high_curve_map[bi] != 0;

        let remove_it = if unreliable && direction_map[bi] >= 0 {
            pores_v2_is_pore(&m, bdata, iw, ih, pi_factor, lfsparms)
        } else {
            false
        };

        // PORT L2929–L2933: remove a pore (no advance), else advance.
        if remove_it {
            minutiae.remove(i);
        } else {
            i += 1;
        }
    }
}

/// The per-minutia pore test of [`remove_pores_v2`] — returns whether the minutia should be removed
/// (stock `remove.c` L2644–L2927).
fn pores_v2_is_pore(
    m: &DetMinutia,
    bdata: &[u8],
    iw: i32,
    ih: i32,
    pi_factor: f64,
    lfsparms: &LfsParms,
) -> bool {
    // PORT L2645–L2660: reference point R, `pores_trans_r` behind the minutia.
    let theta = f64::from(m.direction) * pi_factor;
    let sin_theta = theta.sin();
    let cos_theta = theta.cos();
    let mut drx = f64::from(m.x) - (sin_theta * f64::from(lfsparms.pores_trans_r));
    let mut dry = f64::from(m.y) + (cos_theta * f64::from(lfsparms.pores_trans_r));
    drx = trunc_dbl_precision(drx, TRUNC_SCALE);
    dry = trunc_dbl_precision(dry, TRUNC_SCALE);
    let rx = sround(drx);
    let ry = sround(dry);

    // PORT L2663/L2925: R must be opposite the minutia's colour, else keep the minutia.
    if bdata[(ry * iw + rx) as usize] == m.kind as u8 {
        return false;
    }

    // PORT L2670–L2674: find P (a white transition perpendicular to the minutia direction).
    let (px, py, pex, pey) = match search_in_direction(
        m.kind as u8,
        rx,
        ry,
        -cos_theta,
        -sin_theta,
        lfsparms.pores_perp_steps,
        bdata,
        iw,
        ih,
    ) {
        Some(p) => p,
        // PORT L2913–L2924: P not found → pore.
        None => return true,
    };

    // PORT L2677–L2717: B — trace P counter-clockwise (forward).
    let (bx, by) = match pores_endpoint(
        lfsparms.pores_steps_fwd,
        px,
        py,
        pex,
        pey,
        ScanDir::CounterClockwise,
        bdata,
        iw,
        ih,
    ) {
        Some(p) => p,
        // PORT L2701–L2708: RMB → pore.
        None => return true,
    };

    // PORT L2719–L2761: D — trace P clockwise (backward).
    let (dx, dy) = match pores_endpoint(
        lfsparms.pores_steps_bwd,
        px,
        py,
        pex,
        pey,
        ScanDir::Clockwise,
        bdata,
        iw,
        ih,
    ) {
        Some(p) => p,
        // PORT L2745–L2752: RMD → pore.
        None => return true,
    };

    // PORT L2768–L2772: find Q (opposite side of R from P).
    let (qx, qy, qex, qey) = match search_in_direction(
        m.kind as u8,
        rx,
        ry,
        cos_theta,
        sin_theta,
        lfsparms.pores_perp_steps,
        bdata,
        iw,
        ih,
    ) {
        Some(q) => q,
        // PORT L2898–L2909: Q not found → pore.
        None => return true,
    };

    // PORT L2776–L2816: A — trace Q clockwise (forward).
    let (ax, ay) = match pores_endpoint(
        lfsparms.pores_steps_fwd,
        qx,
        qy,
        qex,
        qey,
        ScanDir::Clockwise,
        bdata,
        iw,
        ih,
    ) {
        Some(p) => p,
        // PORT L2800–L2807: RMA → pore.
        None => return true,
    };

    // PORT L2818–L2862: C — trace Q counter-clockwise (backward).
    let (cx, cy) = match pores_endpoint(
        lfsparms.pores_steps_bwd,
        qx,
        qy,
        qex,
        qey,
        ScanDir::CounterClockwise,
        bdata,
        iw,
        ih,
    ) {
        Some(p) => p,
        // PORT L2845–L2853: RMC → pore.
        None => return true,
    };

    // PORT L2864–L2894: compare the A-B and C-D squared spans.
    let ab2 = squared_distance(ax, ay, bx, by);
    let cd2 = squared_distance(cx, cy, dx, dy);
    if cd2 > lfsparms.pores_min_dist2 {
        let ratio = ab2 / cd2;
        // PORT L2877–L2891: A-B not sufficiently longer than C-D → pore.
        if ratio <= lfsparms.pores_max_ratio {
            return true;
        }
    }

    // PORT L2892/L2925: ratio too big or C-D span negligible → keep the minutia.
    false
}

/// Trace `steps` points from a pore edge pixel and return the last contour point — the shared contour
/// walk of [`pores_v2_is_pore`]. Returns `None` on an impossible/looping/short trace (stock's remove
/// paths).
fn pores_endpoint(
    steps: i32,
    x: i32,
    y: i32,
    ex: i32,
    ey: i32,
    scan: ScanDir,
    bdata: &[u8],
    iw: i32,
    ih: i32,
) -> Option<(i32, i32)> {
    match trace_contour(steps, x, y, x, y, ex, ey, scan, bdata, iw, ih) {
        TraceResult::Traced(c) if (c.len() as i32) >= steps => {
            let l = c.len() - 1;
            Some((c.x[l], c.y[l]))
        }
        _ => None,
    }
}

/// Remove the minutiae whose `to_remove` flag is set, in reverse index order so earlier indices stay
/// valid — the shared tail of the flag-based stages (stock's reverse `remove_minutia` loop).
fn remove_flagged(minutiae: &mut Vec<DetMinutia>, to_remove: &[bool]) {
    for i in (0..minutiae.len()).rev() {
        if to_remove[i] {
            minutiae.remove(i);
        }
    }
}

/// Detect and remove false minutiae — port of stock `remove_false_minutia_V2` (`remove.c` L172).
///
/// Runs the ten removal stages in the reference order over the detected minutiae list, editing the
/// binary image where the reference does (island/lake filling). `direction_map`, `low_flow_map` and
/// `high_curve_map` are the `mw`×`mh` **block** maps (indexed by block, not pixel); `bdata` is the
/// `iw`×`ih` binary image in stock convention (`0 == white/valley`, `1 == black/ridge`).
///
/// # Errors
///
/// Returns `Ok(())`; the `Result` mirrors the reference signature (whose negative error codes are
/// unreachable in the port) so the driver in `lib.rs` can stay uniform with the detection stage.
pub(crate) fn remove_false_minutia_v2(
    minutiae: &mut Vec<DetMinutia>,
    bdata: &mut [u8],
    iw: i32,
    ih: i32,
    direction_map: &[i32],
    low_flow_map: &[i32],
    high_curve_map: &[i32],
    mw: i32,
    mh: i32,
    lfsparms: &LfsParms,
) -> Result<(), i32> {
    // 1. PORT L179–L182: sort top-to-bottom, left-to-right.
    sort_minutiae_y_x(minutiae, iw);

    // 2. PORT L184–L189: remove minutiae on lakes/islands (edits the binary image).
    remove_islands_and_lakes(minutiae, bdata, iw, ih, lfsparms);

    // 3. PORT L191–L195: remove minutiae on single-point holes.
    remove_holes(minutiae, bdata, iw, ih, lfsparms);

    // 4. PORT L197–L202: remove minutiae pointing at an invalid block.
    remove_pointing_invblock_v2(minutiae, direction_map, mw, mh, lfsparms);

    // 5. PORT L204–L209: remove minutiae near an invalid block.
    remove_near_invblock_v2(minutiae, direction_map, mw, mh, lfsparms);

    // 6. PORT L211–L216: remove or adjust side minutiae.
    remove_or_adjust_side_minutiae_v2(minutiae, bdata, iw, ih, direction_map, mw, lfsparms);

    // 7. PORT L218–L221: remove hooks.
    remove_hooks(minutiae, bdata, iw, ih, lfsparms);

    // 8. PORT L223–L226: remove overlaps.
    remove_overlaps(minutiae, bdata, iw, lfsparms);

    // 9. PORT L228–L232: remove malformations.
    remove_malformations(minutiae, bdata, iw, ih, low_flow_map, mw, lfsparms);

    // 10. PORT L234–L240: remove pores.
    remove_pores_v2(
        minutiae,
        bdata,
        iw,
        ih,
        direction_map,
        low_flow_map,
        high_curve_map,
        mw,
        lfsparms,
    );

    // PORT L242: normal return.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::RIDGE_ENDING;

    fn minutia(x: i32, y: i32, ex: i32, ey: i32, direction: i32, kind: i32) -> DetMinutia {
        DetMinutia {
            x,
            y,
            ex,
            ey,
            direction,
            reliability: 0.99,
            kind,
            appearing: true,
            feature_id: 0,
            nbrs: Vec::new(),
            ridge_counts: Vec::new(),
        }
    }

    #[test]
    fn distance_and_squared_distance_agree() {
        assert_eq!(squared_distance(0, 0, 3, 4), 25.0);
        assert_eq!(distance(0, 0, 3, 4), 5.0);
        assert_eq!(distance(2, 2, 2, 2), 0.0);
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
    fn num_valid_8nbrs_counts_within_bounds() {
        // 3x3 map, all valid (0) except center's NW corner which is -1.
        let map = vec![-1, 0, 0, 0, 0, 0, 0, 0, 0];
        // Center (1,1): NW is index 0 == -1 (invalid), other seven valid → 7.
        assert_eq!(num_valid_8nbrs(&map, 1, 1, 3, 3), 7);
        // Corner (0,0): only E, SE, S in-bounds and valid → 3.
        assert_eq!(num_valid_8nbrs(&map, 0, 0, 3, 3), 3);
    }

    #[test]
    fn free_path_counts_transitions() {
        // A 6-wide, 1-tall row: 0 0 1 0 0 0. Path (0,0)->(5,0) has 2 transitions (0->1, 1->0).
        let bdata = vec![0u8, 0, 1, 0, 0, 0];
        let mut p = crate::params::LFSPARMS_V2;
        p.maxtrans = 2;
        assert!(free_path(0, 0, 5, 0, &bdata, 6, &p));
        // With only 1 allowed transition, the same path is not free.
        p.maxtrans = 1;
        assert!(!free_path(0, 0, 5, 0, &bdata, 6, &p));
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

    #[test]
    fn sort_minutiae_y_x_orders_top_left_first() {
        let mut ms = vec![
            minutia(5, 2, 5, 1, 0, RIDGE_ENDING),
            minutia(1, 2, 1, 1, 0, RIDGE_ENDING),
            minutia(3, 1, 3, 0, 0, RIDGE_ENDING),
        ];
        sort_minutiae_y_x(&mut ms, 10);
        // y=1 row first, then y=2 row left-to-right.
        assert_eq!((ms[0].x, ms[0].y), (3, 1));
        assert_eq!((ms[1].x, ms[1].y), (1, 2));
        assert_eq!((ms[2].x, ms[2].y), (5, 2));
    }

    #[test]
    fn remove_flagged_removes_in_reverse() {
        let mut ms = vec![
            minutia(0, 0, 0, 0, 0, RIDGE_ENDING),
            minutia(1, 1, 1, 1, 0, RIDGE_ENDING),
            minutia(2, 2, 2, 2, 0, RIDGE_ENDING),
        ];
        remove_flagged(&mut ms, &[true, false, true]);
        assert_eq!(ms.len(), 1);
        assert_eq!((ms[0].x, ms[0].y), (1, 1));
    }

    #[test]
    fn driver_on_empty_list_is_ok() {
        let mut ms: Vec<DetMinutia> = Vec::new();
        let mut bdata = vec![0u8; 64];
        let map = vec![0i32; 1];
        let p = crate::params::LFSPARMS_V2;
        assert!(
            remove_false_minutia_v2(&mut ms, &mut bdata, 8, 8, &map, &map, &map, 1, 1, &p).is_ok()
        );
        assert!(ms.is_empty());
    }
}
