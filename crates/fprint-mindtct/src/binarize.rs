// SPDX-FileCopyrightText: NIST NBIS — U.S. Government work, public domain (title 17 §105)
//
// SPDX-License-Identifier: LicenseRef-NBIS-PD

//! Directional binarization of the padded image (`binarize_V2`) into a black-ridge/white-valley
//! bitmap, driven by the direction map and the rotated dir-bin grids.
//!
//! Faithful port of the stock NBIS `_V2` binarization path — `binarize_V2` (binar.c L152),
//! `binarize_image_V2` (binar.c L287), `dirbinarize` (binar.c L359) — plus the hole-filling
//! morphology `fill_holes` (imgutil.c L246). The V2 path deliberately drops the V1 *isotropic*
//! binarization: a block is either directionally binarized (valid direction) or forced WHITE
//! (`INVALID_DIR`); there is no `isobinarize` branch.
//!
//! Output convention (the pre-`gray2bin` `bdata`): `0` = ridge (BLACK), `255` = valley (WHITE),
//! full resolution at the *original* image size — the dir-bin grid pad is stripped exactly as
//! `binarize_image_V2` computes `bw = pw - 2*pad`, `bh = ph - 2*pad`.
//!
//! ## Bit-exactness
//!
//! [`dirbinarize`]'s center-row index is `cy = sround(trunc_dbl_precision((grid_h-1)/2, 1/16384))`,
//! matching stock verbatim (the `1/16384` truncation keeps the double→int rounding architecture-
//! stable). All pixel accumulation is integer (`i32`) over 6-bit pixels, and the BLACK/WHITE
//! decision is the integer comparison `csum * grid_h < gsum`. The scan orders — pixel raster,
//! grid row/column walk, and the horizontal-then-vertical hole fill with its "skip the pixel we
//! just proved isn't a hole" 2-step — are reproduced exactly.

use crate::consts::TRUNC_SCALE;
use crate::init::RotGrids;
use crate::num::{sround, trunc_dbl_precision};
use crate::params::LfsParms;

/// `WHITE_PIXEL` (`lfs.h` L303) — valley intensity in the pre-`gray2bin` binary image.
const WHITE_PIXEL: u8 = 255;
/// `BLACK_PIXEL` (`lfs.h` L304) — ridge intensity in the pre-`gray2bin` binary image.
const BLACK_PIXEL: u8 = 0;
/// `INVALID_DIR` (`lfs.h` L320) — a block with no assigned ridge-flow direction.
const INVALID_DIR: i32 = -1;

/// The binarized image plus its (unpadded) dimensions — the structured analogue of stock
/// `binarize_V2`'s `odata`/`ow`/`oh` out-parameters.
///
/// `data` is `width * height` bytes in row-major order, `0` = ridge and `255` = valley (the
/// pre-`gray2bin` form). `width`/`height` equal the *original* image size, the dir-bin grid pad
/// having been stripped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BinaryImage {
    /// Binary pixels, row-major, `0` = ridge (BLACK) / `255` = valley (WHITE).
    pub(crate) data: Vec<u8>,
    /// Width (in pixels) of the binary image (= original image width).
    pub(crate) width: i32,
    /// Height (in pixels) of the binary image (= original image height).
    pub(crate) height: i32,
}

/// `binarize_V2` — binarize the padded image from its Direction Map, then fill 1-pixel holes.
///
/// Stock `binarize_V2` (binar.c L152). Runs [`binarize_image_V2`] to produce the raw directional
/// bitmap, then scans it filling length-1 holes `lfsparms.num_fill_holes` times (the shipping `_V2`
/// value is `3`). Returns the unpadded [`BinaryImage`].
///
// PORT: stock's `mh` (map height) parameter is intentionally omitted — `binarize_image_V2` never
// reads it (only `mw` indexes the row-major Direction Map), the same way `init_rotgrids` drops
// stock's unused `ih`. `blocksize` and `num_fill_holes` are taken from `lfsparms`.
pub(crate) fn binarize_v2(
    pdata: &[u8],
    pw: i32,
    ph: i32,
    direction_map: &[i32],
    mw: i32,
    dirbingrids: &RotGrids,
    lfsparms: &LfsParms,
) -> BinaryImage {
    // 1. Binarize the padded input image using directional block info.
    let mut bin = binarize_image_v2(
        pdata,
        pw,
        ph,
        direction_map,
        mw,
        lfsparms.blocksize,
        dirbingrids,
    );

    // 2. Fill black and white holes in the binary image. LFS scans the image, filling holes,
    //    `num_fill_holes` times.
    for _ in 0..lfsparms.num_fill_holes {
        fill_holes(&mut bin.data, bin.width, bin.height);
    }

    bin
}

/// `binarize_image_V2` — directionally binarize every pixel of the padded image.
///
/// Stock `binarize_image_V2` (binar.c L287). Walks the *unpadded* pixel grid in raster order; for
/// each pixel it finds the block it belongs to (`bx = ix/blocksize`, `by = iy/blocksize`), reads
/// the Direction Map, and either forces WHITE (block direction is `INVALID_DIR`) or
/// [`dirbinarize`]s the pixel against that direction's rotated grid. Note there is **no** isotropic
/// binarization branch in V2. The input pointer starts at the padded image's interior origin
/// (`pad*pw + pad`) so the rotated grids never read outside the allocation.
fn binarize_image_v2(
    pdata: &[u8],
    pw: i32,
    ph: i32,
    direction_map: &[i32],
    mw: i32,
    blocksize: i32,
    dirbingrids: &RotGrids,
) -> BinaryImage {
    let pad = dirbingrids.pad;
    // Dimensions of the "unpadded" binary image results.
    let bw = pw - (pad << 1);
    let bh = ph - (pad << 1);

    let mut bdata = vec![0u8; (bw * bh).max(0) as usize];

    let mut bi = 0usize;
    for iy in 0..bh {
        // Index of the first interior pixel on this row of the padded image: (pad+iy)*pw + pad.
        // This is stock's `spptr` (start) advanced by `iy*pw`, with `pptr = spptr + ix`.
        let row_base = (pad + iy) * pw + pad;
        // Block row for this pixel row.
        let by = iy / blocksize;
        for ix in 0..bw {
            // Compute which block the current pixel is in.
            let bx = ix / blocksize;
            // Get the corresponding value in the Direction Map.
            let mapval = direction_map[(by * mw + bx) as usize];
            let pptr = (row_base + ix) as usize;
            bdata[bi] = if mapval == INVALID_DIR {
                // If the current block has an INVALID direction, set the pixel WHITE.
                WHITE_PIXEL
            } else {
                // Otherwise the block has a valid direction: use directional binarization.
                dirbinarize(pdata, pptr, mapval, dirbingrids)
            };
            bi += 1;
        }
    }

    BinaryImage {
        data: bdata,
        width: bw,
        height: bh,
    }
}

/// `dirbinarize` — binarize one grayscale pixel against a VALID ridge-flow direction.
///
/// Stock `dirbinarize` (binar.c L359). Samples the `grid_w * grid_h` rotated grid for direction
/// `idir` centered on the pixel at `pptr` (a flat index into `pdata`; the grid offsets are signed
/// and already scaled by the padded width). It sums every grid pixel (`gsum`) and, separately, the
/// center row's sum (`csum`, the row at `cy = sround(trunc_dbl_precision((grid_h-1)/2, 1/16384))`).
/// If the center row treated as an average is darker than the whole grid — `csum * grid_h < gsum`
/// — the pixel is on a ridge and returns [`BLACK_PIXEL`], else [`WHITE_PIXEL`].
///
/// # Panics
/// If a grid offset lands outside `pdata`; the caller must pad the image to the grid's radius (the
/// pipeline pads by `get_max_padding_V2`, which covers the dir-bin grid).
fn dirbinarize(pdata: &[u8], pptr: usize, idir: i32, dirbingrids: &RotGrids) -> u8 {
    // Nickname the rotated grid for this direction.
    let grid = &dirbingrids.grids[idir as usize];
    let grid_w = dirbingrids.grid_w;
    let grid_h = dirbingrids.grid_h;

    // Center (0-oriented) row in the grid. Truncate precision so the double→int rounding is
    // consistent across architectures, exactly as stock does.
    let dcy = f64::from(grid_h - 1) / 2.0;
    let dcy = trunc_dbl_precision(dcy, TRUNC_SCALE);
    let cy = sround(dcy);

    let mut gi = 0usize;
    let mut gsum: i32 = 0;
    let mut csum: i32 = 0;

    // Foreach row in the grid ...
    for gy in 0..grid_h {
        // Sum this rotated row.
        let mut rsum: i32 = 0;
        for _gx in 0..grid_w {
            // Accumulate the next pixel along the rotated row in the grid.
            let idx = (pptr as isize + grid[gi] as isize) as usize;
            rsum += i32::from(pdata[idx]);
            gi += 1;
        }
        // Accumulate the row sum into the grid pixel sum.
        gsum += rsum;
        // If the current row is the center row, save its sum separately.
        if gy == cy {
            csum = rsum;
        }
    }

    // If the center row sum, treated as an average, is less than the grid's total pixel sum, the
    // pixel is on a ridge -> BLACK; otherwise it is a valley -> WHITE.
    if csum * grid_h < gsum {
        BLACK_PIXEL
    } else {
        WHITE_PIXEL
    }
}

/// `fill_holes` — fill 1-pixel-wide holes in the binary image, horizontally then vertically.
///
/// Stock `fill_holes` (imgutil.c L246). A "hole" is a pixel whose two opposing neighbors are equal
/// to each other but different from it; it is overwritten with the neighbors' value. The horizontal
/// pass scans each row (skipping the far-left/right columns), the vertical pass each column
/// (skipping the top/bottom rows). Both passes, on filling a hole, **advance an extra step** past
/// the just-proven-non-hole neighbor — reproduced here by the `+2` index jump and matching loop
/// counter bump, because it changes which later pixels are examined and is therefore load-bearing.
/// Operates in place on `bdata` (`iw * ih`, row-major).
fn fill_holes(bdata: &mut [u8], iw: i32, ih: i32) {
    let iw = iw.max(0) as usize;
    let ih = ih.max(0) as usize;

    // 1. Fill 1-pixel-wide holes in horizontal runs first ...
    // The row anchor `sptr` (stock `bdata + 1` advanced by `iy*iw`) starts the middle pixel one
    // column into each row.
    for iy in 0..ih {
        let sptr = 1 + iy * iw; // bdata + 1 + iy*iw
                                // Left / middle / right pixel indices at the start of this row.
        let mut lptr = sptr - 1;
        let mut mptr = sptr;
        let mut rptr = sptr + 1;
        // Foreach column in the image (less the far left and right pixels) ...
        let mut ix = 1usize;
        while ix < iw.saturating_sub(1) {
            // Do we have a horizontal hole of length 1?
            if bdata[lptr] != bdata[mptr] && bdata[lptr] == bdata[rptr] {
                // Fill it.
                bdata[mptr] = bdata[lptr];
                // Bump past the right pixel: we know it will not be a hole. (The extra `ix += 1`
                // here plus the loop's own step is the stock `ix++` inside the fill branch.)
                lptr += 2;
                mptr += 2;
                rptr += 2;
                ix += 2;
            } else {
                // Otherwise, bump to the next pixel to the right.
                lptr += 1;
                mptr += 1;
                rptr += 1;
                ix += 1;
            }
        }
    }

    // 2. Now, fill 1-pixel-wide holes in vertical runs ...
    let iw2 = iw << 1;
    // The column anchor `sptr` (stock `bdata + iw` advanced by `ix`) starts one row down from the
    // top of each column.
    for ix in 0..iw {
        let sptr = iw + ix; // bdata + iw + ix
                            // Top / middle / bottom pixel indices at the start of this column.
        let mut tptr = sptr - iw;
        let mut mptr = sptr;
        let mut bptr = sptr + iw;
        // Foreach row in the image (less the top and bottom row) ...
        let mut iy = 1usize;
        while iy < ih.saturating_sub(1) {
            // Do we have a vertical hole of length 1?
            if bdata[tptr] != bdata[mptr] && bdata[tptr] == bdata[bptr] {
                // Fill it.
                bdata[mptr] = bdata[tptr];
                // Bump past the bottom pixel: we know it will not be a hole.
                tptr += iw2;
                mptr += iw2;
                bptr += iw2;
                iy += 2;
            } else {
                // Otherwise, bump to the next pixel below.
                tptr += iw;
                mptr += iw;
                bptr += iw;
                iy += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init::Relative2;

    /// Build a hand-made [`RotGrids`] whose single direction `0` samples a `grid_w * grid_h`
    /// axis-aligned neighborhood centered on the pixel, on an image of padded width `pw`. Offsets
    /// are `(ix-cx) + (iy-cy)*pw` — the identity (unrotated) grid — so [`dirbinarize`]'s sums are
    /// easy to reason about directly.
    fn axis_grid(grid_w: i32, grid_h: i32, pw: i32, pad: i32) -> RotGrids {
        let cx = (grid_w - 1) / 2;
        let cy = (grid_h - 1) / 2;
        let mut grid = Vec::new();
        for iy in 0..grid_h {
            for ix in 0..grid_w {
                grid.push((ix - cx) + (iy - cy) * pw);
            }
        }
        RotGrids {
            pad,
            relative2: Relative2::Center,
            start_angle: 0.0,
            ngrids: 1,
            grid_w,
            grid_h,
            grids: vec![grid],
        }
    }

    // --- dirbinarize: the center-row-vs-whole-grid decision -----------------------------------
    #[test]
    fn dirbinarize_center_darker_is_black() {
        // 3x3 axis grid on a 5-wide image. cy = sround((3-1)/2) = 1 (the middle row).
        let pw = 5;
        let grids = axis_grid(3, 3, pw, 1);
        // Rows 0 and 2 bright (10), row 1 (center) dark (0). For the pixel at index `p`, the grid
        // reads the 3x3 block around it. Put a dark horizontal band on the center row.
        // gsum = 6*10 + 3*0 = 60; csum = 0; csum*3 = 0 < 60 -> BLACK.
        let mut img = vec![10u8; pw as usize * 3];
        for x in 0..pw as usize {
            img[pw as usize + x] = 0; // middle row
        }
        let p = pw as usize + 2; // center pixel, row 1 col 2
        assert_eq!(dirbinarize(&img, p, 0, &grids), BLACK_PIXEL);
    }

    #[test]
    fn dirbinarize_uniform_is_white() {
        // Uniform grid: csum*grid_h == gsum, so the strict `<` is false -> WHITE.
        let pw = 5;
        let grids = axis_grid(3, 3, pw, 1);
        let img = vec![7u8; pw as usize * 3];
        let p = pw as usize + 2;
        assert_eq!(dirbinarize(&img, p, 0, &grids), WHITE_PIXEL);
    }

    #[test]
    fn dirbinarize_center_brighter_is_white() {
        // Center row bright, others dark: csum large -> csum*3 >= gsum -> WHITE.
        let pw = 5;
        let grids = axis_grid(3, 3, pw, 1);
        let mut img = vec![0u8; pw as usize * 3];
        for x in 0..pw as usize {
            img[pw as usize + x] = 10; // middle row bright
        }
        // gsum = 3*10 = 30; csum = 30; csum*3 = 90 >= 30 -> WHITE.
        let p = pw as usize + 2;
        assert_eq!(dirbinarize(&img, p, 0, &grids), WHITE_PIXEL);
    }

    #[test]
    fn dirbinarize_center_row_index_uses_sround_truncation() {
        // grid_h = 9 -> cy = sround(trunc((9-1)/2)) = 4. Make only row 4 dark to prove csum tracks
        // exactly that row. gsum = 8*grid_w*5 + 0; csum = 0 -> BLACK. If cy were wrong, csum != 0.
        let grid_w = 3;
        let grid_h = 9;
        let pw = 7;
        let grids = axis_grid(grid_w, grid_h, pw, 4);
        let rows = grid_h as usize;
        let mut img = vec![5u8; pw as usize * rows];
        for x in 0..pw as usize {
            img[4 * pw as usize + x] = 0; // row index 4 == cy
        }
        let p = 4 * pw as usize + 3; // center of the grid (col cx=1 -> but any interior col works)
        assert_eq!(dirbinarize(&img, p, 0, &grids), BLACK_PIXEL);
    }

    // --- binarize_image_V2: INVALID_DIR block forced WHITE, valid block binarized --------------
    #[test]
    fn binarize_image_invalid_block_is_white() {
        // 1x1 block map == INVALID_DIR. Every output pixel must be WHITE regardless of image.
        // Use pad 1, blocksize huge so all pixels map to block (0,0).
        let pad = 1;
        let pw = 4;
        let ph = 4;
        let grids = axis_grid(3, 3, pw, pad);
        let pdata = vec![0u8; (pw * ph) as usize]; // all dark -> would be BLACK if binarized
        let dm = vec![INVALID_DIR];
        let bin = binarize_image_v2(&pdata, pw, ph, &dm, 1, 64, &grids);
        assert_eq!(bin.width, pw - 2 * pad);
        assert_eq!(bin.height, ph - 2 * pad);
        assert!(bin.data.iter().all(|&b| b == WHITE_PIXEL));
    }

    #[test]
    fn binarize_image_valid_block_binarizes() {
        // A valid direction (0) with a dark center band yields at least some BLACK pixels.
        let pad = 1;
        let pw = 6;
        let ph = 6;
        let grids = axis_grid(3, 3, pw, pad);
        // Dark horizontal band across the middle padded rows -> interior center pixels see a darker
        // center row than surroundings.
        let mut pdata = vec![20u8; (pw * ph) as usize];
        for x in 0..pw as usize {
            pdata[(ph as usize / 2) * pw as usize + x] = 0;
        }
        let dm = vec![0]; // one block, valid direction 0
        let bin = binarize_image_v2(&pdata, pw, ph, &dm, 1, 64, &grids);
        assert_eq!((bin.width, bin.height), (pw - 2 * pad, ph - 2 * pad));
        assert!(bin.data.contains(&BLACK_PIXEL));
    }

    // --- fill_holes: horizontal then vertical --------------------------------------------------
    #[test]
    fn fill_holes_fills_horizontal_hole() {
        // Row: 0 255 0  -> the middle is a length-1 hole (neighbors equal, center differs) -> 0.
        let mut img = vec![0u8, 255, 0];
        fill_holes(&mut img, 3, 1);
        assert_eq!(img, vec![0, 0, 0]);
    }

    #[test]
    fn fill_holes_fills_vertical_hole() {
        // Column of 3 (width 1): 0 / 255 / 0 -> middle filled to 0.
        let mut img = vec![0u8, 255, 0];
        fill_holes(&mut img, 1, 3);
        assert_eq!(img, vec![0, 0, 0]);
    }

    #[test]
    fn fill_holes_leaves_non_hole_untouched() {
        // 0 255 255: left != middle but left != right -> not a hole; unchanged.
        let mut img = vec![0u8, 255, 255];
        fill_holes(&mut img, 3, 1);
        assert_eq!(img, vec![0, 255, 255]);
    }

    #[test]
    fn fill_holes_skips_pixel_after_a_fill() {
        // 0 255 0 255 0 across one row. At ix=1 we fill (->0) and STEP PAST ix=2, so the pixel at
        // ix=2 is never re-examined as a middle. Stock semantics: after filling index 1 (using
        // neighbors 0 and 2), pointers jump to ix=3. Index 2 stays 0 (already equal to its left).
        // Trace: start 0 255 0 255 0.
        //  ix=1: L=0,M=255,R=0 -> hole, fill M=0 -> 0 0 0 255 0; jump to ix=3.
        //  ix=3: L(idx2)=0,M(idx3)=255,R(idx4)=0 -> hole, fill -> 0 0 0 0 0.
        let mut img = vec![0u8, 255, 0, 255, 0];
        fill_holes(&mut img, 5, 1);
        assert_eq!(img, vec![0, 0, 0, 0, 0]);
    }

    #[test]
    fn fill_holes_noop_on_thin_image() {
        // Width < 3 and height < 3: no interior pixels, nothing changes and nothing panics.
        let mut img = vec![0u8, 255];
        fill_holes(&mut img, 2, 1);
        assert_eq!(img, vec![0, 255]);
    }
}
