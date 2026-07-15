// SPDX-FileCopyrightText: NIST NBIS — U.S. Government work, public domain (title 17 §105)
//
// SPDX-License-Identifier: LicenseRef-NBIS-PD

//! The `lfsparms_V2` parameter block: the thresholds and controls that drive `_V2` MINDTCT,
//! assembled from the `*_V2` and shared `#define`s in stock NBIS `lfs.h` / `globals.c`.
//!
//! [`LfsParms`] mirrors the stock `LFSPARMS` struct field-for-field, in declaration order, and
//! [`LFSPARMS_V2`] reproduces the `lfsparms_V2` initializer verbatim — including the fields that
//! `_V2` deliberately leaves `UNUSED_INT`/`UNUSED_DBL` (isotropic binarization and the whole
//! minutiae-linking group, which `_V2` replaces). Every value is an **interoperability fact**; the
//! comments cite the reference macro. See `docs/mindtct-algorithm.md`.

use crate::consts;

/// Runtime parameter block for the LFS pipeline — the Rust analogue of stock NBIS `LFSPARMS`.
///
/// Fields appear in exactly the stock struct's declaration order (`lfs.h`), grouped by the same
/// comment banners. `int` maps to [`i32`] and `double` to [`f64`]. Read-only configuration handed
/// to the detection stages; see [`LFSPARMS_V2`] for the shipping `_V2` values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LfsParms {
    /* Image Controls */
    /// `pad_value` — intensity used to fill the padded image border.
    pub(crate) pad_value: i32,
    /// `join_line_radius` — radial width added to a minutia join line.
    pub(crate) join_line_radius: i32,

    /* Map Controls */
    /// `blocksize` — pixel dimension of each (non-overlapping) image block.
    pub(crate) blocksize: i32,
    /// `windowsize` — pixel dimension of the window surrounding each block.
    pub(crate) windowsize: i32,
    /// `windowoffset` — X&Y offset from a block's origin to its window's origin.
    pub(crate) windowoffset: i32,
    /// `num_directions` — number of integer directions across the semicircle.
    pub(crate) num_directions: i32,
    /// `start_dir_angle` — theta (radians) at which the integer directions begin.
    pub(crate) start_dir_angle: f64,
    /// `rmv_valid_nbr_min` — min valid neighbors for a block value to survive removal.
    pub(crate) rmv_valid_nbr_min: i32,
    /// `dir_strength_min` — min strength for a direction to count as significant.
    pub(crate) dir_strength_min: f64,
    /// `dir_distance_max` — max distance between a block direction and its neighbors' average.
    pub(crate) dir_distance_max: i32,
    /// `smth_valid_nbr_min` — min valid neighbors to smooth an invalid direction.
    pub(crate) smth_valid_nbr_min: i32,
    /// `vort_valid_nbr_min` — min valid neighbors to measure an invalid block's vorticity.
    pub(crate) vort_valid_nbr_min: i32,
    /// `highcurv_vorticity_min` — min vorticity marking an invalid block high-curvature.
    pub(crate) highcurv_vorticity_min: i32,
    /// `highcurv_curvature_min` — min curvature marking a valid block high-curvature.
    pub(crate) highcurv_curvature_min: i32,
    /// `min_interpolate_nbrs` — min valid neighbors to interpolate an invalid direction.
    pub(crate) min_interpolate_nbrs: i32,
    /// `percentile_min_max` — percentile cutoff for a block's min/max intensities.
    pub(crate) percentile_min_max: i32,
    /// `min_contrast_delta` — min min/max delta (6-bit) for a block to be non-low-contrast.
    pub(crate) min_contrast_delta: i32,

    /* DFT Controls */
    /// `num_dft_waves` — number of DFT waveforms applied.
    pub(crate) num_dft_waves: i32,
    /// `powmax_min` — min DFT power allowable in any one direction.
    pub(crate) powmax_min: f64,
    /// `pownorm_min` — min normalized power allowable in any one direction.
    pub(crate) pownorm_min: f64,
    /// `powmax_max` — max power allowable at the lowest-frequency DFT wave.
    pub(crate) powmax_max: f64,
    /// `fork_interval` — check for a fork at +/- this many units from the current direction.
    pub(crate) fork_interval: i32,
    /// `fork_pct_powmax` — fork power floor as a fraction of the block's max directional power.
    pub(crate) fork_pct_powmax: f64,
    /// `fork_pct_pownorm` — fork normalized-power floor as a fraction of `pownorm_min`.
    pub(crate) fork_pct_pownorm: f64,

    /* Binarization Controls */
    /// `dirbin_grid_w` — directional-binarization grid width.
    pub(crate) dirbin_grid_w: i32,
    /// `dirbin_grid_h` — directional-binarization grid height.
    pub(crate) dirbin_grid_h: i32,
    /// `isobin_grid_dim` — isotropic-binarization grid dimension (unused in `_V2`).
    pub(crate) isobin_grid_dim: i32,
    /// `num_fill_holes` — passes filling length-1 holes in the binary image.
    pub(crate) num_fill_holes: i32,

    /* Minutiae Detection Controls */
    /// `max_minutia_delta` — max X/Y pixel translation for two minutiae to be "similar".
    pub(crate) max_minutia_delta: i32,
    /// `max_high_curve_theta` — contour-angle ceiling (radians) for a contour to hold minutiae.
    pub(crate) max_high_curve_theta: f64,
    /// `high_curve_half_contour` — half the pixel length extracted for a high-curvature contour.
    pub(crate) high_curve_half_contour: i32,
    /// `min_loop_len` — loop must exceed this pixel length to be considered for minutiae.
    pub(crate) min_loop_len: i32,
    /// `min_loop_aspect_dist` — min half-way distance across a loop's contour to test it.
    pub(crate) min_loop_aspect_dist: f64,
    /// `min_loop_aspect_ratio` — min max/min half-way distance ratio to test a loop.
    pub(crate) min_loop_aspect_ratio: f64,

    /* Minutiae Link Controls */
    /// `link_table_dim` — 2D link-table dimension (unused in `_V2`).
    pub(crate) link_table_dim: i32,
    /// `max_link_dist` — orthogonal link distance (unused in `_V2`).
    pub(crate) max_link_dist: i32,
    /// `min_theta_dist` — min distance for a reliable inter-point angle (unused in `_V2`).
    pub(crate) min_theta_dist: i32,
    /// `maxtrans` — max transitions along a trajectory for it to be "free" (also removes overlaps).
    pub(crate) maxtrans: i32,
    /// `score_theta_norm` — link-score theta normalizer (unused in `_V2`).
    pub(crate) score_theta_norm: f64,
    /// `score_dist_norm` — link-score distance normalizer (unused in `_V2`).
    pub(crate) score_dist_norm: f64,
    /// `score_dist_weight` — link-score distance weight (unused in `_V2`).
    pub(crate) score_dist_weight: f64,
    /// `score_numerator` — link-score numerator (unused in `_V2`).
    pub(crate) score_numerator: f64,

    /* False Minutiae Removal Controls */
    /// `max_rmtest_dist` — orthogonal distance for two minutiae to be considered for removal.
    pub(crate) max_rmtest_dist: i32,
    /// `max_hook_len` — pixel-contour length traced when analyzing for hooks.
    pub(crate) max_hook_len: i32,
    /// `max_half_loop` — half the max pixel-contour length traced for islands/lakes.
    pub(crate) max_half_loop: i32,
    /// `trans_dir_pix` — pixels opposite the minutia used to reach an invalid block.
    pub(crate) trans_dir_pix: i32,
    /// `small_loop_len` — max circumference of a small island/lake loop to remove.
    pub(crate) small_loop_len: i32,
    /// `side_half_contour` — half the pixels traced to form a complete side contour.
    pub(crate) side_half_contour: i32,
    /// `inv_block_margin` — max orthogonal distance to an invalid block for removal.
    pub(crate) inv_block_margin: i32,
    /// `rm_valid_nbr_min` — invalid-block valid-neighbor floor below which a minutia is removed.
    pub(crate) rm_valid_nbr_min: i32,
    /// `max_overlap_dist` — max pixel distance between two points tested for overlap.
    pub(crate) max_overlap_dist: i32,
    /// `max_overlap_join_dist` — max distance across an overlap that will be joined.
    pub(crate) max_overlap_join_dist: i32,
    /// `malformation_steps_1` — contour steps to the first malformation measuring point.
    pub(crate) malformation_steps_1: i32,
    /// `malformation_steps_2` — contour steps to the second malformation measuring point.
    pub(crate) malformation_steps_2: i32,
    /// `min_malformation_ratio` — min across-feature distance ratio to be considered normal.
    pub(crate) min_malformation_ratio: f64,
    /// `max_malformation_dist` — max across-feature distance to be considered normal.
    pub(crate) max_malformation_dist: i32,
    /// `pores_trans_r` — translation off a valley edge into the neighboring ridge.
    pub(crate) pores_trans_r: i32,
    /// `pores_perp_steps` — steps searched for the current ridge's edge.
    pub(crate) pores_perp_steps: i32,
    /// `pores_steps_fwd` — pixels traced to find forward contour points.
    pub(crate) pores_steps_fwd: i32,
    /// `pores_steps_bwd` — pixels traced to find backward contour points.
    pub(crate) pores_steps_bwd: i32,
    /// `pores_min_dist2` — min squared distance before it is treated as zero.
    pub(crate) pores_min_dist2: f64,
    /// `pores_max_ratio` — max forward/backward distance ratio to be considered a pore.
    pub(crate) pores_max_ratio: f64,

    /* Ridge Counting Controls */
    /// `max_nbrs` — max nearest neighbors per minutia.
    pub(crate) max_nbrs: i32,
    /// `max_ridge_steps` — max contour steps to validate a ridge crossing.
    pub(crate) max_ridge_steps: i32,
}

/// The stock `lfsparms_V2` initializer — the shipping VERSION 2 parameter set.
///
/// Values are the `#define`s from `lfs.h` in the exact order `globals.c` supplies them: the map
/// group uses the `MAP_*_V2` block geometry, false-removal uses the `*_V2` distances, and the
/// fields `_V2` retired (`isobin_grid_dim`, the linking group, the `score_*` weights) hold their
/// `UNUSED_INT`/`UNUSED_DBL` sentinels (`0` / `0.0`).
//
// Each field is sourced from `crate::consts` (the verbatim `lfs.h` `#define`s), in the exact order
// `globals.c` lists them. The stock `UNUSED_INT`/`UNUSED_DBL` sentinels are inlined as `0` / `0.0`
// because `_V2` retires those fields and `consts` deliberately omits the retired macros.
pub(crate) const LFSPARMS_V2: LfsParms = LfsParms {
    /* Image Controls */
    pad_value: consts::PAD_VALUE,
    join_line_radius: consts::JOIN_LINE_RADIUS,

    /* Map Controls */
    blocksize: consts::MAP_BLOCKSIZE_V2,
    windowsize: consts::MAP_WINDOWSIZE_V2,
    windowoffset: consts::MAP_WINDOWOFFSET_V2,
    num_directions: consts::NUM_DIRECTIONS,
    start_dir_angle: consts::START_DIR_ANGLE,
    rmv_valid_nbr_min: consts::RMV_VALID_NBR_MIN,
    dir_strength_min: consts::DIR_STRENGTH_MIN,
    dir_distance_max: consts::DIR_DISTANCE_MAX,
    smth_valid_nbr_min: consts::SMTH_VALID_NBR_MIN,
    vort_valid_nbr_min: consts::VORT_VALID_NBR_MIN,
    highcurv_vorticity_min: consts::HIGHCURV_VORTICITY_MIN,
    highcurv_curvature_min: consts::HIGHCURV_CURVATURE_MIN,
    min_interpolate_nbrs: consts::MIN_INTERPOLATE_NBRS,
    percentile_min_max: consts::PERCENTILE_MIN_MAX,
    min_contrast_delta: consts::MIN_CONTRAST_DELTA,

    /* DFT Controls */
    num_dft_waves: consts::NUM_DFT_WAVES,
    powmax_min: consts::POWMAX_MIN,
    pownorm_min: consts::POWNORM_MIN,
    powmax_max: consts::POWMAX_MAX,
    fork_interval: consts::FORK_INTERVAL,
    fork_pct_powmax: consts::FORK_PCT_POWMAX,
    fork_pct_pownorm: consts::FORK_PCT_POWNORM,

    /* Binarization Controls */
    dirbin_grid_w: consts::DIRBIN_GRID_W,
    dirbin_grid_h: consts::DIRBIN_GRID_H,
    isobin_grid_dim: 0, // UNUSED_INT (ISOBIN_GRID_DIM retired in _V2)
    num_fill_holes: consts::NUM_FILL_HOLES,

    /* Minutiae Detection Controls */
    max_minutia_delta: consts::MAX_MINUTIA_DELTA,
    max_high_curve_theta: consts::MAX_HIGH_CURVE_THETA,
    high_curve_half_contour: consts::HIGH_CURVE_HALF_CONTOUR,
    min_loop_len: consts::MIN_LOOP_LEN,
    min_loop_aspect_dist: consts::MIN_LOOP_ASPECT_DIST,
    min_loop_aspect_ratio: consts::MIN_LOOP_ASPECT_RATIO,

    /* Minutiae Link Controls */
    link_table_dim: 0,          // UNUSED_INT (LINK_TABLE_DIM)
    max_link_dist: 0,           // UNUSED_INT (MAX_LINK_DIST)
    min_theta_dist: 0,          // UNUSED_INT (MIN_THETA_DIST)
    maxtrans: consts::MAXTRANS, // MAXTRANS (also used for removing overlaps)
    score_theta_norm: 0.0,      // UNUSED_DBL (SCORE_THETA_NORM)
    score_dist_norm: 0.0,       // UNUSED_DBL (SCORE_DIST_NORM)
    score_dist_weight: 0.0,     // UNUSED_DBL (SCORE_DIST_WEIGHT)
    score_numerator: 0.0,       // UNUSED_DBL (SCORE_NUMERATOR)

    /* False Minutiae Removal Controls */
    max_rmtest_dist: consts::MAX_RMTEST_DIST_V2,
    max_hook_len: consts::MAX_HOOK_LEN_V2,
    max_half_loop: consts::MAX_HALF_LOOP_V2,
    trans_dir_pix: consts::TRANS_DIR_PIX_V2,
    small_loop_len: consts::SMALL_LOOP_LEN,
    side_half_contour: consts::SIDE_HALF_CONTOUR,
    inv_block_margin: consts::INV_BLOCK_MARGIN_V2,
    rm_valid_nbr_min: consts::RM_VALID_NBR_MIN,
    max_overlap_dist: consts::MAX_OVERLAP_DIST,
    max_overlap_join_dist: consts::MAX_OVERLAP_JOIN_DIST,
    malformation_steps_1: consts::MALFORMATION_STEPS_1,
    malformation_steps_2: consts::MALFORMATION_STEPS_2,
    min_malformation_ratio: consts::MIN_MALFORMATION_RATIO,
    max_malformation_dist: consts::MAX_MALFORMATION_DIST,
    pores_trans_r: consts::PORES_TRANS_R,
    pores_perp_steps: consts::PORES_PERP_STEPS,
    pores_steps_fwd: consts::PORES_STEPS_FWD,
    pores_steps_bwd: consts::PORES_STEPS_BWD,
    pores_min_dist2: consts::PORES_MIN_DIST2,
    pores_max_ratio: consts::PORES_MAX_RATIO,

    /* Ridge Counting Controls */
    max_nbrs: consts::MAX_NBRS,
    max_ridge_steps: consts::MAX_RIDGE_STEPS,
};

#[cfg(test)]
mod tests {
    use super::LFSPARMS_V2;
    use core::f64::consts::PI;

    // Cross-checks against the stock `lfsparms_V2` initializer in
    // `reference/nbis-stock/mindtct/src/lib/mindtct/globals.c` (values from `lfs.h`).

    #[test]
    fn image_and_map_controls_match_stock() {
        let p = LFSPARMS_V2;
        assert_eq!(p.pad_value, 128);
        assert_eq!(p.join_line_radius, 1);

        assert_eq!(p.blocksize, 8);
        assert_eq!(p.windowsize, 24);
        assert_eq!(p.windowoffset, 8);
        assert_eq!(p.num_directions, 16);
        assert_eq!(p.rmv_valid_nbr_min, 3);
        assert_eq!(p.dir_distance_max, 3);
        assert_eq!(p.smth_valid_nbr_min, 7);
        assert_eq!(p.vort_valid_nbr_min, 7);
        assert_eq!(p.highcurv_vorticity_min, 5);
        assert_eq!(p.highcurv_curvature_min, 5);
        assert_eq!(p.min_interpolate_nbrs, 2);
        assert_eq!(p.percentile_min_max, 10);
        assert_eq!(p.min_contrast_delta, 5);
    }

    #[test]
    fn dft_and_binarization_controls_match_stock() {
        let p = LFSPARMS_V2;
        assert_eq!(p.num_dft_waves, 4);
        assert_eq!(p.powmax_min, 100_000.0);
        assert_eq!(p.pownorm_min, 3.8);
        assert_eq!(p.powmax_max, 50_000_000.0);
        assert_eq!(p.fork_interval, 2);
        assert_eq!(p.fork_pct_powmax, 0.7);
        assert_eq!(p.fork_pct_pownorm, 0.75);

        assert_eq!(p.dirbin_grid_w, 7);
        assert_eq!(p.dirbin_grid_h, 9);
        assert_eq!(p.num_fill_holes, 3);
    }

    #[test]
    fn detection_and_removal_controls_match_stock() {
        let p = LFSPARMS_V2;
        assert_eq!(p.max_minutia_delta, 10);
        assert_eq!(p.high_curve_half_contour, 14);
        assert_eq!(p.min_loop_len, 20);
        assert_eq!(p.min_loop_aspect_dist, 1.0);
        assert_eq!(p.min_loop_aspect_ratio, 2.25);

        // The `*_V2` false-removal distances, not the V1 defaults.
        assert_eq!(p.max_rmtest_dist, 16);
        assert_eq!(p.max_hook_len, 30);
        assert_eq!(p.max_half_loop, 30);
        assert_eq!(p.trans_dir_pix, 4);
        assert_eq!(p.inv_block_margin, 4);
        assert_eq!(p.maxtrans, 2);

        assert_eq!(p.small_loop_len, 15);
        assert_eq!(p.side_half_contour, 7);
        assert_eq!(p.rm_valid_nbr_min, 7);
        assert_eq!(p.max_overlap_dist, 8);
        assert_eq!(p.max_overlap_join_dist, 6);
        assert_eq!(p.malformation_steps_1, 10);
        assert_eq!(p.malformation_steps_2, 20);
        assert_eq!(p.min_malformation_ratio, 2.0);
        assert_eq!(p.max_malformation_dist, 20);
        assert_eq!(p.pores_trans_r, 3);
        assert_eq!(p.pores_perp_steps, 12);
        assert_eq!(p.pores_steps_fwd, 10);
        assert_eq!(p.pores_steps_bwd, 8);
        assert_eq!(p.pores_min_dist2, 0.5);
        assert_eq!(p.pores_max_ratio, 2.25);

        assert_eq!(p.max_nbrs, 5);
        assert_eq!(p.max_ridge_steps, 10);
    }

    #[test]
    fn angles_are_bit_exact_fractions_of_pi() {
        let p = LFSPARMS_V2;
        // Reproduce the stock derivation exactly (M_PI is the same f64 as `core`'s PI).
        assert_eq!(p.start_dir_angle, PI / 2.0);
        assert_eq!(p.max_high_curve_theta, PI / 3.0);
        assert_eq!(p.dir_strength_min, 0.2);
    }

    #[test]
    fn v2_leaves_retired_fields_at_unused_sentinels() {
        let p = LFSPARMS_V2;
        // `_V2` retires isotropic binarization and the whole linking group.
        assert_eq!(p.isobin_grid_dim, 0); // UNUSED_INT (V1 had ISOBIN_GRID_DIM = 11)
        assert_eq!(p.link_table_dim, 0);
        assert_eq!(p.max_link_dist, 0);
        assert_eq!(p.min_theta_dist, 0);
        assert_eq!(p.score_theta_norm, 0.0);
        assert_eq!(p.score_dist_norm, 0.0);
        assert_eq!(p.score_dist_weight, 0.0);
        assert_eq!(p.score_numerator, 0.0);
    }
}
