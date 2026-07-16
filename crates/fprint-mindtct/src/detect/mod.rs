// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Minutiae detection on the binary image (`detect_minutiae_V2`): scan each image block with valid
//! ridge-flow direction for candidate ridge endings and bifurcations, tracing the contour of each
//! feature and matching the fixed 2×3 pixel-pair patterns. Submodule root for the detection stage.
//!
//! Faithful port of the detection routines spread across stock NBIS
//! `mindtct/src/lib/mindtct/{minutia,line,chaincod,matchpat,contour,loop,shape}.c`. The scan driver
//! (`detect_minutiae_V2`, `scan4minutiae_*`) and the shared minutia type live here in the root; the
//! per-primitive helpers are split into submodules mirroring the stock files:
//!
//! * [`line`] — straight-line pixel trajectories between two points (`line.c`).
//! * [`chaincod`] — 8-connected chain-code of a contour and its turn analysis (`chaincod.c`).
//! * [`matchpat`] — the fixed feature patterns and the 2×3 pixel-pair scan (`matchpat.c`).
//! * [`contour`] — ridge/valley contour tracing around a candidate point (`contour.c`), plus
//!   `line2direction`/`angle2line` (`util.c`).
//! * [`loops`] — detection/handling of small ridge/valley loops (`loop.c`; `loop` is a Rust keyword,
//!   hence `loops`).
//! * [`shape`] — per-row shape of a closed loop for concave-aware filling (`shape.c`).
//!
//! See `docs/mindtct-algorithm.md`.

mod chaincod;
pub(crate) mod contour;
pub(crate) mod line;
pub(crate) mod loops;
mod matchpat;
mod shape;

use crate::params::LfsParms;

use contour::{
    get_high_curvature_contour, line2direction, min_contour_theta, search_contour,
    HighCurvatureContour, ScanDir,
};
use loops::{is_loop_clockwise, process_loop_v2};
use matchpat::{
    match_1st_pair, match_2nd_pair, match_3rd_pair, skip_repeated_horizontal_pair,
    skip_repeated_vertical_pair, FEATURE_PATTERNS,
};

use crate::block::block_offsets;

/// Stock `type` values for a detected minutia (`lfs.h` L152–L153): the point is a ridge
/// **bifurcation** (two ridges meeting) or a ridge **ending**.
pub(crate) const BIFURCATION: i32 = 0;
/// See [`BIFURCATION`].
pub(crate) const RIDGE_ENDING: i32 = 1;

/// Stock `INVALID_DIR` (`lfs.h` L320): a block whose ridge-flow direction could not be determined.
const INVALID_DIR: i32 = -1;
/// Stock `SCAN_HORIZONTAL` (`lfs.h` L494): feature scan runs across rows.
const SCAN_HORIZONTAL: i32 = 0;
/// Stock `SCAN_VERTICAL` (`lfs.h` L495): feature scan runs down columns.
const SCAN_VERTICAL: i32 = 1;
/// Stock `APPEARING` (`lfs.h` L155).
const APPEARING: i32 = 1;
/// Stock `DISAPPEARING` (`lfs.h` L154).
const DISAPPEARING: i32 = 0;
/// Stock `TRUE` (`lfs.h` L673) — the default winding passed to [`is_loop_clockwise`].
const TRUE: i32 = 1;
/// Stock `LOOP_ID` (`lfs.h` L491): the `feature_id` assigned to minutiae extracted from a loop.
pub(crate) const LOOP_ID: i32 = 10;
/// Stock `MEDIUM_RELIABILITY` (`lfs.h` L506): reliability of a minutia in a LOW RIDGE FLOW block.
pub(crate) const MEDIUM_RELIABILITY: f64 = 0.50;
/// Stock `HIGH_RELIABILITY` (`lfs.h` L509): reliability of a minutia in a reliable block.
pub(crate) const HIGH_RELIABILITY: f64 = 0.99;

/// One candidate minutia during detection — the port's analogue of the stock `MINUTIA` structure
/// (`lfs.h` L157), one field per member.
///
/// The stock C carries `int appearing` (`APPEARING`/`DISAPPEARING`) and `int type`
/// (`RIDGE_ENDING`/`BIFURCATION`); here `appearing` is a `bool` and `type` becomes `kind` (a Rust
/// keyword). The variable-length neighbor arrays (`nbrs` + `ridge_counts`, sized by `num_nbrs`)
/// become owned `Vec`s whose shared length *is* the stock `num_nbrs`; both are empty until the
/// ridge-count/neighbor stage fills them.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DetMinutia {
    /// X pixel coordinate of the minutia point (stock `x`).
    pub x: i32,
    /// Y pixel coordinate of the minutia point (stock `y`).
    pub y: i32,
    /// X pixel coordinate of the paired feature-edge pixel (stock `ex`).
    pub ex: i32,
    /// Y pixel coordinate of the paired feature-edge pixel (stock `ey`).
    pub ey: i32,
    /// Quantized ridge-flow direction at the point, `0..num_directions` (stock `direction`).
    pub direction: i32,
    /// Reliability estimate in `[0.0, 1.0]`; higher is better (stock `reliability`).
    pub reliability: f64,
    /// Feature class, [`RIDGE_ENDING`] or [`BIFURCATION`] (stock `type`).
    pub kind: i32,
    /// Ridge-scan polarity: `true` = `APPEARING`, `false` = `DISAPPEARING` (stock `appearing`).
    pub appearing: bool,
    /// Index of the matched feature pattern, `0..NFEATURES` (stock `feature_id`).
    pub feature_id: i32,
    /// Indices of neighboring minutiae, filled by the ridge-count stage (stock `nbrs`); its length
    /// is the stock `num_nbrs`.
    pub nbrs: Vec<i32>,
    /// Ridge counts to each neighbor, in `nbrs` order (stock `ridge_counts`).
    pub ridge_counts: Vec<i32>,
}

/// Allocate and initialize a minutia from its detected attributes — port of stock `create_minutia`
/// (`minutia.c` L903).
///
/// PORT: the stock out-parameter / `-230` malloc-failure return collapse into a by-value `DetMinutia`
/// (allocation aborts rather than returning an error). `type`/`appearing` map to
/// [`kind`](DetMinutia::kind) (`i32`) / a `bool`; `nbrs`/`ridge_counts` start empty (stock `NULL`,
/// `num_nbrs == 0`).
#[expect(clippy::too_many_arguments)]
pub(crate) fn create_minutia(
    x_loc: i32,
    y_loc: i32,
    x_edge: i32,
    y_edge: i32,
    idir: i32,
    reliability: f64,
    kind: i32,
    appearing: bool,
    feature_id: i32,
) -> DetMinutia {
    // PORT L920–L933: copy attributes into a fresh minutia; neighbor lists start empty.
    DetMinutia {
        x: x_loc,
        y: y_loc,
        ex: x_edge,
        ey: y_edge,
        direction: idir,
        reliability,
        kind,
        appearing,
        feature_id,
        nbrs: Vec::new(),
        ridge_counts: Vec::new(),
    }
}

/// Remove the minutia at `index` from the list — port of stock `remove_minutia` (`minutia.c` L994).
///
/// PORT: the stock in-place slide + `num--` (and its `free_minutia`) is a `Vec::remove`; the
/// out-of-range guard is dropped (a bad index panics, matching the reference's undefined behavior on
/// the same input).
fn remove_minutia(index: usize, minutiae: &mut Vec<DetMinutia>) {
    minutiae.remove(index);
}

/// Classify a feature pixel as a ridge ending or a bifurcation — port of stock `minutia_type`
/// (`minutia.c` L1167).
///
/// A white (`0`) feature pixel is a valley-ending → [`BIFURCATION`]; a black (`1`) feature pixel is a
/// [`RIDGE_ENDING`].
pub(crate) fn minutia_type(feature_pix: u8) -> i32 {
    // PORT L1171–L1178: white → bifurcation, black → ridge-ending.
    if feature_pix == 0 {
        BIFURCATION
    } else {
        RIDGE_ENDING
    }
}

/// Decide whether a minutia is appearing or disappearing from its feature/edge geometry — port of
/// stock `is_minutia_appearing` (`minutia.c` L1203).
///
/// The edge pixel always sits N/S/E/W of the feature pixel: for a horizontal scan an edge above the
/// feature is [`APPEARING`], below is [`DISAPPEARING`]; for a vertical scan an edge to the left is
/// [`APPEARING`], to the right is [`DISAPPEARING`].
///
/// # Errors
///
/// `Err(-240)` on the stock "bad configuration of pixels" path — unreachable for the cardinally
/// adjacent pairs the detector supplies, retained for a faithful transcription.
pub(crate) fn is_minutia_appearing(
    x_loc: i32,
    y_loc: i32,
    x_edge: i32,
    y_edge: i32,
) -> Result<i32, i32> {
    // PORT L1209–L1214: horizontal scan — edge above/below the feature.
    if x_edge < x_loc {
        return Ok(APPEARING);
    }
    if x_edge > x_loc {
        return Ok(DISAPPEARING);
    }
    // PORT L1217–L1222: vertical scan — edge left/right of the feature.
    if y_edge < y_loc {
        return Ok(APPEARING);
    }
    if y_edge > y_loc {
        return Ok(DISAPPEARING);
    }
    // PORT L1225–L1228: should never happen for a cardinally adjacent pair.
    Err(-240)
}

/// Choose the scan orientation for a block from its ridge-flow direction — port of stock
/// `choose_scan_direction` (`minutia.c` L1244).
///
/// Relatively vertical flow (`imapval <= ndirs/4` or `> 3*ndirs/4`) is scanned [`SCAN_HORIZONTAL`];
/// otherwise (relatively horizontal flow) it is scanned [`SCAN_VERTICAL`] — always orthogonal to the
/// ridge flow.
fn choose_scan_direction(imapval: i32, ndirs: i32) -> i32 {
    // PORT L1251: quarter of the directions in the semicircle.
    let qtr_ndirs = ndirs >> 2;
    // PORT L1256–L1264: vertical flow → horizontal scan, else vertical scan.
    if imapval <= qtr_ndirs || imapval > (qtr_ndirs * 3) {
        SCAN_HORIZONTAL
    } else {
        SCAN_VERTICAL
    }
}

/// Convert a semicircle IMAP direction to a full-circle minutia direction — port of stock
/// `get_low_curvature_direction` (`minutia.c` L3450).
///
/// A bi-directional block direction (`imapval` on `[0..ndirs]`) is disambiguated to a full-circle
/// direction on `[0..2*ndirs]` using the feature's scan orientation and appearing/disappearing
/// polarity: in each of the two flow quadrants, one (scan, polarity) combination points opposite the
/// ridge flow (`idir += ndirs`) and the other points along it.
fn get_low_curvature_direction(scan_dir: i32, appearing: bool, imapval: i32, ndirs: i32) -> i32 {
    // PORT L3459: start from the block direction.
    let mut idir = imapval;

    // PORT L3468: CASE I — ridge flow in quadrant I (`imapval <= ndirs/2`).
    if imapval <= (ndirs >> 1) {
        if scan_dir == SCAN_HORIZONTAL {
            // PORT L3473–L3486: I.A — horizontal scan, appearing points opposite the flow.
            if appearing {
                idir += ndirs;
            }
        } else {
            // PORT L3492–L3506: I.B — vertical scan, disappearing points opposite the flow.
            if !appearing {
                idir += ndirs;
            }
        }
    }
    // PORT L3512: CASE II — ridge flow in quadrant II (`imapval > ndirs/2`).
    else if scan_dir == SCAN_HORIZONTAL {
        // PORT L3517–L3530: II.A — horizontal scan, disappearing points opposite the flow.
        if !appearing {
            idir += ndirs;
        }
    } else {
        // PORT L3536–L3550: II.B — vertical scan, disappearing points opposite the flow.
        if !appearing {
            idir += ndirs;
        }
    }

    // PORT L3555: full-circle direction on `[0..2*ndirs]`.
    idir
}

/// The adjusted attributes of a high-curvature minutia, or a decision to drop it — the port's split of
/// stock `adjust_high_curvature_minutia_V2`'s out-parameters vs. its `IGNORE` return.
enum AdjustResult {
    /// The minutia was relocated to the contour's point of highest curvature with a new direction.
    Adjusted {
        idir: i32,
        x_loc: i32,
        y_loc: i32,
        x_edge: i32,
        y_edge: i32,
    },
    /// The candidate should be dropped (stock `IGNORE`).
    Ignore,
}

/// Relocate a high-curvature candidate to its point of highest curvature — port of stock
/// `adjust_high_curvature_minutia_V2` (`minutia.c` L3272).
///
/// Extracts the feature's contour ([`get_high_curvature_contour`]); if it closes a loop, a clockwise
/// loop is ignored and a counter-clockwise one is handed to [`process_loop_v2`] (which may add loop
/// minutiae or erase the loop) and the triggering candidate ignored. Otherwise the contour's point of
/// minimum interior angle ([`min_contour_theta`]) becomes the minutia's new location, provided that
/// angle is sharp enough and the interior midpoint agrees with the feature's colour; the direction is
/// the line from that point to the interior midpoint ([`line2direction`]).
///
/// PORT: the four out-parameters and the `IGNORE` return collapse into [`AdjustResult`]; `Err(i32)` is
/// a genuine (unreachable-in-port) system error.
#[expect(clippy::too_many_arguments)]
fn adjust_high_curvature_minutia_v2(
    x_loc: i32,
    y_loc: i32,
    x_edge: i32,
    y_edge: i32,
    bdata: &mut [u8],
    iw: i32,
    ih: i32,
    plow_flow_map: &[i32],
    minutiae: &mut Vec<DetMinutia>,
    lfsparms: &LfsParms,
) -> Result<AdjustResult, i32> {
    // PORT L3290: half the desired contour length.
    let half_contour = lfsparms.high_curve_half_contour;
    // PORT L3297: edge length for the curvature angle is a quarter of the full contour.
    let angle_edge = half_contour >> 1;

    // PORT L3300: feature (interior) pixel value.
    let feature_pix = bdata[(y_loc * iw + x_loc) as usize];

    // PORT L3303: extract the feature's contour.
    match get_high_curvature_contour(half_contour, x_loc, y_loc, x_edge, y_edge, bdata, iw, ih) {
        // PORT L3319–L3369: the contour forms a loop.
        HighCurvatureContour::Loop(c) => {
            // PORT L3330: default winding TRUE so an indeterminate loop is ignored (not filled).
            let ret = is_loop_clockwise(&c.x, &c.y, TRUE);
            if ret != 0 {
                // PORT L3335–L3341: system error (<0) or clockwise loop → IGNORE.
                if ret < 0 {
                    return Err(ret);
                }
                return Ok(AdjustResult::Ignore);
            }
            // PORT L3347–L3363: process the counter-clockwise loop, then ignore the trigger.
            process_loop_v2(minutiae, &c, bdata, iw, ih, plow_flow_map, lfsparms)?;
            Ok(AdjustResult::Ignore)
        }
        // PORT L3374–L3376: empty contour → IGNORE.
        HighCurvatureContour::Empty => Ok(AdjustResult::Ignore),
        HighCurvatureContour::Ok(c) => {
            // PORT L3385: point of highest curvature (minimum interior angle).
            let (min_i, min_theta) = match min_contour_theta(angle_edge, &c.x, &c.y) {
                // PORT L3387–L3393: too short → IGNORE.
                None => return Ok(AdjustResult::Ignore),
                Some(v) => v,
            };

            // PORT L3396–L3402: reject a curvature that is too gentle.
            if min_theta >= lfsparms.max_high_curve_theta {
                return Ok(AdjustResult::Ignore);
            }

            // PORT L3407–L3416: interior midpoint symmetric about the minimum-theta point must match
            // the feature's colour.
            let mid_x =
                (c.x[(min_i - angle_edge) as usize] + c.x[(min_i + angle_edge) as usize]) >> 1;
            let mid_y =
                (c.y[(min_i - angle_edge) as usize] + c.y[(min_i + angle_edge) as usize]) >> 1;
            let mid_pix = bdata[(mid_y * iw + mid_x) as usize];
            if mid_pix != feature_pix {
                return Ok(AdjustResult::Ignore);
            }

            // PORT L3420–L3428: new direction and relocated feature/edge points.
            let idir = line2direction(
                c.x[min_i as usize],
                c.y[min_i as usize],
                mid_x,
                mid_y,
                lfsparms.num_directions,
            );
            Ok(AdjustResult::Adjusted {
                idir,
                x_loc: c.x[min_i as usize],
                y_loc: c.y[min_i as usize],
                x_edge: c.ex[min_i as usize],
                y_edge: c.ey[min_i as usize],
            })
        }
    }
}

/// Add a detected minutia to the list unless a compatible one is already present — port of stock
/// `update_minutiae` (**version one**, `minutia.c` L355), used by the loop processor.
///
/// Scans the list (forward) for a same-type minutia within `max_minutia_delta` in X and Y whose
/// direction is within 45°; if such a neighbour shares the point exactly, or lies on the same contour
/// (searched both ways up to `max_minutia_delta` steps), the new minutia is dropped. Otherwise it is
/// appended.
///
/// PORT: the stock `realloc`/`IGNORE`/`0` returns collapse into `Vec::push` vs. an early `Ok(())`
/// (the dropped minutia is simply not pushed).
pub(crate) fn update_minutiae(
    minutiae: &mut Vec<DetMinutia>,
    minutia: DetMinutia,
    bdata: &[u8],
    iw: i32,
    ih: i32,
    lfsparms: &LfsParms,
) -> Result<(), i32> {
    // PORT L376/L379: 45° = quarter of the semicircle; full circle = 2*ndirs.
    let qtr_ndirs = lfsparms.num_directions >> 2;
    let full_ndirs = lfsparms.num_directions << 1;

    // PORT L385–L448: forward scan for a compatible existing minutia.
    for existing in minutiae.iter() {
        let dx = (existing.x - minutia.x).abs();
        if dx >= lfsparms.max_minutia_delta {
            continue;
        }
        let dy = (existing.y - minutia.y).abs();
        if dy >= lfsparms.max_minutia_delta {
            continue;
        }
        if existing.kind != minutia.kind {
            continue;
        }
        // PORT L403–L409: inner direction difference within 45°.
        let mut delta_dir = (existing.direction - minutia.direction).abs();
        delta_dir = delta_dir.min(full_ndirs - delta_dir);
        if delta_dir > qtr_ndirs {
            continue;
        }
        // PORT L412–L418: exact same point → drop.
        if dx == 0 && dy == 0 {
            return Ok(());
        }
        // PORT L423–L446: same contour either way → drop.
        if search_contour(
            minutia.x,
            minutia.y,
            lfsparms.max_minutia_delta,
            existing.x,
            existing.y,
            existing.ex,
            existing.ey,
            ScanDir::Clockwise,
            bdata,
            iw,
            ih,
        ) || search_contour(
            minutia.x,
            minutia.y,
            lfsparms.max_minutia_delta,
            existing.x,
            existing.y,
            existing.ex,
            existing.ey,
            ScanDir::CounterClockwise,
            bdata,
            iw,
            ih,
        ) {
            return Ok(());
        }
    }

    // PORT L453–L457: not already present → append.
    minutiae.push(minutia);
    Ok(())
}

/// Add a detected minutia to the list, preferring the more-compatible of near-duplicate pairs — port
/// of stock `update_minutiae_V2` (`minutia.c` L474), used by the V2 scans.
///
/// Like [`update_minutiae`] but scans the list in **reverse** and, when the new minutia shares a
/// contour with an existing one, keeps whichever is compatible with the block's ridge-flow scan
/// direction: if the new point's scan direction matches, the existing one is removed and the new one
/// added; if not, the new one is dropped; if the block direction is invalid, the new one is dropped.
///
/// PORT: the stock `realloc`/`IGNORE`/`0` returns collapse into `Vec::push`/`Vec::remove` vs. an early
/// `Ok(())`. The reverse walk with in-place removal is preserved so list order matches the reference.
#[expect(clippy::too_many_arguments)]
pub(crate) fn update_minutiae_v2(
    minutiae: &mut Vec<DetMinutia>,
    minutia: DetMinutia,
    scan_dir: i32,
    dmapval: i32,
    bdata: &[u8],
    iw: i32,
    ih: i32,
    lfsparms: &LfsParms,
) -> Result<(), i32> {
    // PORT L509/L512: 45° = quarter of the semicircle; full circle = 2*ndirs.
    let qtr_ndirs = lfsparms.num_directions >> 2;
    let full_ndirs = lfsparms.num_directions << 1;

    // PORT L517–L604: reverse scan for a compatible existing minutia.
    let mut i = minutiae.len() as i32 - 1;
    while i >= 0 {
        let idx = i as usize;
        let dx = (minutiae[idx].x - minutia.x).abs();
        let dy = (minutiae[idx].y - minutia.y).abs();
        let same_type = minutiae[idx].kind == minutia.kind;
        if dx < lfsparms.max_minutia_delta && dy < lfsparms.max_minutia_delta && same_type {
            // PORT L531–L537: inner direction difference within 45°.
            let mut delta_dir = (minutiae[idx].direction - minutia.direction).abs();
            delta_dir = delta_dir.min(full_ndirs - delta_dir);
            if delta_dir <= qtr_ndirs {
                // PORT L540–L546: exact same point → drop.
                if dx == 0 && dy == 0 {
                    return Ok(());
                }
                // PORT L551–L568: do they share the same contour (searched both ways)?
                let same_contour = search_contour(
                    minutia.x,
                    minutia.y,
                    lfsparms.max_minutia_delta,
                    minutiae[idx].x,
                    minutiae[idx].y,
                    minutiae[idx].ex,
                    minutiae[idx].ey,
                    ScanDir::Clockwise,
                    bdata,
                    iw,
                    ih,
                ) || search_contour(
                    minutia.x,
                    minutia.y,
                    lfsparms.max_minutia_delta,
                    minutiae[idx].x,
                    minutiae[idx].y,
                    minutiae[idx].ex,
                    minutiae[idx].ey,
                    ScanDir::CounterClockwise,
                    bdata,
                    iw,
                    ih,
                );
                if same_contour {
                    // PORT L570–L595: choose between the two on the block scan direction.
                    if dmapval >= 0 {
                        let map_scan_dir = choose_scan_direction(dmapval, lfsparms.num_directions);
                        if map_scan_dir == scan_dir {
                            // PORT L583–L588: new point wins — remove the existing one, keep going.
                            remove_minutia(idx, minutiae);
                        } else {
                            // PORT L591–L594: keep the existing one, drop the new one.
                            return Ok(());
                        }
                    } else {
                        // PORT L598–L604: invalid block direction → drop the new one.
                        return Ok(());
                    }
                }
            }
        }
        i -= 1;
    }

    // PORT L615–L619: not already present (or duplicates removed) → append.
    minutiae.push(minutia);
    Ok(())
}

/// Process a candidate detected by the horizontal scan — port of stock
/// `process_horizontal_scan_minutia_V2` (`minutia.c` L2716).
///
/// Places the minutia half-way between the second and third feature pair (`x_loc = (cx + x2) / 2`),
/// derives its direction (locally adjusted in a high-curvature block, else from the block direction),
/// sets reliability from the Low Ridge Flow map, and hands it to [`update_minutiae_v2`]. An invalid
/// block direction or an [`AdjustResult::Ignore`] drops the candidate.
///
/// # Errors
///
/// Propagates negative stock system-error codes (unreachable in the port).
#[expect(clippy::too_many_arguments)]
fn process_horizontal_scan_minutia_v2(
    minutiae: &mut Vec<DetMinutia>,
    cx: i32,
    cy: i32,
    x2: i32,
    feature_id: usize,
    bdata: &mut [u8],
    iw: i32,
    ih: i32,
    pdirection_map: &[i32],
    plow_flow_map: &[i32],
    phigh_curve_map: &[i32],
    lfsparms: &LfsParms,
) -> Result<(), i32> {
    let fp = &FEATURE_PATTERNS[feature_id];

    // PORT L2735–L2739: x half-way between 2nd and 3rd pair; edge shares the x.
    let mut x_loc = (cx + x2) >> 1;
    let mut x_edge = x_loc;

    // PORT L2743–L2757: appearing → feature on the 2nd row (edge on the 1st), else reversed.
    let (mut y_loc, mut y_edge) = if fp.appearing {
        (cy + 1, cy)
    } else {
        (cy, cy + 1)
    };

    // PORT L2759–L2761: map values at the feature point.
    let dmapval = pdirection_map[(y_loc * iw + x_loc) as usize];
    let fmapval = plow_flow_map[(y_loc * iw + x_loc) as usize];
    let cmapval = phigh_curve_map[(y_loc * iw + x_loc) as usize];

    // PORT L2764–L2766: invalid block direction → drop.
    if dmapval == INVALID_DIR {
        return Ok(());
    }

    // PORT L2769–L2783: high-curvature block → adjust locally, else derive from block direction.
    let idir;
    if cmapval != 0 {
        match adjust_high_curvature_minutia_v2(
            x_loc,
            y_loc,
            x_edge,
            y_edge,
            bdata,
            iw,
            ih,
            plow_flow_map,
            minutiae,
            lfsparms,
        )? {
            AdjustResult::Ignore => return Ok(()),
            AdjustResult::Adjusted {
                idir: d,
                x_loc: xl,
                y_loc: yl,
                x_edge: xe,
                y_edge: ye,
            } => {
                idir = d;
                x_loc = xl;
                y_loc = yl;
                x_edge = xe;
                y_edge = ye;
            }
        }
    } else {
        idir = get_low_curvature_direction(
            SCAN_HORIZONTAL,
            fp.appearing,
            dmapval,
            lfsparms.num_directions,
        );
    }

    // PORT L2786–L2791: reliability from the (original) Low Ridge Flow value.
    let reliability = if fmapval != 0 {
        MEDIUM_RELIABILITY
    } else {
        HIGH_RELIABILITY
    };

    // PORT L2794–L2807: create the minutia and offer it to the list.
    let minutia = create_minutia(
        x_loc,
        y_loc,
        x_edge,
        y_edge,
        idir,
        reliability,
        fp.kind,
        fp.appearing,
        feature_id as i32,
    );
    update_minutiae_v2(
        minutiae,
        minutia,
        SCAN_HORIZONTAL,
        dmapval,
        bdata,
        iw,
        ih,
        lfsparms,
    )
}

/// Process a candidate detected by the vertical scan — port of stock
/// `process_vertical_scan_minutia_V2` (`minutia.c` L2942).
///
/// The vertical analogue of [`process_horizontal_scan_minutia_v2`]: the minutia is placed half-way
/// between the second and third pair on the Y axis (`y_loc = (cy + y2) / 2`), and appearing/
/// disappearing shifts the X coordinate instead of the Y.
///
/// # Errors
///
/// Propagates negative stock system-error codes (unreachable in the port).
#[expect(clippy::too_many_arguments)]
fn process_vertical_scan_minutia_v2(
    minutiae: &mut Vec<DetMinutia>,
    cx: i32,
    cy: i32,
    y2: i32,
    feature_id: usize,
    bdata: &mut [u8],
    iw: i32,
    ih: i32,
    pdirection_map: &[i32],
    plow_flow_map: &[i32],
    phigh_curve_map: &[i32],
    lfsparms: &LfsParms,
) -> Result<(), i32> {
    let fp = &FEATURE_PATTERNS[feature_id];

    // PORT L2960–L2974: appearing → feature on the 2nd column (edge on the 1st), else reversed.
    let (mut x_loc, mut x_edge) = if fp.appearing {
        (cx + 1, cx)
    } else {
        (cx, cx + 1)
    };

    // PORT L2979–L2981: y half-way between 2nd and 3rd pair; edge shares the y.
    let mut y_loc = (cy + y2) >> 1;
    let mut y_edge = y_loc;

    // PORT L2983–L2985: map values at the feature point.
    let dmapval = pdirection_map[(y_loc * iw + x_loc) as usize];
    let fmapval = plow_flow_map[(y_loc * iw + x_loc) as usize];
    let cmapval = phigh_curve_map[(y_loc * iw + x_loc) as usize];

    // PORT L2988–L2990: invalid block direction → drop.
    if dmapval == INVALID_DIR {
        return Ok(());
    }

    // PORT L2993–L3007: high-curvature block → adjust locally, else derive from block direction.
    let idir;
    if cmapval != 0 {
        match adjust_high_curvature_minutia_v2(
            x_loc,
            y_loc,
            x_edge,
            y_edge,
            bdata,
            iw,
            ih,
            plow_flow_map,
            minutiae,
            lfsparms,
        )? {
            AdjustResult::Ignore => return Ok(()),
            AdjustResult::Adjusted {
                idir: d,
                x_loc: xl,
                y_loc: yl,
                x_edge: xe,
                y_edge: ye,
            } => {
                idir = d;
                x_loc = xl;
                y_loc = yl;
                x_edge = xe;
                y_edge = ye;
            }
        }
    } else {
        idir = get_low_curvature_direction(
            SCAN_VERTICAL,
            fp.appearing,
            dmapval,
            lfsparms.num_directions,
        );
    }

    // PORT L3010–L3015: reliability from the (original) Low Ridge Flow value.
    let reliability = if fmapval != 0 {
        MEDIUM_RELIABILITY
    } else {
        HIGH_RELIABILITY
    };

    // PORT L3018–L3031: create the minutia and offer it to the list.
    let minutia = create_minutia(
        x_loc,
        y_loc,
        x_edge,
        y_edge,
        idir,
        reliability,
        fp.kind,
        fp.appearing,
        feature_id as i32,
    );
    update_minutiae_v2(
        minutiae,
        minutia,
        SCAN_VERTICAL,
        dmapval,
        bdata,
        iw,
        ih,
        lfsparms,
    )
}

/// Scan the whole binary image horizontally for minutiae — port of stock
/// `scan4minutiae_horizontally_V2` (`minutia.c` L1508).
///
/// Walks every adjacent row pair top-to-bottom, left-to-right, matching the three feature pixel pairs
/// (skipping runs of the repeated second pair) and processing each hit via
/// [`process_horizontal_scan_minutia_v2`]. `bdata` is `&mut` because a detected loop can erase itself
/// from the image mid-scan, exactly as in the reference.
#[expect(clippy::too_many_arguments)]
fn scan4minutiae_horizontally_v2(
    minutiae: &mut Vec<DetMinutia>,
    bdata: &mut [u8],
    iw: i32,
    ih: i32,
    pdirection_map: &[i32],
    plow_flow_map: &[i32],
    phigh_curve_map: &[i32],
    lfsparms: &LfsParms,
) -> Result<(), i32> {
    // PORT L1519–L1522: scan region is the entire image.
    let (sx, ex, sy, ey) = (0, iw, 0, ih);

    // PORT L1525–L1601: for each adjacent row pair (top row `cy`, bottom row `cy+1`).
    let mut cy = sy;
    while cy + 1 < ey {
        let mut cx = sx;
        // PORT L1530: walk the current scan row.
        while cx < ex {
            // PORT L1533–L1535: pixel pair (top, bottom) at the current column.
            let mut p1 = (cy * iw + cx) as usize;
            let mut p2 = ((cy + 1) * iw + cx) as usize;
            // PORT L1538: first pair matches one or more features?
            let possible = match_1st_pair(bdata[p1], bdata[p2]);
            if !possible.is_empty() {
                // PORT L1540–L1543: bump to the next pixel pair.
                cx += 1;
                p1 += 1;
                p2 += 1;
                // PORT L1545: still on the scan row?
                if cx < ex {
                    // PORT L1548: second pair matches?
                    let possible = match_2nd_pair(bdata[p1], bdata[p2], &possible);
                    if !possible.is_empty() {
                        // PORT L1551–L1554: remember x, then skip repeated second pairs.
                        let x2 = cx;
                        skip_repeated_horizontal_pair(&mut cx, ex, &mut p1, &mut p2, bdata);
                        // PORT L1556: still on the scan row?
                        if cx < ex {
                            // PORT L1559: third pair matches a single feature?
                            let possible = match_3rd_pair(bdata[p1], bdata[p2], &possible);
                            if !possible.is_empty() {
                                // PORT L1562–L1573: process the detected minutia.
                                process_horizontal_scan_minutia_v2(
                                    minutiae,
                                    cx,
                                    cy,
                                    x2,
                                    possible[0],
                                    bdata,
                                    iw,
                                    ih,
                                    pdirection_map,
                                    plow_flow_map,
                                    phigh_curve_map,
                                    lfsparms,
                                )?;
                            }
                            // PORT L1578–L1585: if the 3rd pair differs, back up one so it can seed
                            // the next first-pair test.
                            if bdata[p1] != bdata[p2] {
                                cx -= 1;
                            }
                        }
                    }
                }
            } else {
                // PORT L1595–L1597: first pair failed → next column.
                cx += 1;
            }
        }
        // PORT L1600: next scan row.
        cy += 1;
    }

    Ok(())
}

/// Scan the whole binary image vertically for minutiae — port of stock
/// `scan4minutiae_vertically_V2` (`minutia.c` L1769).
///
/// The vertical analogue of [`scan4minutiae_horizontally_v2`]: walks every adjacent column pair
/// left-to-right, top-to-bottom, stepping one image row per pixel pair.
#[expect(clippy::too_many_arguments)]
fn scan4minutiae_vertically_v2(
    minutiae: &mut Vec<DetMinutia>,
    bdata: &mut [u8],
    iw: i32,
    ih: i32,
    pdirection_map: &[i32],
    plow_flow_map: &[i32],
    phigh_curve_map: &[i32],
    lfsparms: &LfsParms,
) -> Result<(), i32> {
    // PORT L1781–L1784: scan region is the entire image.
    let (sx, ex, sy, ey) = (0, iw, 0, ih);
    let step = iw as usize;

    // PORT L1787–L1863: for each adjacent column pair (left column `cx`, right column `cx+1`).
    let mut cx = sx;
    while cx + 1 < ex {
        let mut cy = sy;
        // PORT L1792: walk the current scan column.
        while cy < ey {
            // PORT L1795–L1797: pixel pair (left, right) at the current row.
            let mut p1 = (cy * iw + cx) as usize;
            let mut p2 = p1 + 1;
            // PORT L1800: first pair matches one or more features?
            let possible = match_1st_pair(bdata[p1], bdata[p2]);
            if !possible.is_empty() {
                // PORT L1802–L1805: bump down one image row to the next pixel pair.
                cy += 1;
                p1 += step;
                p2 += step;
                // PORT L1807: still on the scan column?
                if cy < ey {
                    // PORT L1810: second pair matches?
                    let possible = match_2nd_pair(bdata[p1], bdata[p2], &possible);
                    if !possible.is_empty() {
                        // PORT L1813–L1816: remember y, then skip repeated second pairs.
                        let y2 = cy;
                        skip_repeated_vertical_pair(&mut cy, ey, &mut p1, &mut p2, iw, bdata);
                        // PORT L1818: still on the scan column?
                        if cy < ey {
                            // PORT L1821: third pair matches a single feature?
                            let possible = match_3rd_pair(bdata[p1], bdata[p2], &possible);
                            if !possible.is_empty() {
                                // PORT L1824–L1835: process the detected minutia.
                                process_vertical_scan_minutia_v2(
                                    minutiae,
                                    cx,
                                    cy,
                                    y2,
                                    possible[0],
                                    bdata,
                                    iw,
                                    ih,
                                    pdirection_map,
                                    plow_flow_map,
                                    phigh_curve_map,
                                    lfsparms,
                                )?;
                            }
                            // PORT L1840–L1847: if the 3rd pair differs, back up one so it can seed
                            // the next first-pair test.
                            if bdata[p1] != bdata[p2] {
                                cy -= 1;
                            }
                        }
                    }
                }
            } else {
                // PORT L1857–L1859: first pair failed → next row.
                cy += 1;
            }
        }
        // PORT L1862: next scan column.
        cx += 1;
    }

    Ok(())
}

/// Expand a block map to per-pixel resolution — the stock `pixelize_map` (`maps.c` L724).
///
/// Assigns every pixel the value of the block it falls in, so block-map values can be addressed by
/// pixel coordinate during the scan. It lives with the detection stage (rather than in `maps`)
/// because detection is its only consumer — `maps` owns block-map *generation*, this owns their
/// *pixelization*. Recovers the block grid via `block_offsets` (with zero padding, as in the
/// reference) and requires it to match `mw`×`mh`.
///
/// # Errors
///
/// `Err(-591)` if the recovered block grid does not match `mw`×`mh`; otherwise propagates the
/// `block_offsets` error codes.
pub(crate) fn pixelize_map(
    iw: i32,
    ih: i32,
    imap: &[i32],
    mw: i32,
    mh: i32,
    blocksize: i32,
) -> Result<Vec<i32>, i32> {
    // PORT L731–L734: output pixel map, one value per pixel.
    let mut pmap = vec![0i32; (iw * ih) as usize];

    // PORT L738–L740: recover the (unpadded) block grid.
    let bo = block_offsets(iw, ih, 0, blocksize)?;

    // PORT L742–L747: the recovered grid must match the map dimensions.
    if bo.map_w != mw || bo.map_h != mh {
        return Err(-591);
    }

    // PORT L749–L758: paint each block's value across its `blocksize`×`blocksize` pixels.
    for (bi, &val) in imap.iter().enumerate().take((mw * mh) as usize) {
        for y in 0..blocksize {
            let start = (bo.offsets[bi] + y * iw) as usize;
            pmap[start..start + blocksize as usize].fill(val);
        }
    }

    // PORT L765: return the pixelized map.
    Ok(pmap)
}

/// Scan a binary image for minutiae — the stock `detect_minutiae_V2` (`minutia.c` L283).
///
/// Pixelizes the three block maps ([`pixelize_map`]), then scans the image horizontally and then
/// vertically for the 2×3 feature patterns, collecting every accepted point into the returned list
/// (points in `LOW FLOW` blocks are recorded with lower reliability). `bdata` is the binary image in
/// stock convention (`0 == white/valley`, `1 == black/ridge`), `iw`×`ih` pixels; it is `&mut` because
/// loop-fill can rewrite it mid-scan. `direction_map`, `low_flow_map`, and `high_curve_map` are each
/// `mw`×`mh` blocks in row-major order.
///
/// # Errors
///
/// Propagates the negative stock error codes surfaced by [`pixelize_map`] and the scan routines.
#[expect(clippy::too_many_arguments)]
pub(crate) fn detect_minutiae_v2(
    bdata: &mut [u8],
    iw: i32,
    ih: i32,
    direction_map: &[i32],
    low_flow_map: &[i32],
    high_curve_map: &[i32],
    mw: i32,
    mh: i32,
    lfsparms: &LfsParms,
) -> Result<Vec<DetMinutia>, i32> {
    // PORT L292–L308: pixelize the three block maps.
    let pdirection_map = pixelize_map(iw, ih, direction_map, mw, mh, lfsparms.blocksize)?;
    let plow_flow_map = pixelize_map(iw, ih, low_flow_map, mw, mh, lfsparms.blocksize)?;
    let phigh_curve_map = pixelize_map(iw, ih, high_curve_map, mw, mh, lfsparms.blocksize)?;

    let mut minutiae = Vec::new();

    // PORT L310–L324: horizontal scan first, then vertical.
    scan4minutiae_horizontally_v2(
        &mut minutiae,
        bdata,
        iw,
        ih,
        &pdirection_map,
        &plow_flow_map,
        &phigh_curve_map,
        lfsparms,
    )?;
    scan4minutiae_vertically_v2(
        &mut minutiae,
        bdata,
        iw,
        ih,
        &pdirection_map,
        &plow_flow_map,
        &phigh_curve_map,
        lfsparms,
    )?;

    // PORT L333: raw minutia list (before false-minutia removal).
    Ok(minutiae)
}
