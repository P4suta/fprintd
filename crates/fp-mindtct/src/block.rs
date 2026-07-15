// SPDX-FileCopyrightText: NIST NBIS — U.S. Government work, public domain (title 17 §105)
//
// SPDX-License-Identifier: LicenseRef-NBIS-PD

//! Image block partitioning and the block-level low-contrast test.
//!
//! Faithful port of the two block-geometry routines in stock NBIS `mindtct/src/lib/mindtct/block.c`:
//!
//! * [`block_offsets`] — carve the (unpadded) image into an `mw × mh` grid of `blocksize`-square
//!   blocks and return, for each block, the pixel offset of its top-left corner **in the padded
//!   image**. When a dimension is not an even multiple of `blocksize`, the last column/row of blocks
//!   is pulled flush against the far edge and thus *overlaps* its neighbor (the stock "leftover"
//!   strategy) — never a partial block.
//! * [`low_contrast_block`] — build a 64-bin (6-bit) intensity histogram over one block and flag it
//!   low-contrast when the 10th/90th percentile spread is below `min_contrast_delta`.
//!
//! The percentile threshold is computed with the port's bit-exact numeric primitives
//! ([`trunc_dbl_precision`] then [`sround`]) so the rounding matches stock to the bit. The remaining
//! `block.c` routines (`find_valid_block`, `set_margin_blocks`) belong to the map stage and live in
//! `maps`. See `docs/mindtct-algorithm.md`.

use crate::consts::TRUNC_SCALE;
use crate::num::{sround, trunc_dbl_precision};
use crate::params::LfsParms;

/// `IMG_6BIT_PIX_LIMIT` — pixel-value limit of a 6-bit image (`lfs.h`), i.e. the number of histogram
/// bins in [`low_contrast_block`]. Defined locally because `consts` does not export it.
const IMG_6BIT_PIX_LIMIT: usize = 64;

/// The result of [`block_offsets`]: the per-block origin offsets plus the block-grid dimensions.
///
/// Mirrors the stock C out-parameters `optr` / `ow` / `oh`. `offsets` is `map_w * map_h` long in
/// row-major block order (all of row 0 left→right, then row 1, …); each entry is a pixel offset into
/// the **padded** image to that block's top-left corner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BlockOffsets {
    /// Pixel offsets (into the padded image) of each block's top-left corner, row-major.
    pub offsets: Vec<i32>,
    /// Number of block columns, `mw = ceil(iw / blocksize)` (stock `ow` / `bw`).
    pub map_w: i32,
    /// Number of block rows, `mh = ceil(ih / blocksize)` (stock `oh` / `bh`).
    pub map_h: i32,
}

/// Divide an image into `mw × mh` equally sized blocks, returning the padded-image offset of each
/// block's origin — the stock `block_offsets` (`block.c` L102).
///
/// For images whose dimensions are even multiples of `blocksize`, blocks are non-overlapping and
/// immediately adjacent. Otherwise the last column and/or last row of blocks is placed flush against
/// the far edge of the unpadded image and comes *inward* `blocksize` pixels, overlapping its
/// neighbor — so processing arbitrary sizes never produces a partial block. Offsets account for the
/// surrounding `pad` pixels: the grid begins `pad` rows down and `pad` columns in.
///
/// `mw = ceil(iw / blocksize)` and `mh = ceil(ih / blocksize)`, taken as a floating-point division
/// then ceiling exactly as stock (`(int)ceil(iw/(double)blocksize)`).
///
/// # Errors
///
/// Returns `Err(-80)` when the unpadded image is smaller than a single block in either dimension
/// (`iw < blocksize || ih < blocksize`), mirroring the stock C error code. (The stock `-81` malloc
/// failure has no analogue here.)
pub(crate) fn block_offsets(
    iw: i32,
    ih: i32,
    pad: i32,
    blocksize: i32,
) -> Result<BlockOffsets, i32> {
    // Test if unpadded image is smaller than a single block.
    if iw < blocksize || ih < blocksize {
        return Err(-80);
    }

    // Padded width of image (padded height is unused by this routine, as in stock).
    let pad2 = pad << 1;
    let pw = iw + pad2;

    // Number of columns/rows of blocks: ceiling to account for the right/bottom "leftovers".
    let bw = (f64::from(iw) / f64::from(blocksize)).ceil() as i32;
    let bh = (f64::from(ih) / f64::from(blocksize)).ceil() as i32;

    // Total number of blocks (allocation hint only; `push` preserves the stock `bi++` order).
    let bsize = (bw * bh) as usize;

    // Index of the last column / last row.
    let lastbw = bw - 1;
    let lastbh = bh - 1;

    let mut blkoffs: Vec<i32> = Vec::with_capacity(bsize);

    // Offset from the top of the padded image to the start of the current row of unpadded blocks.
    // Indented `pad` in from the left edge of the padded image.
    let mut blkrow_start = (pad * pw) + pad;
    // Number of pixels in a row of blocks in the padded image (row width X block height).
    let blkrow_size = pw * blocksize;

    // Foreach non-overlapping row of blocks in the image.
    for _by in 0..lastbh {
        let mut offset = blkrow_start;
        // Foreach non-overlapping column of blocks in the image.
        for _bx in 0..lastbw {
            blkoffs.push(offset);
            offset += blocksize;
        }
        // "Left-over" block in the last column: flush to the far right edge of the unpadded image,
        // coming in BLOCKSIZE pixels.
        blkoffs.push(blkrow_start + iw - blocksize);
        // Bump to the beginning of the next row of blocks.
        blkrow_start += blkrow_size;
    }

    // "Left-over" row of blocks at the bottom of the image: flush to the bottom edge of the unpadded
    // image, coming up BLOCKSIZE pixels (still accounting for padding).
    let blkrow_start = ((pad + ih - blocksize) * pw) + pad;
    let mut offset = blkrow_start;
    for _bx in 0..lastbw {
        blkoffs.push(offset);
        offset += blocksize;
    }
    // Last "left-over" block in the last row.
    blkoffs.push(blkrow_start + iw - blocksize);

    Ok(BlockOffsets {
        offsets: blkoffs,
        map_w: bw,
        map_h: bh,
    })
}

/// Analyze one image block's pixel intensities and report whether it has too *little* contrast for
/// further processing — the stock `low_contrast_block` (`block.c` L224).
///
/// Builds a 64-bin histogram over the `blocksize × blocksize` block anchored at `blkoffset` in the
/// padded image `pdata` (row stride `pw`), then derives the `percentile_min_max`-th percentile
/// minimum and maximum intensities (`prctmin` / `prctmax`). The block is **low contrast**
/// (`Ok(true)`) when `prctmax - prctmin < min_contrast_delta`, else high contrast (`Ok(false)`).
///
/// The percentile rank is `sround(trunc_dbl_precision((pct/100) * (numpix - 1), TRUNC_SCALE))`,
/// reproducing stock's exact quantize-then-round so the min/max cutoffs land on the same bins.
///
/// `_ph` (padded height) is part of the stock signature but unused by the routine. `pdata` must hold
/// 6-bit values (`0..IMG_6BIT_PIX_LIMIT`); an out-of-range pixel would index past the histogram — the
/// same 6-bit-image precondition the stock C assumes.
///
/// # Errors
///
/// Returns `Err(-510)` / `Err(-511)` if the min / max percentile pixel is not found — only reachable
/// for an empty block (`blocksize <= 0`), matching the stock C error codes.
pub(crate) fn low_contrast_block(
    blkoffset: i32,
    blocksize: i32,
    pdata: &[u8],
    pw: i32,
    _ph: i32,
    lfsparms: &LfsParms,
) -> Result<bool, i32> {
    let numpix = blocksize * blocksize;
    let mut pixtable = [0i32; IMG_6BIT_PIX_LIMIT];

    // Percentile rank into the sorted intensities: quantize then round, bit-for-bit as stock.
    let tdbl = (f64::from(lfsparms.percentile_min_max) / 100.0) * f64::from(numpix - 1);
    let tdbl = trunc_dbl_precision(tdbl, TRUNC_SCALE);
    let prctthresh = sround(tdbl);

    // Accumulate the block's intensity histogram (row stride is the padded width `pw`).
    let start = blkoffset as usize;
    let stride = pw as usize;
    for py in 0..blocksize {
        let row = start + (py as usize) * stride;
        for px in 0..blocksize {
            let v = pdata[row + px as usize];
            pixtable[usize::from(v)] += 1;
        }
    }

    // Walk up from the darkest bin to the first bin whose cumulative count reaches the rank.
    let mut prctmin = 0i32;
    let mut pixsum = 0i32;
    let mut found = false;
    for (pi, &count) in pixtable.iter().enumerate() {
        pixsum += count;
        if pixsum >= prctthresh {
            prctmin = pi as i32;
            found = true;
            break;
        }
    }
    if !found {
        return Err(-510);
    }

    // Walk down from the brightest bin to the first bin whose cumulative count reaches the rank.
    let mut prctmax = 0i32;
    let mut pixsum = 0i32;
    let mut found = false;
    for pi in (0..IMG_6BIT_PIX_LIMIT).rev() {
        pixsum += pixtable[pi];
        if pixsum >= prctthresh {
            prctmax = pi as i32;
            found = true;
            break;
        }
    }
    if !found {
        return Err(-511);
    }

    let delta = prctmax - prctmin;

    Ok(delta < lfsparms.min_contrast_delta)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::LFSPARMS_V2;

    // ---- block_offsets ----------------------------------------------------------------------

    /// Even multiple, no padding: 16x16 image, 8x8 blocks -> a clean 2x2 non-overlapping grid.
    /// Offsets are `by * (pw * blocksize) + bx * blocksize` with `pw = iw = 16`.
    #[test]
    fn block_offsets_even_multiple_no_pad() {
        let b = block_offsets(16, 16, 0, 8).unwrap();
        assert_eq!(b.map_w, 2);
        assert_eq!(b.map_h, 2);
        // row 0: cols at 0, 8; row 1 starts at 8*16 = 128: 128, 136.
        assert_eq!(b.offsets, vec![0, 8, 128, 136]);
    }

    /// Padding shifts every origin by `pad` rows and `pad` cols of the padded image
    /// (`pw = iw + 2*pad = 32`), and the last row is measured up from the bottom edge.
    #[test]
    fn block_offsets_even_multiple_with_pad() {
        let b = block_offsets(16, 16, 8, 8).unwrap();
        assert_eq!((b.map_w, b.map_h), (2, 2));
        // blkrow_start = pad*pw + pad = 8*32 + 8 = 264; last row = (8+16-8)*32 + 8 = 520.
        assert_eq!(b.offsets, vec![264, 272, 520, 528]);
    }

    /// Fractional width forces the "leftover" overlap: iw=20 -> ceil(20/8)=3 columns, the last of
    /// which starts at `iw - blocksize = 12`, overlapping the middle block (starts at 8).
    #[test]
    fn block_offsets_fractional_width_overlaps() {
        let b = block_offsets(20, 8, 0, 8).unwrap();
        assert_eq!((b.map_w, b.map_h), (3, 1));
        // Single row (bh=1): non-overlapping cols 0, 8, then the flush-right leftover at 12.
        assert_eq!(b.offsets, vec![0, 8, 12]);
    }

    /// Fractional in both dimensions: iw=20, ih=12 -> 3x2 grid. Verifies the last *row* is also
    /// pulled flush to the bottom edge (`(ih - blocksize) * pw = 4 * 20 = 80`).
    #[test]
    fn block_offsets_fractional_both_dims() {
        let b = block_offsets(20, 12, 0, 8).unwrap();
        assert_eq!((b.map_w, b.map_h), (3, 2));
        // row 0 at y=0: 0, 8, 12 ; last row flush at y=4 (offset 80): 80, 88, 92.
        assert_eq!(b.offsets, vec![0, 8, 12, 80, 88, 92]);
        assert_eq!(b.offsets.len(), (b.map_w * b.map_h) as usize);
    }

    /// Image smaller than one block in either dimension is the stock `-80` error.
    #[test]
    fn block_offsets_too_small_is_err() {
        assert_eq!(block_offsets(4, 16, 0, 8), Err(-80));
        assert_eq!(block_offsets(16, 4, 0, 8), Err(-80));
    }

    // ---- low_contrast_block -----------------------------------------------------------------

    /// A flat block (every pixel the same value) has zero spread -> unambiguously low contrast.
    #[test]
    fn low_contrast_uniform_block_is_low() {
        let data = vec![30u8; 64]; // 8x8, tightly packed (pw == blocksize).
        assert_eq!(
            low_contrast_block(0, 8, &data, 8, 8, &LFSPARMS_V2),
            Ok(true)
        );
    }

    /// Half dark (10) / half bright (60): percentile min lands on 10, max on 60, delta 50 >= 5.
    #[test]
    fn low_contrast_split_block_is_high() {
        let mut data = vec![10u8; 64];
        for v in data.iter_mut().take(64).skip(32) {
            *v = 60;
        }
        assert_eq!(
            low_contrast_block(0, 8, &data, 8, 8, &LFSPARMS_V2),
            Ok(false)
        );
    }

    /// Delta exactly equal to `min_contrast_delta` (5) is NOT low contrast: the test is strict `<`.
    /// 32 px at 10 and 32 px at 15 -> prctmin=10, prctmax=15, delta=5.
    #[test]
    fn low_contrast_delta_equal_threshold_is_high() {
        let mut data = vec![10u8; 64];
        for v in data.iter_mut().take(64).skip(32) {
            *v = 15;
        }
        assert_eq!(LFSPARMS_V2.min_contrast_delta, 5);
        assert_eq!(
            low_contrast_block(0, 8, &data, 8, 8, &LFSPARMS_V2),
            Ok(false)
        );
    }

    /// One below threshold (delta 4 < 5) tips to low contrast. 32 px at 10 and 32 px at 14.
    #[test]
    fn low_contrast_delta_below_threshold_is_low() {
        let mut data = vec![10u8; 64];
        for v in data.iter_mut().take(64).skip(32) {
            *v = 14;
        }
        assert_eq!(
            low_contrast_block(0, 8, &data, 8, 8, &LFSPARMS_V2),
            Ok(true)
        );
    }

    /// Row-stride correctness: a padded image (`pw = 12`) whose 8x8 block region is uniform (30) but
    /// whose surrounding padding is high-contrast (63). Only the block's pixels must be counted, so
    /// the verdict is low contrast — a wrong stride would fold in the bright padding and flip it.
    #[test]
    fn low_contrast_respects_padded_stride() {
        let pw = 12i32;
        let ph = 12i32;
        let pad = 2i32;
        let mut data = vec![63u8; (pw * ph) as usize];
        // Fill the 8x8 block at (pad, pad) with a single uniform value.
        for by in 0..8 {
            let row = ((pad + by) * pw + pad) as usize;
            for bx in 0..8 {
                data[row + bx as usize] = 30;
            }
        }
        let blkoffset = pad * pw + pad;
        assert_eq!(
            low_contrast_block(blkoffset, 8, &data, pw, ph, &LFSPARMS_V2),
            Ok(true)
        );
    }

    /// The percentile rank matches the stock quantize-then-round chain for the V2 8x8 block:
    /// `sround(trunc_dbl_precision(0.10 * 63, 16384)) == 6`. A block with only 5 dark pixels (below
    /// the rank of 6) must not let those pixels set `prctmin`.
    #[test]
    fn low_contrast_percentile_rank_ignores_sub_threshold_tail() {
        // 5 px at value 0 (below the rank-6 cutoff), 59 px at value 40.
        let mut data = vec![40u8; 64];
        for v in data.iter_mut().take(5) {
            *v = 0;
        }
        // prctmin: cumulative reaches 6 only at value 40 (5 zeros < 6). prctmax: 40. delta 0 -> low.
        assert_eq!(
            low_contrast_block(0, 8, &data, 8, 8, &LFSPARMS_V2),
            Ok(true)
        );
    }
}
