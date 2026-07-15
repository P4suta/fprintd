// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Fixed algorithm constants copied verbatim from stock NBIS `lfs.h` (block/window sizes,
//! `NUM_DIRECTIONS`, DFT wave count, grid dimensions, `PAD_VALUE`, `MAX_MINUTIAE`, `TRUNC_SCALE`).
//! Each value is an interoperability fact; comments cite the reference macro.
//!
//! These are the `#define`s from `mindtct/include/lfs.h` (and the `dft_coefs` array from
//! `globals.c`) that feed the **VERSION 2** parameter block `lfsparms_V2` — the only configuration
//! MINDTCT ships with — plus the handful of fixed geometry/scale constants the pipeline reads
//! directly. The V2 struct leaves the whole *minutiae-link* group and `isobin_grid_dim` as
//! `UNUSED_INT`/`UNUSED_DBL`, so those link-stage macros are deliberately **absent** here.
//!
//! Types follow the C `LFSPARMS` struct: `int` fields → `i32`, `double` fields → `f64`. The one
//! capacity value (`MAX_MINUTIAE`) is a `usize`, matching its role as an allocation cap. See
//! `docs/mindtct-algorithm.md`.

// ---------------------------------------------------------------------------
// Image controls
// ---------------------------------------------------------------------------

/// `PAD_VALUE` — medium-gray (`128` @ 8 bits) used to pad the image border before block analysis.
pub(crate) const PAD_VALUE: i32 = 128;

/// `JOIN_LINE_RADIUS` — half-width of the line drawn when joining two minutiae.
pub(crate) const JOIN_LINE_RADIUS: i32 = 1;

// ---------------------------------------------------------------------------
// Map controls (VERSION 2)
// ---------------------------------------------------------------------------

/// `MAP_BLOCKSIZE_V2` — pixel dimension of an image block (`8`).
pub(crate) const MAP_BLOCKSIZE_V2: i32 = 8;

/// `MAP_WINDOWSIZE_V2` — pixel dimension of the window surrounding a block (`24`).
///
/// Note the V2 geometry invariant `windowsize == blocksize + 2 * windowoffset`; see the tests.
pub(crate) const MAP_WINDOWSIZE_V2: i32 = 24;

/// `MAP_WINDOWOFFSET_V2` — offset in X & Y from a block to its window origin (`8`).
pub(crate) const MAP_WINDOWOFFSET_V2: i32 = 8;

/// `NUM_DIRECTIONS` — number of discrete ridge-flow orientations sampled (`16`).
pub(crate) const NUM_DIRECTIONS: i32 = 16;

/// `START_DIR_ANGLE` — orientation of direction index `0`, `M_PI/2` radians (90°).
///
/// Stock defines this as `(double)(M_PI/2.0)`; `std::f64::consts::PI` is the exact `f64` nearest to
/// the same `M_PI` literal, so the bit pattern matches.
pub(crate) const START_DIR_ANGLE: f64 = std::f64::consts::PI / 2.0;

/// `RMV_VALID_NBR_MIN` — min valid neighbors before an isolated block direction is removed.
pub(crate) const RMV_VALID_NBR_MIN: i32 = 3;

/// `DIR_STRENGTH_MIN` — minimum direction strength for a block direction to be valid.
pub(crate) const DIR_STRENGTH_MIN: f64 = 0.2;

/// `DIR_DISTANCE_MAX` — max block distance over which directions are interpolated.
pub(crate) const DIR_DISTANCE_MAX: i32 = 3;

/// `SMTH_VALID_NBR_MIN` — min valid neighbors required to smooth a block direction.
pub(crate) const SMTH_VALID_NBR_MIN: i32 = 7;

/// `VORT_VALID_NBR_MIN` — min valid neighbors required for a vorticity measure.
pub(crate) const VORT_VALID_NBR_MIN: i32 = 7;

/// `HIGHCURV_VORTICITY_MIN` — min vorticity marking a block high-curvature.
pub(crate) const HIGHCURV_VORTICITY_MIN: i32 = 5;

/// `HIGHCURV_CURVATURE_MIN` — min curvature marking a block high-curvature.
pub(crate) const HIGHCURV_CURVATURE_MIN: i32 = 5;

/// `MIN_INTERPOLATE_NBRS` — min valid neighbors required to interpolate a direction.
pub(crate) const MIN_INTERPOLATE_NBRS: i32 = 2;

/// `PERCENTILE_MIN_MAX` — percentile used to derive the min/max block intensities.
pub(crate) const PERCENTILE_MIN_MAX: i32 = 10;

/// `MIN_CONTRAST_DELTA` — min intensity delta before a block is considered non-low-contrast.
pub(crate) const MIN_CONTRAST_DELTA: i32 = 5;

// ---------------------------------------------------------------------------
// DFT controls
// ---------------------------------------------------------------------------

/// `NUM_DFT_WAVES` — number of DFT frequencies analyzed per block window (`4`).
pub(crate) const NUM_DFT_WAVES: i32 = 4;

/// `dft_coefs` — the `NUM_DFT_WAVES` frequency coefficients `C`, each a multiple of `PI_FACTOR`
/// (`globals.c`). Stored as `double`; one/two/three/four periods across the window range.
pub(crate) const DFT_COEFS: [f64; NUM_DFT_WAVES as usize] = [1.0, 2.0, 3.0, 4.0];

/// `POWMAX_MIN` — min DFT power maximum for a block to carry a dominant direction.
pub(crate) const POWMAX_MIN: f64 = 100_000.0;

/// `POWNORM_MIN` — min normalized DFT power for a dominant direction.
pub(crate) const POWNORM_MIN: f64 = 3.8;

/// `POWMAX_MAX` — DFT power maximum above which a block is treated as high-flow.
pub(crate) const POWMAX_MAX: f64 = 50_000_000.0;

/// `FORK_INTERVAL` — direction interval used when testing for a fork.
pub(crate) const FORK_INTERVAL: i32 = 2;

/// `FORK_PCT_POWMAX` — fraction of powmax a fork's neighbor must reach.
pub(crate) const FORK_PCT_POWMAX: f64 = 0.7;

/// `FORK_PCT_POWNORM` — fraction of pownorm a fork's neighbor must reach.
pub(crate) const FORK_PCT_POWNORM: f64 = 0.75;

// ---------------------------------------------------------------------------
// Binarization controls
// ---------------------------------------------------------------------------

/// `DIRBIN_GRID_W` — width of the directional-binarization grid (`7`).
pub(crate) const DIRBIN_GRID_W: i32 = 7;

/// `DIRBIN_GRID_H` — height of the directional-binarization grid (`9`).
pub(crate) const DIRBIN_GRID_H: i32 = 9;

/// `NUM_FILL_HOLES` — number of hole-filling passes over the binarized image.
pub(crate) const NUM_FILL_HOLES: i32 = 3;

// ---------------------------------------------------------------------------
// Minutiae detection controls
// ---------------------------------------------------------------------------

/// `MAX_MINUTIA_DELTA` — max pixel delta permitted between adjacent contour points.
pub(crate) const MAX_MINUTIA_DELTA: i32 = 10;

/// `MAX_HIGH_CURVE_THETA` — max angle (`M_PI/3` rad) still counted as one high-curvature turn.
///
/// Stock defines this as `(double)(M_PI/3.0)`.
pub(crate) const MAX_HIGH_CURVE_THETA: f64 = std::f64::consts::PI / 3.0;

/// `HIGH_CURVE_HALF_CONTOUR` — half-length of the contour walked to test high curvature.
pub(crate) const HIGH_CURVE_HALF_CONTOUR: i32 = 14;

/// `MIN_LOOP_LEN` — min contour length for a loop to be considered.
pub(crate) const MIN_LOOP_LEN: i32 = 20;

/// `MIN_LOOP_ASPECT_DIST` — min half-way distance across a loop before its aspect is tested.
pub(crate) const MIN_LOOP_ASPECT_DIST: f64 = 1.0;

/// `MIN_LOOP_ASPECT_RATIO` — min max/min half-way distance ratio to reject a loop.
pub(crate) const MIN_LOOP_ASPECT_RATIO: f64 = 2.25;

// ---------------------------------------------------------------------------
// Minutiae link controls
// ---------------------------------------------------------------------------
//
// The V2 block wires the whole link group to UNUSED_INT/UNUSED_DBL except `maxtrans`, which is
// reused for overlap removal. Only that one survives here.

/// `MAXTRANS` — max direction transitions tolerated (reused for removing overlaps in V2).
pub(crate) const MAXTRANS: i32 = 2;

// ---------------------------------------------------------------------------
// False-minutiae removal controls (VERSION 2)
// ---------------------------------------------------------------------------

/// `MAX_RMTEST_DIST_V2` — max distance between two minutiae still tested for removal (`16`).
pub(crate) const MAX_RMTEST_DIST_V2: i32 = 16;

/// `MAX_HOOK_LEN_V2` — max contour length of a hook artifact to remove (`30`).
pub(crate) const MAX_HOOK_LEN_V2: i32 = 30;

/// `MAX_HALF_LOOP_V2` — max half-loop contour length treated as an artifact (`30`).
pub(crate) const MAX_HALF_LOOP_V2: i32 = 30;

/// `TRANS_DIR_PIX_V2` — pixel step used when tracing minutia direction transitions (`4`).
pub(crate) const TRANS_DIR_PIX_V2: i32 = 4;

/// `SMALL_LOOP_LEN` — max contour length of a small loop removed outright.
pub(crate) const SMALL_LOOP_LEN: i32 = 15;

/// `SIDE_HALF_CONTOUR` — half-contour length used to locate a loop's side points.
pub(crate) const SIDE_HALF_CONTOUR: i32 = 7;

/// `INV_BLOCK_MARGIN_V2` — margin (in pixels) from an invalid block within which minutiae are
/// dropped (`4`).
pub(crate) const INV_BLOCK_MARGIN_V2: i32 = 4;

/// `RM_VALID_NBR_MIN` — min valid neighbor blocks a minutia must have to survive.
pub(crate) const RM_VALID_NBR_MIN: i32 = 7;

/// `MAX_OVERLAP_DIST` — max distance between two minutiae tested as an overlap.
pub(crate) const MAX_OVERLAP_DIST: i32 = 8;

/// `MAX_OVERLAP_JOIN_DIST` — max distance over which an overlap pair may be joined.
pub(crate) const MAX_OVERLAP_JOIN_DIST: i32 = 6;

/// `MALFORMATION_STEPS_1` — first contour step count used in malformation testing.
pub(crate) const MALFORMATION_STEPS_1: i32 = 10;

/// `MALFORMATION_STEPS_2` — second contour step count used in malformation testing.
pub(crate) const MALFORMATION_STEPS_2: i32 = 20;

/// `MIN_MALFORMATION_RATIO` — min ratio of the two malformation-step distances.
pub(crate) const MIN_MALFORMATION_RATIO: f64 = 2.0;

/// `MAX_MALFORMATION_DIST` — max distance over which malformation is tested.
pub(crate) const MAX_MALFORMATION_DIST: i32 = 20;

/// `PORES_TRANS_R` — translation radius used when testing for pores.
pub(crate) const PORES_TRANS_R: i32 = 3;

/// `PORES_PERP_STEPS` — perpendicular step count used when testing for pores.
pub(crate) const PORES_PERP_STEPS: i32 = 12;

/// `PORES_STEPS_FWD` — forward contour steps walked when testing for pores.
pub(crate) const PORES_STEPS_FWD: i32 = 10;

/// `PORES_STEPS_BWD` — backward contour steps walked when testing for pores.
pub(crate) const PORES_STEPS_BWD: i32 = 8;

/// `PORES_MIN_DIST2` — min squared distance separating a valid pore pair.
pub(crate) const PORES_MIN_DIST2: f64 = 0.5;

/// `PORES_MAX_RATIO` — max ratio of forward/backward pore distances.
pub(crate) const PORES_MAX_RATIO: f64 = 2.25;

// ---------------------------------------------------------------------------
// Ridge-counting controls
// ---------------------------------------------------------------------------

/// `MAX_NBRS` — max nearest-neighbor minutiae considered for ridge counting.
pub(crate) const MAX_NBRS: i32 = 5;

/// `MAX_RIDGE_STEPS` — max steps walked between two minutiae when counting ridges.
pub(crate) const MAX_RIDGE_STEPS: i32 = 10;

// ---------------------------------------------------------------------------
// Fixed geometry / scale constants (read directly, outside lfsparms)
// ---------------------------------------------------------------------------

/// `TRUNC_SCALE` — the `1/16384` quantization scale for `trunc_dbl_precision`, matching the FORTRAN
/// reference implementation's fixed precision (see `num::trunc_dbl_precision`).
pub(crate) const TRUNC_SCALE: f64 = 16384.0;

/// `MAX_MINUTIAE` — hard cap on the number of minutiae detected (allocation bound), so kept as a
/// `usize`.
// `dead_code`: the stock allocation bound. The port grows minutiae in a `Vec` rather than a
// fixed-size array, so nothing in the pipeline reads this cap; it is transcribed for fidelity to
// `lfs.h` and pinned by the test below.
#[allow(dead_code)]
pub(crate) const MAX_MINUTIAE: usize = 1000;

#[cfg(test)]
mod tests {
    use super::*;

    /// Spot-check the interoperability facts that name the task: a typo here silently desyncs the
    /// whole detector from stock NBIS `xyt` output.
    #[test]
    fn named_fixed_values() {
        assert_eq!(PAD_VALUE, 128);
        assert_eq!(NUM_DIRECTIONS, 16);
        assert_eq!(NUM_DFT_WAVES, 4);
        assert_eq!(MAX_MINUTIAE, 1000);
        assert_eq!(TRUNC_SCALE, 16384.0);
        assert_eq!(MAP_BLOCKSIZE_V2, 8);
        assert_eq!(MAP_WINDOWSIZE_V2, 24);
        assert_eq!(MAP_WINDOWOFFSET_V2, 8);
    }

    /// The `dft_coefs` array is `{1,2,3,4}` and exactly `NUM_DFT_WAVES` long.
    #[test]
    fn dft_coefs_match_globals() {
        assert_eq!(DFT_COEFS, [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(DFT_COEFS.len(), NUM_DFT_WAVES as usize);
    }

    /// Stock derives both angles from `M_PI`; confirm the `std::f64::consts::PI` substitution is the
    /// same `f64` and the arithmetic (`/2`, `/3`) lands on the expected radians.
    #[test]
    fn angles_from_pi() {
        assert_eq!(START_DIR_ANGLE, std::f64::consts::FRAC_PI_2);
        assert_eq!(MAX_HIGH_CURVE_THETA, std::f64::consts::PI / 3.0);
        // 90 degrees / 60 degrees, to a tight tolerance.
        assert!((START_DIR_ANGLE.to_degrees() - 90.0).abs() < 1e-9);
        assert!((MAX_HIGH_CURVE_THETA.to_degrees() - 60.0).abs() < 1e-9);
    }

    /// The V2 map window is centered on its block: `windowsize == blocksize + 2 * windowoffset`
    /// (`8 + 2*8 == 24`). This relationship is load-bearing for block/window addressing.
    #[test]
    fn v2_window_geometry_invariant() {
        assert_eq!(
            MAP_WINDOWSIZE_V2,
            MAP_BLOCKSIZE_V2 + 2 * MAP_WINDOWOFFSET_V2
        );
    }
}
