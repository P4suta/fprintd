// SPDX-FileCopyrightText: NIST NBIS — U.S. Government work, public domain (title 17 §105)
//
// SPDX-License-Identifier: LicenseRef-NBIS-PD

//! One-time lookup-table construction: `init_dir2rad` (direction→radians), `init_dftwaves` (DFT
//! waveform tables), and `init_rotgrids` (rotated-grid pixel-offset tables) for the DFT and
//! directional-binarization passes, plus `get_max_padding_V2` — the single border-pad width that
//! covers every rotated grid so the input image is padded exactly once.
//!
//! Faithful port of stock NBIS `mindtct/src/lib/mindtct/init.c`. The bit-exactness contract here
//! turns on **one deliberate asymmetry** (see `docs/mindtct-algorithm.md` §Bit-exactness):
//!
//! * `init_dir2rad` **quantizes** every `cos`/`sin` through
//!   [`trunc_dbl_precision`](crate::num::trunc_dbl_precision) at `1/16384`, so the integer-direction
//!   table is reproducible across architectures.
//! * `init_dftwaves` stores the **raw, un-truncated** `f64::cos`/`f64::sin` — stock does *not*
//!   quantize the DFT waveforms. This is the one place a `libm` difference can leak in, and it is
//!   kept verbatim rather than "fixed".
//!
//! `init_rotgrids` precomputes, for each of `ndirs` rotations, a flat list of pixel offsets
//! `ixt + iyt * pw` into the *padded* image, where `pw = iw + 2*pad` — so every offset is tied to
//! the width the image will actually be padded to. Two conventions are supported: offsets relative
//! to the grid's [`Center`](Relative2::Center) (directional binarization) or its
//! [`Origin`](Relative2::Origin) (DFT windows).

use crate::consts::TRUNC_SCALE;
use crate::num::{sround, trunc_dbl_precision};

/// Whether an [`init_rotgrids`] grid's rotated offsets are measured from the grid's geometric
/// **center** or its **origin** (top-left) — the stock `RELATIVE2CENTER` (`0`) / `RELATIVE2ORIGIN`
/// (`1`) flags, made an enum so the illegal-flag error path in the C (`-31`) cannot arise.
///
/// Directional binarization uses [`Center`](Self::Center) (grid centers sit on valid pixels); the
/// DFT window analysis uses [`Origin`](Self::Origin) (window origins sit on valid pixels). The
/// choice changes both the required padding and whether the grid center offset is folded back in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Relative2 {
    /// `RELATIVE2CENTER`: offsets are relative to the grid center `((w-1)/2, (h-1)/2)`.
    Center,
    /// `RELATIVE2ORIGIN`: offsets are relative to the grid origin (top-left), so the center is
    /// added back into each offset.
    Origin,
}

/// Lookup table converting an integer IMAP direction to its angle's `cos`/`sin` — the stock
/// `DIR2RAD`.
///
/// `cos[i]` and `sin[i]` hold the (quantized) cosine and sine of `theta = i * 2*PI/ndirs`, the angle
/// of direction `i` around a semicircle. Both vectors are `ndirs` long.
#[derive(Clone, Debug)]
pub(crate) struct Dir2Rad {
    /// Number of integer directions across the semicircle.
    pub(crate) ndirs: i32,
    /// Quantized cosine per direction.
    pub(crate) cos: Vec<f64>,
    /// Quantized sine per direction.
    pub(crate) sin: Vec<f64>,
}

/// A single DFT waveform — matched `cos`/`sin` sample vectors for one frequency (the stock
/// `DFTWAVE`). Each vector is `blocksize` long.
#[derive(Clone, Debug)]
pub(crate) struct DftWave {
    /// Cosine samples (raw, un-truncated `f64`).
    pub(crate) cos: Vec<f64>,
    /// Sine samples (raw, un-truncated `f64`).
    pub(crate) sin: Vec<f64>,
}

/// The full set of DFT waveforms applied to each block window (the stock `DFTWAVES`).
#[derive(Clone, Debug)]
pub(crate) struct DftWaves {
    /// Number of waveforms.
    pub(crate) nwaves: i32,
    /// Sample length of every waveform (all identical; equals `blocksize`).
    pub(crate) wavelen: i32,
    /// One [`DftWave`] per frequency coefficient.
    pub(crate) waves: Vec<DftWave>,
}

/// Precomputed rotated-grid pixel offsets for `ngrids` orientations (the stock `ROTGRIDS`).
///
/// `grids[dir]` is a `grid_w * grid_h` flat list (row-major over the *unrotated* grid) of signed
/// offsets `ixt + iyt * pw` into the padded image, where `pw = iw + 2*pad`. Adding a grid's offset
/// to a base pixel index lands on the correspondingly rotated pixel.
#[derive(Clone, Debug)]
pub(crate) struct RotGrids {
    /// Border padding (in pixels) these offsets assume on every side of the image.
    pub(crate) pad: i32,
    /// Whether offsets are relative to the grid center or origin.
    // `dead_code`: `relative2`/`start_angle` mirror the stock `ROTGRIDS` layout but are consumed only
    // while *building* the grids (`init_rotgrids`); the offsets carry all the port needs afterward, so
    // nothing reads them back. Transcribed for fidelity and pinned by the tests. Targeted per-field
    // allows — the minimal suppression.
    #[allow(dead_code)]
    pub(crate) relative2: Relative2,
    /// Angle (radians) of the first grid (direction `0`).
    #[allow(dead_code)]
    pub(crate) start_angle: f64,
    /// Number of rotated grids (`= ndirs`).
    pub(crate) ngrids: i32,
    /// Grid width in pixels.
    pub(crate) grid_w: i32,
    /// Grid height in pixels.
    pub(crate) grid_h: i32,
    /// Per-direction flat offset lists.
    pub(crate) grids: Vec<Vec<i32>>,
}

/// `init_dir2rad` — build the direction→radians `cos`/`sin` table.
///
/// Stock `init_dir2rad` (init.c L86). `pi_factor = 2*PI/ndirs` sets the period to `ndirs` units;
/// for each direction `i`, `theta = i * pi_factor` and the cosine/sine are **quantized** to
/// `1/16384` via [`trunc_dbl_precision`] so the table is identical on every architecture.
pub(crate) fn init_dir2rad(ndirs: i32) -> Dir2Rad {
    // pi_factor sets the period of the trig functions to ndirs units in x.
    let pi_factor = 2.0 * std::f64::consts::PI / f64::from(ndirs);

    let mut cos = Vec::with_capacity(ndirs.max(0) as usize);
    let mut sin = Vec::with_capacity(ndirs.max(0) as usize);
    for i in 0..ndirs {
        let theta = f64::from(i) * pi_factor;
        // Truncate precision so answers are consistent across architectures.
        cos.push(trunc_dbl_precision(theta.cos(), TRUNC_SCALE));
        sin.push(trunc_dbl_precision(theta.sin(), TRUNC_SCALE));
    }

    Dir2Rad { ndirs, cos, sin }
}

/// `init_dftwaves` — build the set of DFT waveforms for block analysis.
///
/// Stock `init_dftwaves` (init.c L160). `pi_factor = 2*PI/blocksize`; each waveform `i` has
/// `freq = pi_factor * dft_coefs[i]` and samples `cos(freq*j)` / `sin(freq*j)` for `j` in
/// `0..blocksize`. **Unlike [`init_dir2rad`], these samples are stored raw — not quantized** —
/// exactly as stock does; this is the one deliberate `libm`-sensitive spot in the port.
pub(crate) fn init_dftwaves(dft_coefs: &[f64], nwaves: i32, blocksize: i32) -> DftWaves {
    // pi_factor sets the period of the trig functions to blocksize units in x.
    let pi_factor = 2.0 * std::f64::consts::PI / f64::from(blocksize);

    let mut waves = Vec::with_capacity(nwaves.max(0) as usize);
    for i in 0..nwaves {
        // Compute actual frequency for this coefficient.
        let freq = pi_factor * dft_coefs[i as usize];
        let mut cos = Vec::with_capacity(blocksize.max(0) as usize);
        let mut sin = Vec::with_capacity(blocksize.max(0) as usize);
        for j in 0..blocksize {
            let x = freq * f64::from(j);
            // Store cos and sin components of sample point (NO truncation — stock keeps these raw).
            cos.push(x.cos());
            sin.push(x.sin());
        }
        waves.push(DftWave { cos, sin });
    }

    DftWaves {
        nwaves,
        wavelen: blocksize,
        waves,
    }
}

/// `get_max_padding_V2` — the single border pad covering every rotated grid used by `_V2`.
///
/// Stock `get_max_padding_V2` (init.c L364). Takes the larger of two paddings so the image is padded
/// only once:
///
/// * **DFT windows** (`RELATIVE2ORIGIN`): rotational pad `round((diag - windowsize)/2)` over the
///   window's diagonal, **plus** `map_windowoffset` (the window overhangs the block by that much).
/// * **Directional-binarization grids** (`RELATIVE2CENTER`): `round((diag - 1)/2)` over the grid's
///   diagonal.
///
/// Each `pad` is quantized to `1/16384` before [`sround`] to stay architecture-stable. For the
/// shipping `_V2` geometry (`windowsize 24`, `offset 8`, dirbin grid `7x9`) this returns `13`.
#[allow(non_snake_case)]
pub(crate) fn get_max_padding_V2(
    map_windowsize: i32,
    map_windowoffset: i32,
    dirbin_grid_w: i32,
    dirbin_grid_h: i32,
) -> i32 {
    // 1. Pad for rotated windows used in DFT analyses (offsets RELATIVE2ORIGIN).
    let diag = (2.0 * f64::from(map_windowsize) * f64::from(map_windowsize)).sqrt();
    let pad = (diag - f64::from(map_windowsize)) / 2.0;
    let pad = trunc_dbl_precision(pad, TRUNC_SCALE);
    // Must add the window offset to the rotational padding.
    let dft_pad = sround(pad) + map_windowoffset;

    // 2. Pad for rotated blocks used in directional binarization (offsets RELATIVE2CENTER).
    let diag = f64::from(dirbin_grid_w * dirbin_grid_w + dirbin_grid_h * dirbin_grid_h).sqrt();
    let pad = (diag - 1.0) / 2.0;
    let pad = trunc_dbl_precision(pad, TRUNC_SCALE);
    let dirbin_pad = sround(pad);

    dft_pad.max(dirbin_pad)
}

/// `init_rotgrids` — precompute rotated pixel offsets for `ndirs` grid orientations.
///
/// Stock `init_rotgrids` (init.c L471). Rotates a `grid_w x grid_h` grid about its center for each
/// of `ndirs` directions starting at `start_dir_angle` and stepping `PI/ndirs`, and stores, per
/// pixel, the signed offset `ixt + iyt * pw` into the padded image (`pw = iw + 2*pad`).
///
/// The grid's own required padding is `round((diag - 1)/2)` for [`Relative2::Center`] or
/// `round((diag - min(w,h))/2)` for [`Relative2::Origin`] (`diag` = grid diagonal), quantized to
/// `1/16384`. `ipad` selects the padding actually used: [`None`] (stock `UNDEFINED`) adopts the
/// grid's own pad; `Some(p)` uses `p` (which callers size via [`get_max_padding_V2`] so a single
/// padded image serves every grid — it must be at least the grid's own pad).
///
/// The rotation transform of pixel `P=(ix,iy)` about center `C=(cx,cy)` is
/// `Rx = cx + (ix-cx)cos - (iy-cy)sin`, `Ry = cy + (ix-cx)sin + (iy-cy)cos`; for
/// [`Relative2::Center`] the leading `cx`/`cy` are dropped (offsets stay center-relative). Each
/// `fx`/`fy` is quantized then [`sround`]ed, verbatim with stock.
///
/// (Stock's `ih` image-height parameter is intentionally omitted: `init_rotgrids` never reads it —
/// only `iw` feeds the padded width `pw`.)
pub(crate) fn init_rotgrids(
    iw: i32,
    ipad: Option<i32>,
    start_dir_angle: f64,
    ndirs: i32,
    grid_w: i32,
    grid_h: i32,
    relative2: Relative2,
) -> RotGrids {
    // Compute the grid's own required pad from its diagonal.
    let diag = f64::from(grid_w * grid_w + grid_h * grid_h).sqrt();
    let grid_pad = match relative2 {
        Relative2::Center => {
            // Grid centers reside in valid memory.
            let pad = (diag - 1.0) / 2.0;
            sround(trunc_dbl_precision(pad, TRUNC_SCALE))
        }
        Relative2::Origin => {
            // Grid origins reside in valid memory; pad off the smallest dimension.
            let min_dim = grid_w.min(grid_h);
            let pad = (diag - f64::from(min_dim)) / 2.0;
            sround(trunc_dbl_precision(pad, TRUNC_SCALE))
        }
    };

    // UNDEFINED -> use the grid's own pad; otherwise use the caller's pad (must be large enough).
    let pad = match ipad {
        None => grid_pad,
        Some(p) => {
            debug_assert!(
                p >= grid_pad,
                "init_rotgrids: pad {p} too small for grid (needs >= {grid_pad})"
            );
            p
        }
    };

    // Width of the "padded" image — the multiplier on every offset's y-component.
    let pw = iw + (pad << 1);

    // Center coord of grid (0-oriented).
    let cx = f64::from(grid_w - 1) / 2.0;
    let cy = f64::from(grid_h - 1) / 2.0;

    // pi_offset is the radian offset at which angles begin; pi_incr steps a semicircle over ndirs.
    let pi_offset = start_dir_angle;
    let pi_incr = std::f64::consts::PI / f64::from(ndirs);

    let mut grids = Vec::with_capacity(ndirs.max(0) as usize);
    for dir in 0..ndirs {
        let theta = pi_offset + f64::from(dir) * pi_incr;
        let cs = theta.cos();
        let sn = theta.sin();

        let mut grid = Vec::with_capacity((grid_w * grid_h).max(0) as usize);
        for iy in 0..grid_h {
            // Rotation factors dependent on iy.
            let mut fxm = -((f64::from(iy) - cy) * sn);
            let mut fym = (f64::from(iy) - cy) * cs;
            // For origin-relative offsets, fold the center back in.
            if relative2 == Relative2::Origin {
                fxm += cx;
                fym += cy;
            }

            for ix in 0..grid_w {
                // Combine the ix-dependent factors with the iy-dependent ones.
                let fx = fxm + ((f64::from(ix) - cx) * cs);
                let fy = fym + ((f64::from(ix) - cx) * sn);
                // Truncate precision so answers are consistent across architectures.
                let fx = trunc_dbl_precision(fx, TRUNC_SCALE);
                let fy = trunc_dbl_precision(fy, TRUNC_SCALE);
                let ixt = sround(fx);
                let iyt = sround(fy);
                // Multiply the y-component of the offset by the padded image width.
                grid.push(ixt + iyt * pw);
            }
        }
        grids.push(grid);
    }

    RotGrids {
        pad,
        relative2,
        start_angle: start_dir_angle,
        ngrids: ndirs,
        grid_w,
        grid_h,
        grids,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::{
        DFT_COEFS, MAP_WINDOWOFFSET_V2, MAP_WINDOWSIZE_V2, NUM_DFT_WAVES, NUM_DIRECTIONS,
    };

    // --- init_dir2rad: quantized cos/sin ------------------------------------------------------
    #[test]
    fn dir2rad_matches_stock_known_values() {
        let t = init_dir2rad(NUM_DIRECTIONS);
        assert_eq!(t.ndirs, 16);
        assert_eq!(t.cos.len(), 16);
        assert_eq!(t.sin.len(), 16);

        // theta(0) = 0: cos 1, sin 0.
        assert_eq!(t.cos[0], 1.0);
        assert_eq!(t.sin[0], 0.0);
        // theta(4) = PI/2: cos ~6.1e-17 QUANTIZES to exactly 0.0; sin exactly 1.0.
        // This exact 0.0 is the signature of dir2rad's truncation (contrast dftwaves below).
        assert_eq!(t.cos[4], 0.0);
        assert_eq!(t.sin[4], 1.0);
        // theta(8) = PI: cos -1, sin 0.
        assert_eq!(t.cos[8], -1.0);
        assert_eq!(t.sin[8], 0.0);

        // Every entry is an exact multiple of 1/16384.
        for &c in t.cos.iter().chain(t.sin.iter()) {
            assert_eq!((c * 16384.0).round(), c * 16384.0);
        }
    }

    // --- init_dftwaves: RAW (un-truncated) cos/sin --------------------------------------------
    #[test]
    fn dftwaves_are_raw_untruncated() {
        let w = init_dftwaves(&DFT_COEFS, NUM_DFT_WAVES, MAP_WINDOWSIZE_V2);
        assert_eq!(w.nwaves, 4);
        assert_eq!(w.wavelen, 24);
        assert_eq!(w.waves.len(), 4);
        for wave in &w.waves {
            assert_eq!(wave.cos.len(), 24);
            assert_eq!(wave.sin.len(), 24);
        }

        // Wave 0, sample 0: cos 1, sin 0.
        assert_eq!(w.waves[0].cos[0], 1.0);
        assert_eq!(w.waves[0].sin[0], 0.0);

        // Wave 0, sample 6: x = (2PI/24)*6 = PI/2. Stock does NOT quantize, so cos is the raw
        // ~6.123233995736766e-17 (NOT the 0.0 that dir2rad would produce) and sin is 1.0.
        let c = w.waves[0].cos[6];
        assert!(c != 0.0, "dftwaves must keep the raw non-zero cos, got {c}");
        assert!(c.abs() < 1e-15);
        assert!((w.waves[0].sin[6] - 1.0).abs() < 1e-15);
    }

    // --- get_max_padding_V2 -------------------------------------------------------------------
    #[test]
    fn max_padding_v2_shipping_geometry_is_13() {
        // DFT window pad = round((sqrt(1152)-24)/2)+8 = 5+8 = 13; dirbin = round((sqrt(130)-1)/2) = 5.
        assert_eq!(get_max_padding_V2(24, 8, 7, 9), 13);
        // Same via the named constants.
        assert_eq!(
            get_max_padding_V2(MAP_WINDOWSIZE_V2, MAP_WINDOWOFFSET_V2, 7, 9),
            13
        );
    }

    // --- init_rotgrids: pad derivation --------------------------------------------------------
    #[test]
    fn rotgrids_pad_derivation() {
        // RELATIVE2ORIGIN, 24x24 window: pad = round((sqrt(1152)-24)/2) = 5.
        let g = init_rotgrids(100, None, 0.0, 4, 24, 24, Relative2::Origin);
        assert_eq!(g.pad, 5);

        // RELATIVE2CENTER, 3x3 grid: pad = round((sqrt(18)-1)/2) = 2.
        let g = init_rotgrids(10, None, 0.0, 4, 3, 3, Relative2::Center);
        assert_eq!(g.pad, 2);
    }

    // --- init_rotgrids: exact offsets at theta = 0 (center-relative) --------------------------
    #[test]
    fn rotgrids_center_offsets_at_zero_angle() {
        // start_angle 0, dir 0 -> theta 0 -> cos 1, sin 0. Center of a 3x3 grid is (1,1).
        // fx = ix-1, fy = iy-1, so offset = (ix-1) + (iy-1)*pw.
        let iw = 10;
        let g = init_rotgrids(iw, None, 0.0, 4, 3, 3, Relative2::Center);
        // pad = 2 -> pw = 10 + 4 = 14.
        let pw = iw + 2 * g.pad;
        assert_eq!(pw, 14);

        let grid0 = &g.grids[0];
        assert_eq!(grid0.len(), 9);
        // Row-major over (iy, ix): expected (ix-1) + (iy-1)*pw.
        let expected: Vec<i32> = (0..3)
            .flat_map(|iy| (0..3).map(move |ix| (ix - 1) + (iy - 1) * pw))
            .collect();
        assert_eq!(grid0, &expected);
        // Spot points: top-left = -1-pw, center = 0, bottom-right = 1+pw.
        assert_eq!(grid0[0], -1 - pw);
        assert_eq!(grid0[4], 0);
        assert_eq!(grid0[8], 1 + pw);
    }

    // --- init_rotgrids: offsets track the padded width via the caller's pad --------------------
    #[test]
    fn rotgrids_offsets_use_caller_pad_width() {
        // A larger caller-supplied pad widens pw, which scales every y-component of the offsets.
        let iw = 20;
        let big_pad = 9; // >= grid's own pad (2 for a 3x3 center grid)
        let g = init_rotgrids(iw, Some(big_pad), 0.0, 4, 3, 3, Relative2::Center);
        assert_eq!(g.pad, big_pad);
        let pw = iw + 2 * big_pad; // 38
                                   // Bottom-centre pixel (ix=1, iy=2) at theta 0 -> offset (0) + (1)*pw = pw.
                                   // Index in row-major 3x3 is iy*3 + ix = 2*3 + 1 = 7.
        assert_eq!(g.grids[0][7], pw);
    }

    // --- init_rotgrids: structural attributes -------------------------------------------------
    #[test]
    fn rotgrids_attributes() {
        let g = init_rotgrids(64, None, START_ANGLE, 16, 24, 24, Relative2::Origin);
        assert_eq!(g.ngrids, 16);
        assert_eq!(g.grid_w, 24);
        assert_eq!(g.grid_h, 24);
        assert_eq!(g.relative2, Relative2::Origin);
        assert_eq!(g.start_angle, START_ANGLE);
        assert_eq!(g.grids.len(), 16);
        for grid in &g.grids {
            assert_eq!(grid.len(), 24 * 24);
        }
    }

    const START_ANGLE: f64 = std::f64::consts::PI / 2.0;
}
