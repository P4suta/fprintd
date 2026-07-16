// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Block-map generation — a faithful port of stock NBIS `mindtct/src/lib/mindtct/maps.c`
//! (`gen_image_maps` L126), plus the DFT analysis it drives (`dft.c`), the TRUE/FALSE map
//! morphology it calls (`morph.c`), and the direction helpers it shares with `util.c`
//! (`closest_dir_dist`) and `block.c` (`find_valid_block`, `set_margin_blocks`).
//!
//! [`gen_image_maps`] turns a padded, 6-bit fingerprint image into four `map_w × map_h` block maps:
//!
//! * **Direction Map** — the dominant integer ridge-flow direction of each block (`-1` = INVALID),
//!   chosen by a DFT power analysis over `num_directions` rotated windows and then cleaned up by a
//!   fixed sequence of neighbor-consistency passes.
//! * **Low Contrast Map** — blocks whose 10th/90th-percentile intensity spread is too small.
//! * **Low Ridge Flow Map** — blocks where the DFT analysis found no significant flow.
//! * **High Curvature Map** — blocks near cores/deltas, flagged by vorticity or curvature.
//!
//! ## Bit-exactness
//!
//! Every float lives in `f64` and every rounding goes through [`sround`] / [`trunc_dbl_precision`]
//! exactly where stock does — never elsewhere. Two libm-sensitive spots are kept **raw** as stock
//! keeps them (see `docs/mindtct-algorithm.md` §Bit-exactness):
//!
//! * [`dft_power`] multiplies integer row sums by the *un-truncated* DFT-wave samples
//!   ([`init_dftwaves`](crate::init::init_dftwaves) stores them raw) and accumulates in `f64`.
//! * [`average_8nbr_dir`] runs `atan2`/`fmod` on raw `f64` — the cos/sin it accumulates come from the
//!   *quantized* [`init_dir2rad`](crate::init::init_dir2rad) table, but the angle math itself is not
//!   quantized; only the final `avr` (direction) and the `dir_strength` are truncated, as in stock.
//!
//! The scan orders — block raster order, the 8-neighbor visiting order, and the concentric-square
//! walk of [`remove_incon_dirs`] — are reproduced verbatim, because the stable bubble sorts and the
//! removal side effects make the result order-dependent.

use crate::block::{block_offsets, low_contrast_block};
use crate::consts::{DIR_STRENGTH_MIN, TRUNC_SCALE};
use crate::init::{DftWave, DftWaves, Dir2Rad, RotGrids};
use crate::num::{bubble_sort_double_dec_2, sround, trunc_dbl_precision};
use crate::params::LfsParms;

/// `INVALID_DIR` (`lfs.h` L320) — the sentinel for a block with no assigned direction.
const INVALID_DIR: i32 = -1;

/// `TRUE` / `FALSE` (`lfs.h`) as the `int` map flags stock stores.
const TRUE: i32 = 1;
const FALSE: i32 = 0;

/// `MIN_POWER_SUM` (`lfs.h` L419) — the floor placed on a direction's total DFT power before it is
/// used as the denominator of the normalized power, so [`get_max_norm`] never divides by zero.
/// Not in `consts` (it is neither an `lfsparms` field nor read outside the DFT stage), so it is a
/// local `const` at the stock value.
const MIN_POWER_SUM: f64 = 10.0;

/// The four block maps produced by [`gen_image_maps`], with their shared block-grid dimensions.
///
/// Mirrors the stock C out-parameters (`odmap`/`olcmap`/`olfmap`/`ohcmap`/`omw`/`omh`). Every map is
/// `map_w * map_h` long in row-major block order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImageMaps {
    /// Per-block ridge-flow direction: `-1` (INVALID) or a direction index in `0..num_directions`.
    pub(crate) direction_map: Vec<i32>,
    /// Per-block low-contrast flag (`1`/`0`).
    pub(crate) low_contrast_map: Vec<i32>,
    /// Per-block low-ridge-flow flag (`1`/`0`).
    pub(crate) low_flow_map: Vec<i32>,
    /// Per-block high-curvature flag (`1`/`0`).
    pub(crate) high_curve_map: Vec<i32>,
    /// Block-grid width (blocks).
    pub(crate) map_w: i32,
    /// Block-grid height (blocks).
    pub(crate) map_h: i32,
}

// =====================================================================================================
// DFT analysis (dft.c)
// =====================================================================================================

/// `sum_rot_block_rows` (dft.c L159) — accumulate a vector of pixel row sums by sampling the block at
/// `blkoffset` through one direction's rotated grid offsets.
///
/// For each of `blocksize` rows the sum runs across `blocksize` columns, reading
/// `pdata[blkoffset + grid_offsets[gi]]` (the offset is signed and already scaled by the padded
/// width). Row sums are integer sums of 6-bit pixels, matching stock's `int` accumulation.
fn sum_rot_block_rows(
    rowsums: &mut [i32],
    pdata: &[u8],
    blkoffset: i32,
    grid_offsets: &[i32],
    blocksize: i32,
) {
    let mut gi = 0usize;
    for iy in 0..blocksize {
        // Sums accumulate along the rotated rows of the grid, so start each row at 0.
        rowsums[iy as usize] = 0;
        for _ix in 0..blocksize {
            let idx = (blkoffset + grid_offsets[gi]) as usize;
            rowsums[iy as usize] += i32::from(pdata[idx]);
            gi += 1;
        }
    }
}

/// `dft_power` (dft.c L198) — the DFT power of one wave form applied to a row-sum vector.
///
/// Accumulates `cospart`/`sinpart` as `f64` sums of `rowsums[i] * wave.cos[i]` (resp. `sin`) — the
/// wave samples are the *raw, un-truncated* `f64` from [`init_dftwaves`](crate::init::init_dftwaves),
/// kept verbatim. Power is `cospart² + sinpart²`.
fn dft_power(rowsums: &[i32], wave: &DftWave, wavelen: i32) -> f64 {
    let mut cospart = 0.0_f64;
    let mut sinpart = 0.0_f64;
    for ((&r, &c), &s) in rowsums
        .iter()
        .zip(wave.cos.iter())
        .zip(wave.sin.iter())
        .take(wavelen as usize)
    {
        // Multiply each rotated row sum by its corresponding cos/sin point in the DFT wave.
        cospart += f64::from(r) * c;
        sinpart += f64::from(r) * s;
    }
    (cospart * cospart) + (sinpart * sinpart)
}

/// `dft_dir_powers` (dft.c L102) — the full `nwaves × ngrids` power matrix for a block.
///
/// For each direction, [`sum_rot_block_rows`] samples the block through that grid, then every DFT
/// wave is applied to the row sums. Returns `powers[w][dir]`. (Stock asserts the grid is square; the
/// V2 geometry guarantees it, so the padded image always covers every sample and no bounds check is
/// needed — the offsets are precomputed against the padded width.)
fn dft_dir_powers(
    pdata: &[u8],
    blkoffset: i32,
    dftwaves: &DftWaves,
    dftgrids: &RotGrids,
) -> Vec<Vec<f64>> {
    let nwaves = dftwaves.nwaves as usize;
    let ngrids = dftgrids.ngrids as usize;
    let mut powers = vec![vec![0.0_f64; ngrids]; nwaves];
    let mut rowsums = vec![0i32; dftgrids.grid_w as usize];

    for (dir, grid) in dftgrids.grids.iter().enumerate().take(ngrids) {
        sum_rot_block_rows(&mut rowsums, pdata, blkoffset, grid, dftgrids.grid_w);
        for (w, wave) in dftwaves.waves.iter().enumerate().take(nwaves) {
            powers[w][dir] = dft_power(&rowsums, wave, dftwaves.wavelen);
        }
    }
    powers
}

/// `get_max_norm` (dft.c L297) — max power, its direction, and the normalized power for one wave.
///
/// The normalized power is `powmax / (max(powsum, MIN_POWER_SUM) / ndirs)`. The max scan keeps the
/// *first* direction on ties (strict `>`), exactly as stock.
fn get_max_norm(power_vector: &[f64], ndirs: i32) -> (f64, i32, f64) {
    let mut max_v = power_vector[0];
    let mut max_i = 0i32;
    let mut powsum = power_vector[0];
    for dir in 1..ndirs {
        powsum += power_vector[dir as usize];
        if power_vector[dir as usize] > max_v {
            max_v = power_vector[dir as usize];
            max_i = dir;
        }
    }
    // Non-zero minimum on the denominator avoids possible division by zero.
    let powmean = powsum.max(MIN_POWER_SUM) / f64::from(ndirs);
    (max_v, max_i, max_v / powmean)
}

/// `sort_dft_waves` (dft.c L350) — rank wave indices by normalized squared max power, descending.
///
/// `pownorms2[i] = powmaxs[i] * pownorms[i]`; the ranking uses the *stable* decreasing bubble sort so
/// equal keys keep their input order (equal keys are never swapped). Returns the ranked index list.
fn sort_dft_waves(powmaxs: &[f64], pownorms: &[f64]) -> Vec<i32> {
    let nstats = powmaxs.len();
    let mut wis: Vec<i32> = (0..nstats as i32).collect();
    let mut pownorms2: Vec<f64> = (0..nstats).map(|i| powmaxs[i] * pownorms[i]).collect();
    bubble_sort_double_dec_2(&mut pownorms2, &mut wis);
    wis
}

/// `dft_power_stats` (dft.c L257) — derive `(wis, powmaxs, powmax_dirs, pownorms)` for waves `fw..tw`.
///
/// Statistics are computed for every wave except the lowest frequency (the caller passes `fw = 1`),
/// so the arrays are `tw - fw` long and index `i` corresponds to original wave `fw + i`.
fn dft_power_stats(
    powers: &[Vec<f64>],
    fw: i32,
    tw: i32,
    ndirs: i32,
) -> (Vec<i32>, Vec<f64>, Vec<i32>, Vec<f64>) {
    let nstats = (tw - fw) as usize;
    let mut powmaxs = vec![0.0_f64; nstats];
    let mut powmax_dirs = vec![0i32; nstats];
    let mut pownorms = vec![0.0_f64; nstats];

    for (i, w) in (fw..tw).enumerate() {
        let (pm, pd, pnorm) = get_max_norm(&powers[w as usize], ndirs);
        powmaxs[i] = pm;
        powmax_dirs[i] = pd;
        pownorms[i] = pnorm;
    }

    let wis = sort_dft_waves(&powmaxs, &pownorms);
    (wis, powmaxs, powmax_dirs, pownorms)
}

// =====================================================================================================
// Direction selection (maps.c)
// =====================================================================================================

/// `primary_dir_test` (maps.c L1188) — the primary criteria for choosing a block direction.
///
/// Walks the ranked wave statistics (via `wis`) and returns the first direction whose max power,
/// normalized power, and low-frequency power all pass their thresholds; else [`INVALID_DIR`].
fn primary_dir_test(
    powers: &[Vec<f64>],
    wis: &[i32],
    powmaxs: &[f64],
    powmax_dirs: &[i32],
    pownorms: &[f64],
    lfsparms: &LfsParms,
) -> i32 {
    for &wi in wis {
        let wi = wi as usize;
        // 1. Max power large enough, 2. normalized max power large enough, and 3. the lowest-frequency
        // wave's power at that direction is not too big.
        if powmaxs[wi] > lfsparms.powmax_min
            && pownorms[wi] > lfsparms.pownorm_min
            && powers[0][powmax_dirs[wi] as usize] <= lfsparms.powmax_max
        {
            return powmax_dirs[wi];
        }
    }
    INVALID_DIR
}

/// `secondary_fork_test` (maps.c L1272) — the fork criteria applied when the primary test fails.
///
/// Uses only the strongest wave (`wis[0]`). After a relaxed power/normalized-power/low-frequency
/// gate, it looks `fork_interval` directions to each side (modulo `num_directions`) and requires
/// **exactly one** of the two fork directions to exceed `fork_pct_powmax * powmax` — the XOR encoded
/// by the stock's paired `<=`/`>` conditions. Returns the strongest direction, else [`INVALID_DIR`].
fn secondary_fork_test(
    powers: &[Vec<f64>],
    wis: &[i32],
    powmaxs: &[f64],
    powmax_dirs: &[i32],
    pownorms: &[f64],
    lfsparms: &LfsParms,
) -> i32 {
    // Relax the normalized power threshold under fork conditions.
    let fork_pownorm_min = lfsparms.fork_pct_pownorm * lfsparms.pownorm_min;
    let w0 = wis[0] as usize;

    if powmaxs[w0] > lfsparms.powmax_min
        && pownorms[w0] >= fork_pownorm_min
        && powers[0][powmax_dirs[w0] as usize] <= lfsparms.powmax_max
    {
        let dir = powmax_dirs[w0];
        // Add / subtract FORK_INTERVALs modulo NDIRS (the `+ num_directions` keeps ldir non-negative).
        let rdir = (dir + lfsparms.fork_interval) % lfsparms.num_directions;
        let ldir =
            (dir + lfsparms.num_directions - lfsparms.fork_interval) % lfsparms.num_directions;

        // Forked-angle threshold is a fraction of the max directional power.
        let fork_pow_thresh = powmaxs[w0] * lfsparms.fork_pct_powmax;

        // wis indices are on [0..nstats); +1 maps back into the original power vectors.
        let lp = powers[w0 + 1][ldir as usize];
        let rp = powers[w0 + 1][rdir as usize];

        // Exactly one of the two fork angles may exceed the relative power threshold.
        if ((lp <= fork_pow_thresh) || (rp <= fork_pow_thresh))
            && ((lp > fork_pow_thresh) || (rp > fork_pow_thresh))
        {
            return dir;
        }
    }
    INVALID_DIR
}

// =====================================================================================================
// Initial map generation (maps.c gen_initial_maps L247)
// =====================================================================================================

/// The three maps [`gen_initial_maps`] returns: `(direction_map, low_contrast_map, low_flow_map)`.
type InitialMaps = (Vec<i32>, Vec<i32>, Vec<i32>);

/// `gen_initial_maps` (maps.c L247) — the initial Direction, Low Contrast, and Low Ridge Flow maps.
///
/// For every block (raster order): shift the block offset to its surrounding window origin, clamp
/// that origin so the low-contrast window never reads padded pixels, and test contrast. Low-contrast
/// blocks flag the Low Contrast Map (their direction stays INVALID). Otherwise the DFT analysis runs
/// and the primary then secondary tests pick a direction; if both fail the Low Ridge Flow Map is
/// flagged. The Direction Map is initialized to [`INVALID_DIR`].
fn gen_initial_maps(
    blkoffs: &[i32],
    mdims: (i32, i32),
    pdata: &[u8],
    dims: (i32, i32),
    dftwaves: &DftWaves,
    dftgrids: &RotGrids,
    lfsparms: &LfsParms,
) -> Result<InitialMaps, i32> {
    let (mw, mh) = mdims;
    let (pw, ph) = dims;
    let bsize = (mw * mh) as usize;

    let mut direction_map = vec![INVALID_DIR; bsize];
    let mut low_contrast_map = vec![FALSE; bsize];
    let mut low_flow_map = vec![FALSE; bsize];

    // Window origin limits that avoid analyzing the padded borders for low contrast.
    let xminlimit = dftgrids.pad;
    let yminlimit = dftgrids.pad;
    let xmaxlimit = pw - dftgrids.pad - lfsparms.windowsize - 1;
    let ymaxlimit = ph - dftgrids.pad - lfsparms.windowsize - 1;

    // A window origin must exist between the min and max limits. For an image with a dimension under
    // `windowsize + 1` the max limit falls below the min — there is nowhere to place the
    // `windowsize × windowsize` low-contrast window clear of the padding — and clamping to an inverted
    // range would drive the origin negative, indexing a wrapped offset. Surface it as the size error
    // the front-end already answers with an empty minutiae list, exactly as `block_offsets` rejects an
    // image smaller than a single block.
    if xmaxlimit < xminlimit || ymaxlimit < yminlimit {
        return Err(-82);
    }

    for bi in 0..bsize {
        // Adjust block offset from block origin to surrounding window origin.
        let dft_offset = blkoffs[bi] - (lfsparms.windowoffset * pw) - lfsparms.windowoffset;

        // Pixel coords of the window origin (dft_offset is always >= 0 for V2: pad > windowoffset).
        let mut win_x = dft_offset % pw;
        let mut win_y = dft_offset / pw;

        // Keep the low-contrast window off the padded borders.
        win_x = win_x.max(xminlimit).min(xmaxlimit);
        win_y = win_y.max(yminlimit).min(ymaxlimit);
        let low_contrast_offset = (win_y * pw) + win_x;

        // If block is low contrast ...
        if low_contrast_block(
            low_contrast_offset,
            lfsparms.windowsize,
            pdata,
            pw,
            ph,
            lfsparms,
        )? {
            low_contrast_map[bi] = TRUE;
            // Direction Map's block is already INVALID.
        } else {
            // Sufficient contrast for DFT processing.
            let powers = dft_dir_powers(pdata, low_contrast_offset, dftwaves, dftgrids);

            // Power statistics, skipping the first applied DFT wave (fw = 1).
            let (wis, powmaxs, powmax_dirs, pownorms) =
                dft_power_stats(&powers, 1, dftwaves.nwaves, dftgrids.ngrids);

            // Primary direction test.
            let mut blkdir =
                primary_dir_test(&powers, &wis, &powmaxs, &powmax_dirs, &pownorms, lfsparms);

            if blkdir != INVALID_DIR {
                direction_map[bi] = blkdir;
            } else {
                // Secondary (fork) direction test.
                blkdir =
                    secondary_fork_test(&powers, &wis, &powmaxs, &powmax_dirs, &pownorms, lfsparms);
                if blkdir != INVALID_DIR {
                    direction_map[bi] = blkdir;
                } else {
                    // Both tests failed: flag LOW RIDGE FLOW.
                    low_flow_map[bi] = TRUE;
                }
            }
        }
    }

    Ok((direction_map, low_contrast_map, low_flow_map))
}

// =====================================================================================================
// TRUE/FALSE map morphology (morph.c)
// =====================================================================================================

/// `get_south8_2` (morph.c L170) — the pixel one row below, or `failcode` off the bottom edge.
fn get_south8_2(img: &[u8], idx: usize, row: i32, iw: i32, ih: i32, failcode: u8) -> u8 {
    if row >= ih - 1 {
        failcode
    } else {
        img[idx + iw as usize]
    }
}

/// `get_north8_2` (morph.c L194) — the pixel one row above, or `failcode` off the top edge.
fn get_north8_2(img: &[u8], idx: usize, row: i32, iw: i32, failcode: u8) -> u8 {
    if row < 1 {
        failcode
    } else {
        img[idx - iw as usize]
    }
}

/// `get_east8_2` (morph.c L218) — the pixel one column right, or `failcode` off the right edge.
fn get_east8_2(img: &[u8], idx: usize, col: i32, iw: i32, failcode: u8) -> u8 {
    if col >= iw - 1 {
        failcode
    } else {
        img[idx + 1]
    }
}

/// `get_west8_2` (morph.c L241) — the pixel one column left, or `failcode` off the left edge.
fn get_west8_2(img: &[u8], idx: usize, col: i32, failcode: u8) -> u8 {
    if col < 1 {
        failcode
    } else {
        img[idx - 1]
    }
}

/// `dilate_charimage_2` (morph.c L128) — set each false pixel true if any 4-neighbor is true.
///
/// Off-image neighbors use `failcode = 0` (treated as false), so dilation does not spill past the
/// border. `out` starts as a copy of `inp` (already-true pixels are left alone).
fn dilate_charimage_2(inp: &[u8], out: &mut [u8], iw: i32, ih: i32) {
    out.copy_from_slice(inp);
    let mut idx = 0usize;
    for row in 0..ih {
        for col in 0..iw {
            if inp[idx] == 0
                && (get_west8_2(inp, idx, col, 0) != 0
                    || get_east8_2(inp, idx, col, iw, 0) != 0
                    || get_north8_2(inp, idx, row, iw, 0) != 0
                    || get_south8_2(inp, idx, row, iw, ih, 0) != 0)
            {
                out[idx] = 1;
            }
            idx += 1;
        }
    }
}

/// `erode_charimage_2` (morph.c L88) — set each true pixel false if any 4-neighbor is false.
///
/// Off-image neighbors use `failcode = 1` (treated as true), so border pixels are not eroded
/// indiscriminately. `out` starts as a copy of `inp`.
fn erode_charimage_2(inp: &[u8], out: &mut [u8], iw: i32, ih: i32) {
    out.copy_from_slice(inp);
    let mut idx = 0usize;
    for row in 0..ih {
        for col in 0..iw {
            if inp[idx] != 0
                && !(get_west8_2(inp, idx, col, 1) != 0
                    && get_east8_2(inp, idx, col, iw, 1) != 0
                    && get_north8_2(inp, idx, row, iw, 1) != 0
                    && get_south8_2(inp, idx, row, iw, ih, 1) != 0)
            {
                out[idx] = 0;
            }
            idx += 1;
        }
    }
}

/// `morph_TF_map` (maps.c L661) — dilate twice then erode twice to fill voids in a TRUE/FALSE map.
///
/// The map's `1`/`0` `int` flags are copied into a byte image, run through
/// `dilate → dilate → erode → erode` (bouncing between two buffers, exactly as stock), and copied
/// back. This "closing" fills small holes in the Low Ridge Flow map.
fn morph_tf_map(tfmap: &mut [i32], mw: i32, mh: i32) {
    let n = (mw * mh) as usize;
    let mut cimage: Vec<u8> = tfmap.iter().map(|&v| v as u8).collect();
    let mut mimage: Vec<u8> = vec![0u8; n];

    dilate_charimage_2(&cimage, &mut mimage, mw, mh); // c -> m
    dilate_charimage_2(&mimage, &mut cimage, mw, mh); // m -> c
    erode_charimage_2(&cimage, &mut mimage, mw, mh); //  c -> m
    erode_charimage_2(&mimage, &mut cimage, mw, mh); //  m -> c

    for (dst, &src) in tfmap.iter_mut().zip(cimage.iter()) {
        *dst = i32::from(src);
    }
}

// =====================================================================================================
// Neighbor direction helpers (maps.c / util.c)
// =====================================================================================================

/// `average_8nbr_dir` (maps.c L1838) — the average of a block's 8 valid neighbor directions.
///
/// Returns `(avrdir, dir_strength, nvalid)`. Cos/sin of each valid neighbor's direction are summed
/// (from the quantized [`Dir2Rad`] table) in the fixed order NW, N, NE, E, SE, S, SW, W, averaged,
/// and the strength `cos² + sin²` is [`trunc_dbl_precision`]-quantized. If there are no valid
/// neighbors, or the strength is below [`DIR_STRENGTH_MIN`], the direction is [`INVALID_DIR`] and the
/// strength `0`. Otherwise `atan2`/`fmod` (raw `f64`) map the angle to `[0, 2π)`, `avr = θ/pi_factor`
/// is truncated and [`sround`]ed, and taken modulo `ndirs`.
fn average_8nbr_dir(
    imap: &[i32],
    mx: i32,
    my: i32,
    mw: i32,
    mh: i32,
    dir2rad: &Dir2Rad,
) -> (i32, f64, i32) {
    let e = mx + 1;
    let w = mx - 1;
    let n = my - 1;
    let s = my + 1;

    let mut nvalid = 0i32;
    let mut cospart = 0.0_f64;
    let mut sinpart = 0.0_f64;

    // Accumulate a neighbor's direction if it is in bounds and valid.
    let accum = |bx: i32, by: i32, nvalid: &mut i32, cospart: &mut f64, sinpart: &mut f64| {
        let v = imap[(by * mw + bx) as usize];
        if v != INVALID_DIR {
            *cospart += dir2rad.cos[v as usize];
            *sinpart += dir2rad.sin[v as usize];
            *nvalid += 1;
        }
    };

    // 1. NW  2. N  3. NE  4. E  5. SE  6. S  7. SW  8. W  (stock visiting order)
    if (w >= 0) && (n >= 0) {
        accum(w, n, &mut nvalid, &mut cospart, &mut sinpart);
    }
    if n >= 0 {
        accum(mx, n, &mut nvalid, &mut cospart, &mut sinpart);
    }
    if (e < mw) && (n >= 0) {
        accum(e, n, &mut nvalid, &mut cospart, &mut sinpart);
    }
    if e < mw {
        accum(e, my, &mut nvalid, &mut cospart, &mut sinpart);
    }
    if (e < mw) && (s < mh) {
        accum(e, s, &mut nvalid, &mut cospart, &mut sinpart);
    }
    if s < mh {
        accum(mx, s, &mut nvalid, &mut cospart, &mut sinpart);
    }
    if (w >= 0) && (s < mh) {
        accum(w, s, &mut nvalid, &mut cospart, &mut sinpart);
    }
    if w >= 0 {
        accum(w, my, &mut nvalid, &mut cospart, &mut sinpart);
    }

    // No valid neighbors -> INVALID.
    if nvalid == 0 {
        return (INVALID_DIR, 0.0, 0);
    }

    // Average the accumulated cos/sin components.
    cospart /= f64::from(nvalid);
    sinpart /= f64::from(nvalid);

    // Directional strength as squared hypotenuse, quantized for cross-architecture consistency.
    let dir_strength = trunc_dbl_precision((cospart * cospart) + (sinpart * sinpart), TRUNC_SCALE);

    // Strength too low -> INVALID.
    if dir_strength < DIR_STRENGTH_MIN {
        return (INVALID_DIR, 0.0, nvalid);
    }

    // Angle from arctan of the average components (raw f64 — libm-sensitive, kept as stock).
    let mut theta = sinpart.atan2(cospart);
    let pi2 = 2.0 * std::f64::consts::PI;
    theta += pi2;
    theta %= pi2;

    // pi_factor sets the trig period to ndirs units; theta/pi_factor lands on [0..ndirs].
    let pi_factor = pi2 / f64::from(dir2rad.ndirs);
    let avr = trunc_dbl_precision(theta / pi_factor, TRUNC_SCALE);
    let mut avrdir = sround(avr);

    // Map values >= ndirs back onto [0..ndirs).
    avrdir %= dir2rad.ndirs;
    (avrdir, dir_strength, nvalid)
}

/// `num_valid_8nbrs` (maps.c L2044) — count the block's 8-neighbors that hold a valid direction.
fn num_valid_8nbrs(imap: &[i32], mx: i32, my: i32, mw: i32, mh: i32) -> i32 {
    let e = mx + 1;
    let w = mx - 1;
    let n = my - 1;
    let s = my + 1;
    let mut nvalid = 0i32;

    let val = |bx: i32, by: i32| imap[(by * mw + bx) as usize];

    if (w >= 0) && (n >= 0) && (val(w, n) >= 0) {
        nvalid += 1;
    }
    if (n >= 0) && (val(mx, n) >= 0) {
        nvalid += 1;
    }
    if (n >= 0) && (e < mw) && (val(e, n) >= 0) {
        nvalid += 1;
    }
    if (e < mw) && (val(e, my) >= 0) {
        nvalid += 1;
    }
    if (e < mw) && (s < mh) && (val(e, s) >= 0) {
        nvalid += 1;
    }
    if (s < mh) && (val(mx, s) >= 0) {
        nvalid += 1;
    }
    if (w >= 0) && (s < mh) && (val(w, s) >= 0) {
        nvalid += 1;
    }
    if (w >= 0) && (val(w, my) >= 0) {
        nvalid += 1;
    }
    nvalid
}

/// `closest_dir_dist` (util.c L602) — the shortest wrap-aware distance between two directions.
///
/// Returns `min(|dir2-dir1|, ndirs - |dir2-dir1|)`, or [`INVALID_DIR`] if either direction is
/// invalid.
fn closest_dir_dist(dir1: i32, dir2: i32, ndirs: i32) -> i32 {
    if (dir1 >= 0) && (dir2 >= 0) {
        let d1 = (dir2 - dir1).abs();
        let d2 = ndirs - d1;
        d1.min(d2)
    } else {
        INVALID_DIR
    }
}

// =====================================================================================================
// Inconsistent-direction removal (maps.c)
// =====================================================================================================

/// One concentric square walked by [`remove_incon_dirs`], by its four edge coordinates.
#[derive(Clone, Copy)]
struct ConcentricBox {
    l: i32,
    t: i32,
    r: i32,
    b: i32,
}

/// `remove_dir` (maps.c L1754) — whether a block's direction is too weak/inconsistent to keep.
///
/// Returns non-zero to remove: `1` when fewer than `rmv_valid_nbr_min` valid neighbors exist, `2`
/// when the average neighbor direction is strong enough (`>= dir_strength_min`) yet the wrap-aware
/// distance between it and the block's direction exceeds `dir_distance_max`.
fn remove_dir(
    imap: &[i32],
    mx: i32,
    my: i32,
    mw: i32,
    mh: i32,
    dir2rad: &Dir2Rad,
    lfsparms: &LfsParms,
) -> i32 {
    let (avrdir, dir_strength, nvalid) = average_8nbr_dir(imap, mx, my, mw, mh, dir2rad);

    // Valid-neighbor test.
    if nvalid < lfsparms.rmv_valid_nbr_min {
        return 1;
    }

    // Only trust the average direction if it is strong enough.
    if dir_strength >= lfsparms.dir_strength_min {
        // Minimum absolute distance, accounting for wrap from 0 to NDIRS.
        let cur = imap[(my * mw + mx) as usize];
        let mut dist = (avrdir - cur).abs();
        dist = dist.min(dir2rad.ndirs - dist);
        if dist > lfsparms.dir_distance_max {
            return 2;
        }
    }
    0
}

/// One point of an edge walk: if it holds a valid direction that [`remove_dir`] rejects, set it
/// INVALID and count it. Shared by the four edge routines to keep the removal side effect identical.
fn test_and_remove(
    imap: &mut [i32],
    bx: i32,
    by: i32,
    mw: i32,
    mh: i32,
    dir2rad: &Dir2Rad,
    lfsparms: &LfsParms,
) -> i32 {
    let idx = (by * mw + bx) as usize;
    if imap[idx] != INVALID_DIR && remove_dir(imap, bx, by, mw, mh, dir2rad, lfsparms) != 0 {
        imap[idx] = INVALID_DIR;
        1
    } else {
        0
    }
}

/// `test_top_edge` (maps.c L1514) — walk the box's top edge left→right, removing weak directions.
fn test_top_edge(
    bx: &ConcentricBox,
    imap: &mut [i32],
    mw: i32,
    mh: i32,
    dir2rad: &Dir2Rad,
    lfsparms: &LfsParms,
) -> i32 {
    let sx = bx.l.max(0);
    let ex = (bx.r - 1).min(mw - 1);
    let mut nremoved = 0;
    for cx in sx..=ex {
        nremoved += test_and_remove(imap, cx, bx.t, mw, mh, dir2rad, lfsparms);
    }
    nremoved
}

/// `test_right_edge` (maps.c L1575) — walk the box's right edge top→bottom, removing weak directions.
fn test_right_edge(
    bx: &ConcentricBox,
    imap: &mut [i32],
    mw: i32,
    mh: i32,
    dir2rad: &Dir2Rad,
    lfsparms: &LfsParms,
) -> i32 {
    let sy = bx.t.max(0);
    let ey = (bx.b - 1).min(mh - 1);
    let mut nremoved = 0;
    for cy in sy..=ey {
        nremoved += test_and_remove(imap, bx.r, cy, mw, mh, dir2rad, lfsparms);
    }
    nremoved
}

/// `test_bottom_edge` (maps.c L1635) — walk the box's bottom edge right→left, removing weak directions.
fn test_bottom_edge(
    bx: &ConcentricBox,
    imap: &mut [i32],
    mw: i32,
    mh: i32,
    dir2rad: &Dir2Rad,
    lfsparms: &LfsParms,
) -> i32 {
    let sx = bx.r.min(mw - 1);
    let ex = (bx.l - 1).max(0);
    let mut nremoved = 0;
    // Stock walks from sx down to ex inclusive (iptr--).
    let mut cx = sx;
    while cx >= ex {
        nremoved += test_and_remove(imap, cx, bx.b, mw, mh, dir2rad, lfsparms);
        cx -= 1;
    }
    nremoved
}

/// `test_left_edge` (maps.c L1696) — walk the box's left edge bottom→top, removing weak directions.
fn test_left_edge(
    bx: &ConcentricBox,
    imap: &mut [i32],
    mw: i32,
    mh: i32,
    dir2rad: &Dir2Rad,
    lfsparms: &LfsParms,
) -> i32 {
    let sy = bx.b.min(mh - 1);
    let ey = (bx.t - 1).max(0);
    let mut nremoved = 0;
    // Stock walks from sy up to ey inclusive (iptr -= mw).
    let mut cy = sy;
    while cy >= ey {
        nremoved += test_and_remove(imap, bx.l, cy, mw, mh, dir2rad, lfsparms);
        cy -= 1;
    }
    nremoved
}

/// `remove_incon_dirs` (maps.c L1409) — prune weak/inconsistent directions, center-outward.
///
/// Each pass starts at the map center, then grows concentric squares (top, right, bottom, left
/// edges, each guarded by its boundary) removing directions [`remove_dir`] rejects. The removals are
/// side effects visible to later tests in the same pass. Passes repeat until one completes with no
/// removals (the stock `do { ... } while (nremoved)`).
fn remove_incon_dirs(imap: &mut [i32], mw: i32, mh: i32, dir2rad: &Dir2Rad, lfsparms: &LfsParms) {
    let cx = mw >> 1;
    let cy = mh >> 1;

    loop {
        let mut nremoved = 0;

        // Start at center.
        nremoved += test_and_remove(imap, cx, cy, mw, mh, dir2rad, lfsparms);

        // Initialize the concentric box just outside the center.
        let mut bx = ConcentricBox {
            l: cx - 1,
            t: cy - 1,
            r: cx + 1,
            b: cy + 1,
        };

        // Grow boxes until ALL edges exceed the map bounds.
        while (bx.l >= 0) || (bx.r < mw) || (bx.t >= 0) || (bx.b < mh) {
            if bx.t >= 0 {
                nremoved += test_top_edge(&bx, imap, mw, mh, dir2rad, lfsparms);
            }
            if bx.r < mw {
                nremoved += test_right_edge(&bx, imap, mw, mh, dir2rad, lfsparms);
            }
            if bx.b < mh {
                nremoved += test_bottom_edge(&bx, imap, mw, mh, dir2rad, lfsparms);
            }
            if bx.l >= 0 {
                nremoved += test_left_edge(&bx, imap, mw, mh, dir2rad, lfsparms);
            }
            bx.l -= 1;
            bx.t -= 1;
            bx.r += 1;
            bx.b += 1;
        }

        if nremoved == 0 {
            break;
        }
    }
}

// =====================================================================================================
// Smoothing and interpolation (maps.c)
// =====================================================================================================

/// `smooth_direction_map` (maps.c L783) — replace directions with their neighbor average where strong.
///
/// For each non-low-contrast block whose neighbor average is strong enough
/// (`>= dir_strength_min`): a *valid* direction is overwritten when there are at least
/// `rmv_valid_nbr_min` valid neighbors; an *invalid* one is filled when there are at least
/// `smth_valid_nbr_min`. Runs in place in raster order (each write is visible to later blocks).
fn smooth_direction_map(
    direction_map: &mut [i32],
    low_contrast_map: &[i32],
    mw: i32,
    mh: i32,
    dir2rad: &Dir2Rad,
    lfsparms: &LfsParms,
) {
    for my in 0..mh {
        for mx in 0..mw {
            let idx = (my * mw + mx) as usize;
            // Skip LOW CONTRAST blocks (keep their INVALID direction).
            if low_contrast_map[idx] != 0 {
                continue;
            }

            let (avrdir, dir_strength, nvalid) =
                average_8nbr_dir(direction_map, mx, my, mw, mh, dir2rad);

            if dir_strength >= lfsparms.dir_strength_min {
                if direction_map[idx] != INVALID_DIR {
                    // Valid direction: overwrite if enough valid neighbors.
                    if nvalid >= lfsparms.rmv_valid_nbr_min {
                        direction_map[idx] = avrdir;
                    }
                } else {
                    // Invalid direction: fill if enough valid neighbors.
                    if nvalid >= lfsparms.smth_valid_nbr_min {
                        direction_map[idx] = avrdir;
                    }
                }
            }
        }
    }
}

/// `find_valid_block` (block.c L324) — search from `start` along `incr` for a valid direction.
///
/// Steps by `incr` (starting one step out), stopping unsuccessfully at a LOW CONTRAST block or the
/// map boundary, and successfully at the first block with a valid direction — returned as
/// `(dir, x, y)`.
fn find_valid_block(
    direction_map: &[i32],
    low_contrast_map: &[i32],
    start: (i32, i32),
    dims: (i32, i32),
    incr: (i32, i32),
) -> Option<(i32, i32, i32)> {
    let (mw, mh) = dims;
    let mut x = start.0 + incr.0;
    let mut y = start.1 + incr.1;

    while (x >= 0) && (x < mw) && (y >= 0) && (y < mh) {
        let idx = (y * mw + x) as usize;
        // Stop unsuccessfully at a LOW CONTRAST block.
        if low_contrast_map[idx] != 0 {
            return None;
        }
        // Stop successfully at a block with valid direction.
        let dir = direction_map[idx];
        if dir >= 0 {
            return Some((dir, x, y));
        }
        x += incr.0;
        y += incr.1;
    }
    None
}

/// `interpolate_direction_map` (maps.c L481) — fill INVALID directions from valid N/E/S/W neighbors.
///
/// For each non-low-contrast INVALID block, the nearest valid block in each cardinal direction is
/// found ([`find_valid_block`], stopping at low-contrast blocks). If at least `min_interpolate_nbrs`
/// are found, their directions are combined in a distance-weighted average (weight `total_dist - dist`),
/// [`trunc_dbl_precision`]-quantized and [`sround`]ed. Results are computed into a separate buffer
/// read from the original map, then copied back — so no in-pass write affects another block.
fn interpolate_direction_map(
    direction_map: &mut [i32],
    low_contrast_map: &[i32],
    mw: i32,
    mh: i32,
    lfsparms: &LfsParms,
) {
    let mut omap = direction_map.to_vec();

    for y in 0..mh {
        for x in 0..mw {
            let idx = (y * mw + x) as usize;

            // Only interpolate non-low-contrast blocks with INVALID direction.
            if (low_contrast_map[idx] == 0) && (direction_map[idx] == INVALID_DIR) {
                let mut total_found = 0i32;
                let mut total_dist = 0i32;

                // North (0,-1): dist = y - nbr_y.
                let north =
                    find_valid_block(direction_map, low_contrast_map, (x, y), (mw, mh), (0, -1))
                        .map(|(d, _nx, ny)| {
                            let dist = y - ny;
                            total_dist += dist;
                            total_found += 1;
                            (d, dist)
                        });
                // East (1,0): dist = nbr_x - x.
                let east =
                    find_valid_block(direction_map, low_contrast_map, (x, y), (mw, mh), (1, 0))
                        .map(|(d, nx, _ny)| {
                            let dist = nx - x;
                            total_dist += dist;
                            total_found += 1;
                            (d, dist)
                        });
                // South (0,1): dist = nbr_y - y.
                let south =
                    find_valid_block(direction_map, low_contrast_map, (x, y), (mw, mh), (0, 1))
                        .map(|(d, _nx, ny)| {
                            let dist = ny - y;
                            total_dist += dist;
                            total_found += 1;
                            (d, dist)
                        });
                // West (-1,0): dist = x - nbr_x.
                let west =
                    find_valid_block(direction_map, low_contrast_map, (x, y), (mw, mh), (-1, 0))
                        .map(|(d, nx, _ny)| {
                            let dist = x - nx;
                            total_dist += dist;
                            total_found += 1;
                            (d, dist)
                        });

                if total_found >= lfsparms.min_interpolate_nbrs {
                    // Weighted sum inversely related to distance: weight = total_dist - dist.
                    let mut total_delta = 0i32;
                    let weight =
                        |nbr: Option<(i32, i32)>, total_delta: &mut i32| -> Option<(i32, i32)> {
                            nbr.map(|(d, dist)| {
                                let delta = total_dist - dist;
                                *total_delta += delta;
                                (d, delta)
                            })
                        };
                    let n_w = weight(north, &mut total_delta);
                    let e_w = weight(east, &mut total_delta);
                    let s_w = weight(south, &mut total_delta);
                    let w_w = weight(west, &mut total_delta);

                    let mut avr_dir = 0.0_f64;
                    let mut accum = |nbr: Option<(i32, i32)>| {
                        if let Some((d, delta)) = nbr {
                            avr_dir += f64::from(d) * (f64::from(delta) / f64::from(total_delta));
                        }
                    };
                    accum(n_w);
                    accum(e_w);
                    accum(s_w);
                    accum(w_w);

                    // Truncate precision for cross-architecture consistency, then round.
                    avr_dir = trunc_dbl_precision(avr_dir, TRUNC_SCALE);
                    omap[idx] = sround(avr_dir);
                } else {
                    // Not enough neighbors: direction remains INVALID.
                    omap[idx] = direction_map[idx];
                }
            } else {
                // Otherwise, keep the current direction.
                omap[idx] = direction_map[idx];
            }
        }
    }

    // Copy interpolated directions back into the input map.
    direction_map.copy_from_slice(&omap);
}

// =====================================================================================================
// High-curvature map (maps.c gen_high_curve_map L881 + vorticity/curvature)
// =====================================================================================================

/// `accum_nbr_vorticity` (maps.c L2401) — accumulate the signed turn between two neighbor directions.
///
/// When both directions are valid and distinct, the clockwise distance `dir2 - dir1` (wrapped to
/// `[0, ndirs)`) increments the measure if `<= ndirs/2` and decrements it otherwise.
fn accum_nbr_vorticity(vmeasure: &mut i32, dir1: i32, dir2: i32, ndirs: i32) {
    if (dir1 != dir2) && (dir1 >= 0) && (dir2 >= 0) {
        let mut dist = dir2 - dir1;
        if dist < 0 {
            dist += ndirs;
        }
        if dist > (ndirs >> 1) {
            *vmeasure -= 1;
        } else {
            *vmeasure += 1;
        }
    }
}

/// Fetch the 8 neighbor directions of `(mx,my)` in stock order, each INVALID when out of bounds.
/// Returns `[nw, n, ne, e, se, s, sw, w]`, shared by [`vorticity`] and [`curvature`].
fn neighbor_dirs(imap: &[i32], mx: i32, my: i32, mw: i32, mh: i32) -> [i32; 8] {
    let e = mx + 1;
    let w = mx - 1;
    let n = my - 1;
    let s = my + 1;
    let at = |bx: i32, by: i32| imap[(by * mw + bx) as usize];

    let nw = if (w >= 0) && (n >= 0) {
        at(w, n)
    } else {
        INVALID_DIR
    };
    let n_v = if n >= 0 { at(mx, n) } else { INVALID_DIR };
    let ne = if (n >= 0) && (e < mw) {
        at(e, n)
    } else {
        INVALID_DIR
    };
    let e_v = if e < mw { at(e, my) } else { INVALID_DIR };
    let se = if (e < mw) && (s < mh) {
        at(e, s)
    } else {
        INVALID_DIR
    };
    let s_v = if s < mh { at(mx, s) } else { INVALID_DIR };
    let sw = if (w >= 0) && (s < mh) {
        at(w, s)
    } else {
        INVALID_DIR
    };
    let w_v = if w >= 0 { at(w, my) } else { INVALID_DIR };
    [nw, n_v, ne, e_v, se, s_v, sw, w_v]
}

/// `vorticity` (maps.c L2291) — cumulative curvature among a block's 8 neighbors, walked as a ring.
///
/// Accumulates [`accum_nbr_vorticity`] over the consecutive neighbor pairs
/// NW-N, N-NE, NE-E, E-SE, SE-S, S-SW, SW-W, W-NW.
fn vorticity(imap: &[i32], mx: i32, my: i32, mw: i32, mh: i32, ndirs: i32) -> i32 {
    let [nw, n, ne, e, se, s, sw, w] = neighbor_dirs(imap, mx, my, mw, mh);
    let mut vmeasure = 0i32;
    accum_nbr_vorticity(&mut vmeasure, nw, n, ndirs);
    accum_nbr_vorticity(&mut vmeasure, n, ne, ndirs);
    accum_nbr_vorticity(&mut vmeasure, ne, e, ndirs);
    accum_nbr_vorticity(&mut vmeasure, e, se, ndirs);
    accum_nbr_vorticity(&mut vmeasure, se, s, ndirs);
    accum_nbr_vorticity(&mut vmeasure, s, sw, ndirs);
    accum_nbr_vorticity(&mut vmeasure, sw, w, ndirs);
    accum_nbr_vorticity(&mut vmeasure, w, nw, ndirs);
    vmeasure
}

/// `curvature` (maps.c L2451) — the largest [`closest_dir_dist`] between the block and any valid
/// neighbor, or `-1` if no valid neighbor exists.
fn curvature(imap: &[i32], mx: i32, my: i32, mw: i32, mh: i32, ndirs: i32) -> i32 {
    let nbrs = neighbor_dirs(imap, mx, my, mw, mh);
    let cur = imap[(my * mw + mx) as usize];
    let mut cmeasure = -1i32;
    for &nbr in &nbrs {
        let dist = closest_dir_dist(cur, nbr, ndirs);
        if dist > cmeasure {
            cmeasure = dist;
        }
    }
    cmeasure
}

/// `gen_high_curve_map` (maps.c L881) — flag blocks near cores/deltas as high curvature.
///
/// A block with valid neighbors is flagged when: its direction is INVALID and it has at least
/// `vort_valid_nbr_min` valid neighbors with [`vorticity`] `>= highcurv_vorticity_min`; or its
/// direction is valid with [`curvature`] `>= highcurv_curvature_min`.
fn gen_high_curve_map(direction_map: &[i32], mw: i32, mh: i32, lfsparms: &LfsParms) -> Vec<i32> {
    let mapsize = (mw * mh) as usize;
    let mut high_curve_map = vec![FALSE; mapsize];

    for by in 0..mh {
        for bx in 0..mw {
            let idx = (by * mw + bx) as usize;
            let nvalid = num_valid_8nbrs(direction_map, bx, by, mw, mh);

            if nvalid > 0 {
                if direction_map[idx] == INVALID_DIR {
                    if nvalid >= lfsparms.vort_valid_nbr_min {
                        let vmeasure =
                            vorticity(direction_map, bx, by, mw, mh, lfsparms.num_directions);
                        if vmeasure >= lfsparms.highcurv_vorticity_min {
                            high_curve_map[idx] = TRUE;
                        }
                    }
                } else {
                    let cmeasure =
                        curvature(direction_map, bx, by, mw, mh, lfsparms.num_directions);
                    if cmeasure >= lfsparms.highcurv_curvature_min {
                        high_curve_map[idx] = TRUE;
                    }
                }
            }
        }
    }
    high_curve_map
}

/// `set_margin_blocks` (block.c L373) — set the map's entire perimeter to `margin_value`.
fn set_margin_blocks(map: &mut [i32], mw: i32, mh: i32, margin_value: i32) {
    // Top and bottom rows.
    for x in 0..mw {
        map[x as usize] = margin_value;
        map[((mh - 1) * mw + x) as usize] = margin_value;
    }
    // Left and right columns (excluding the corners already set).
    for y in 1..mh - 1 {
        map[(y * mw) as usize] = margin_value;
        map[(y * mw + mw - 1) as usize] = margin_value;
    }
}

// =====================================================================================================
// Top level (maps.c gen_image_maps L126)
// =====================================================================================================

/// `gen_image_maps` (maps.c L126) — the full block-map pipeline over a padded, 6-bit image.
///
/// Steps, in the exact stock order (steps 3–7 are what make the Direction Map usable):
/// 1. block offsets, 2. [`gen_initial_maps`], `morph_TF_map` on the Low Flow map,
/// 3. [`remove_incon_dirs`], 4. [`smooth_direction_map`], 5. [`interpolate_direction_map`],
/// 6. [`remove_incon_dirs`] again, 7. [`smooth_direction_map`] again,
/// 8. [`set_margin_blocks`] to INVALID, 9. [`gen_high_curve_map`].
///
/// `pdata`/`pw`/`ph` are the padded image and its dimensions; the unpadded size is recovered as
/// `pw - 2*pad`. The DFT grid must be square (the V2 geometry guarantees `windowsize × windowsize`).
///
/// # Errors
/// Propagates the negative stock error codes from [`block_offsets`]/[`low_contrast_block`], plus
/// `-540` if the DFT grid is not square.
pub(crate) fn gen_image_maps(
    pdata: &[u8],
    pw: i32,
    ph: i32,
    dir2rad: &Dir2Rad,
    dftwaves: &DftWaves,
    dftgrids: &RotGrids,
    lfsparms: &LfsParms,
) -> Result<ImageMaps, i32> {
    // block_offsets assumes a square block/grid.
    if dftgrids.grid_w != dftgrids.grid_h {
        return Err(-540);
    }

    // Unpadded image dimensions.
    let iw = pw - (dftgrids.pad << 1);
    let ih = ph - (dftgrids.pad << 1);

    // 1. Block offsets.
    let bo = block_offsets(iw, ih, dftgrids.pad, lfsparms.blocksize)?;
    let (mw, mh) = (bo.map_w, bo.map_h);

    // 2. Initial Direction / Low Contrast / Low Ridge Flow maps.
    let (mut direction_map, low_contrast_map, mut low_flow_map) = gen_initial_maps(
        &bo.offsets,
        (mw, mh),
        pdata,
        (pw, ph),
        dftwaves,
        dftgrids,
        lfsparms,
    )?;

    morph_tf_map(&mut low_flow_map, mw, mh);

    // 3. Remove inconsistent directions.
    remove_incon_dirs(&mut direction_map, mw, mh, dir2rad, lfsparms);

    // 4. Smooth.
    smooth_direction_map(
        &mut direction_map,
        &low_contrast_map,
        mw,
        mh,
        dir2rad,
        lfsparms,
    );

    // 5. Interpolate INVALID blocks from valid neighbors.
    interpolate_direction_map(&mut direction_map, &low_contrast_map, mw, mh, lfsparms);

    // 6. Remove inconsistent directions again.
    remove_incon_dirs(&mut direction_map, mw, mh, dir2rad, lfsparms);

    // 7. Smooth again.
    smooth_direction_map(
        &mut direction_map,
        &low_contrast_map,
        mw,
        mh,
        dir2rad,
        lfsparms,
    );

    // 8. Set margin blocks to INVALID.
    set_margin_blocks(&mut direction_map, mw, mh, INVALID_DIR);

    // 9. High Curvature Map from the interpolated Direction Map.
    let high_curve_map = gen_high_curve_map(&direction_map, mw, mh, lfsparms);

    Ok(ImageMaps {
        direction_map,
        low_contrast_map,
        low_flow_map,
        high_curve_map,
        map_w: mw,
        map_h: mh,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::{DFT_COEFS, MAP_WINDOWSIZE_V2, NUM_DFT_WAVES, NUM_DIRECTIONS};
    use crate::init::{get_max_padding_V2, init_dftwaves, init_dir2rad, init_rotgrids, Relative2};
    use crate::params::LFSPARMS_V2;

    // --- closest_dir_dist: wrap-aware direction distance --------------------------------------
    #[test]
    fn closest_dir_dist_wraps() {
        // ndirs = 16: 2 and 14 are 4 apart the short way (min(12, 4)).
        assert_eq!(closest_dir_dist(2, 14, 16), 4);
        assert_eq!(closest_dir_dist(0, 8, 16), 8);
        assert_eq!(closest_dir_dist(3, 3, 16), 0);
        // Either direction invalid -> INVALID.
        assert_eq!(closest_dir_dist(-1, 5, 16), -1);
        assert_eq!(closest_dir_dist(5, -1, 16), -1);
    }

    // --- accum_nbr_vorticity: signed turn accumulation ----------------------------------------
    #[test]
    fn accum_nbr_vorticity_signs() {
        let mut v = 0;
        // Clockwise 2 (<= 8) increments.
        accum_nbr_vorticity(&mut v, 2, 4, 16);
        assert_eq!(v, 1);
        // Clockwise distance 3-8 = -5 -> +16 = 11 (> 8) decrements.
        accum_nbr_vorticity(&mut v, 8, 3, 16);
        assert_eq!(v, 0);
        // Equal or invalid: ignored.
        accum_nbr_vorticity(&mut v, 5, 5, 16);
        accum_nbr_vorticity(&mut v, -1, 5, 16);
        assert_eq!(v, 0);
    }

    // --- num_valid_8nbrs: only in-bounds valid neighbors count --------------------------------
    #[test]
    fn num_valid_8nbrs_counts_in_bounds() {
        // 3x3 map, center at (1,1) surrounded by 8 valid dirs -> 8.
        let all_valid = vec![0i32; 9];
        assert_eq!(num_valid_8nbrs(&all_valid, 1, 1, 3, 3), 8);
        // Corner (0,0) has only 3 in-bounds neighbors.
        assert_eq!(num_valid_8nbrs(&all_valid, 0, 0, 3, 3), 3);
        // INVALID neighbors are not counted.
        let mut m = vec![INVALID_DIR; 9];
        m[4] = 0; // only center valid; corner (0,0) sees center as SE -> 1 valid.
        assert_eq!(num_valid_8nbrs(&m, 0, 0, 3, 3), 1);
    }

    // --- set_margin_blocks: perimeter set, interior untouched ---------------------------------
    #[test]
    fn set_margin_blocks_sets_perimeter() {
        let mut m = vec![5i32; 16]; // 4x4 filled with 5.
        set_margin_blocks(&mut m, 4, 4, -1);
        #[rustfmt::skip]
        let expected = vec![
            -1, -1, -1, -1,
            -1,  5,  5, -1,
            -1,  5,  5, -1,
            -1, -1, -1, -1,
        ];
        assert_eq!(m, expected);
    }

    // --- morph_tf_map: closing fills a single-block hole --------------------------------------
    #[test]
    fn morph_tf_map_closes_hole() {
        // 5x5 all TRUE with one FALSE hole in the center; dilate-dilate-erode-erode fills it.
        let mut map = vec![1i32; 25];
        map[12] = 0;
        morph_tf_map(&mut map, 5, 5);
        assert_eq!(map[12], 1, "center hole should be closed");
        // A fully-true map stays true.
        let mut full = vec![1i32; 25];
        morph_tf_map(&mut full, 5, 5);
        assert!(full.iter().all(|&v| v == 1));
    }

    // --- get_max_norm: max, its direction, normalized power -----------------------------------
    #[test]
    fn get_max_norm_picks_first_max() {
        // Power peaks at dir 2; ties keep the first index.
        let pv = vec![1.0, 2.0, 9.0, 9.0, 1.0, 1.0, 1.0, 1.0];
        let (pm, pd, pnorm) = get_max_norm(&pv, 8);
        assert_eq!(pm, 9.0);
        assert_eq!(pd, 2); // first of the two 9.0 peaks
        let powsum: f64 = pv.iter().sum();
        assert!((pnorm - 9.0 / (powsum.max(MIN_POWER_SUM) / 8.0)).abs() < 1e-12);
    }

    // --- Full pipeline on a flat image: every block is low contrast ---------------------------
    #[test]
    fn gen_image_maps_flat_image_is_all_low_contrast() {
        // Build the V2 tables and a flat, already-6-bit padded image.
        let p = &LFSPARMS_V2;
        let pad = get_max_padding_V2(
            p.windowsize,
            p.windowoffset,
            p.dirbin_grid_w,
            p.dirbin_grid_h,
        );
        assert_eq!(pad, 13);

        let dir2rad = init_dir2rad(NUM_DIRECTIONS);
        let dftwaves = init_dftwaves(&DFT_COEFS, NUM_DFT_WAVES, MAP_WINDOWSIZE_V2);

        // Unpadded 32x32 -> 4x4 blocks; padded to 58x58.
        let iw = 32i32;
        let ih = 32i32;
        let dftgrids = init_rotgrids(
            iw,
            Some(pad),
            p.start_dir_angle,
            NUM_DIRECTIONS,
            p.windowsize,
            p.windowsize,
            Relative2::Origin,
        );
        let pw = iw + 2 * pad;
        let ph = ih + 2 * pad;

        // Flat 6-bit image (constant value): zero contrast everywhere.
        let pdata = vec![10u8; (pw * ph) as usize];

        let maps = gen_image_maps(&pdata, pw, ph, &dir2rad, &dftwaves, &dftgrids, p).unwrap();

        assert_eq!((maps.map_w, maps.map_h), (4, 4));
        let n = (maps.map_w * maps.map_h) as usize;
        assert_eq!(maps.direction_map.len(), n);
        // Flat -> all low contrast, all directions INVALID, no low flow, no high curvature.
        assert!(maps.low_contrast_map.iter().all(|&v| v == TRUE));
        assert!(maps.direction_map.iter().all(|&v| v == INVALID_DIR));
        assert!(maps.low_flow_map.iter().all(|&v| v == FALSE));
        assert!(maps.high_curve_map.iter().all(|&v| v == FALSE));
    }

    // --- gen_image_maps rejects a non-square DFT grid -----------------------------------------
    #[test]
    fn gen_image_maps_non_square_grid_errs() {
        let p = &LFSPARMS_V2;
        let dir2rad = init_dir2rad(NUM_DIRECTIONS);
        let dftwaves = init_dftwaves(&DFT_COEFS, NUM_DFT_WAVES, MAP_WINDOWSIZE_V2);
        // Deliberately non-square grid (grid_w != grid_h).
        let dftgrids = init_rotgrids(
            32,
            Some(13),
            p.start_dir_angle,
            NUM_DIRECTIONS,
            24,
            20,
            Relative2::Origin,
        );
        let pw = 32 + 26;
        let ph = 32 + 26;
        let pdata = vec![10u8; (pw * ph) as usize];
        assert_eq!(
            gen_image_maps(&pdata, pw, ph, &dir2rad, &dftwaves, &dftgrids, p),
            Err(-540)
        );
    }
}
