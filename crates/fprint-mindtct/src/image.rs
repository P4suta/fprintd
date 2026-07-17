// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Image plumbing: the 6-bit rescale (`bits_8to6`, a `>> 2`, and its inverse `bits_6to8`, a `<< 2`)
//! and the constant-intensity border padding (`pad_uchar_image`) that the detection front-end runs
//! before block analysis. Mirrors stock NBIS `imgutil.c` (`bits_6to8` L88, `bits_8to6` L118,
//! `pad_uchar_image` L189). See `docs/mindtct-algorithm.md`.
//!
//! Two faithful details a naive port would get wrong:
//! * **`bits_6to8` wraps like C's `unsigned char <<= 2`.** Only values on `[0..64)` (the output of
//!   `bits_8to6`) round-trip meaningfully; a stray value `>= 64` has its high bits truncated exactly
//!   as the stock byte shift does, which the `<< 2` on `u8` reproduces bit-for-bit.
//! * **Padding fills with a *constant* intensity**, not edge replication — `PAD_VALUE` (medium gray)
//!   is written across the whole allocation first, then the input is `memcpy`'d scanline-by-scanline
//!   into the centered region. The stock code notes edge-copying as an unimplemented alternative.

use crate::consts::PAD_VALUE;
use crate::GrayImage;

/// `bits_8to6` — scale every pixel from 8-bit `[0..256)` down to 6-bit `[0..64)` by `>> 2`.
///
/// Stock `imgutil.c` iterates `isize = iw * ih` bytes shifting each right by 2 (an integer divide by
/// 4). MINDTCT feeds this into the DFT / binarization front-end, which is built around 6-bit input
/// (`IMG_6BIT_PIX_LIMIT == 64`). Operates in place over the whole slice; the caller passes the full
/// `width * height` (already padded) buffer, so no dimensions are needed — the C `iw`/`ih` served
/// only to compute that product.
pub(crate) fn bits_8to6(idata: &mut [u8]) {
    for pix in idata.iter_mut() {
        // Divide every pixel value by 4 so that [0..256) -> [0..64).
        *pix >>= 2;
    }
}

/// `bits_6to8` — the inverse of [`bits_8to6`]: scale every pixel from 6-bit `[0..64)` back up to
/// 8-bit `[0..256)` by `<< 2` (a multiply by 4).
///
/// Stock `imgutil.c` shifts each byte left by 2. The low two bits discarded by [`bits_8to6`] are
/// gone, so the round trip is lossy (`255 -> 63 -> 252`), matching the reference. The `<< 2` on `u8`
/// truncates any high bits exactly like C's `unsigned char <<= 2`, so out-of-range inputs wrap
/// identically. MINDTCT uses this in `results.c` to restore the binarized image to 8-bit for output.
//
// `#[cfg(test)]`: the inverse of the wired [`bits_8to6`]. The port emits minutiae, not the 8-bit
// binarized image (`results.c`'s output stage), so nothing in the pipeline calls it; it is
// transcribed for fidelity and compiled only for the tests below that pin its arithmetic.
#[cfg(test)]
pub(crate) fn bits_6to8(idata: &mut [u8]) {
    for pix in idata.iter_mut() {
        // Multiply every pixel value by 4 so that [0..64) -> [0..256).
        *pix <<= 2;
    }
}

/// `pad_uchar_image` — copy an 8-bit grayscale image into a larger buffer, centered, with `pad`
/// pixels of constant `pad_value` intensity added on every side.
///
/// Returns the padded image plus its dimensions `(pdata, pw, ph)` where `pw = iw + 2*pad` and
/// `ph = ih + 2*pad`. Faithful to `imgutil.c` L189: allocate `pw * ph`, `memset` it all to
/// `pad_value`, then `memcpy` the input in one scanline at a time starting at offset
/// `pad * pw + pad`. The stock C returns an error code only on `malloc` failure; the Rust `Vec`
/// allocation replaces that path, so this is infallible for the sizes MINDTCT uses.
///
/// # Panics
/// If `idata.len() < iw * ih` (the input must hold at least a full `iw * ih` image).
pub(crate) fn pad_uchar_image(
    idata: &[u8],
    iw: usize,
    ih: usize,
    pad: usize,
    pad_value: u8,
) -> (Vec<u8>, usize, usize) {
    // Account for pad on both sides of the image.
    let pad2 = pad << 1;
    let pw = iw + pad2;
    let ph = ih + pad2;

    // Initialize the whole allocation to the constant PAD value.
    let mut pdata = vec![pad_value; pw * ph];

    // Copy the input image into the padded image one scanline at a time, centered: the first
    // destination row/col is `pad`, matching the stock `pdata + (pad * pw) + pad` start offset.
    for row in 0..ih {
        let src = &idata[row * iw..row * iw + iw];
        let dst_start = (pad + row) * pw + pad;
        pdata[dst_start..dst_start + iw].copy_from_slice(src);
    }

    (pdata, pw, ph)
}

/// Build the padded working buffer for a [`GrayImage`], mirroring the `lfs_detect_minutiae_V2`
/// front-end (`detect.c` L488): pad by `maxpad` with [`PAD_VALUE`] when `maxpad > 0`, otherwise take
/// a plain copy of the image at its original size.
///
/// Returns `(pdata, pw, ph)`. This is the seam the pipeline calls right before [`bits_8to6`]; the
/// `else` branch reproduces the stock "padding is unnecessary, so just copy" fast path exactly (same
/// dimensions, same bytes) so the two paths stay bit-identical to the reference.
pub(crate) fn pad_gray_image(img: &GrayImage<'_>, maxpad: usize) -> (Vec<u8>, usize, usize) {
    if maxpad > 0 {
        pad_uchar_image(
            img.data(),
            img.width(),
            img.height(),
            maxpad,
            PAD_VALUE as u8,
        )
    } else {
        // If padding is unnecessary, copy the input image unchanged.
        (img.data().to_vec(), img.width(), img.height())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- bits_8to6: >> 2 over [0..256) -> [0..64) ---------------------------------------------
    #[test]
    fn bits_8to6_shifts_right_two() {
        let mut v = [0u8, 3, 4, 5, 128, 254, 255];
        bits_8to6(&mut v);
        // 3>>2==0, 4>>2==1, 5>>2==1, 128>>2==32, 254>>2==63, 255>>2==63.
        assert_eq!(v, [0, 0, 1, 1, 32, 63, 63]);
    }

    #[test]
    fn bits_8to6_never_exceeds_6bit_limit() {
        // Every possible 8-bit input maps strictly below IMG_6BIT_PIX_LIMIT (64).
        for p in 0u8..=255 {
            let mut one = [p];
            bits_8to6(&mut one);
            assert!(one[0] < 64, "{p} >> 2 = {} should be < 64", one[0]);
        }
    }

    // --- bits_6to8: << 2 over [0..64) -> [0..256) ---------------------------------------------
    #[test]
    fn bits_6to8_shifts_left_two() {
        let mut v = [0u8, 1, 32, 63];
        bits_6to8(&mut v);
        // 1<<2==4, 32<<2==128, 63<<2==252.
        assert_eq!(v, [0, 4, 128, 252]);
    }

    #[test]
    fn bits_6to8_wraps_like_unsigned_char() {
        // Out-of-range (>= 64) input truncates its high bits exactly like C's `unsigned char <<= 2`.
        let mut v = [64u8, 127, 128, 255];
        bits_6to8(&mut v);
        // 64<<2 == 256 & 0xFF == 0; 127<<2 == 508 & 0xFF == 252;
        // 128<<2 == 512 & 0xFF == 0; 255<<2 == 1020 & 0xFF == 252.
        assert_eq!(v, [0, 252, 0, 252]);
    }

    // --- round trip through the 6-bit reduction is lossy in the low two bits ------------------
    #[test]
    fn round_trip_drops_low_two_bits() {
        let mut v = [255u8, 5, 7, 200];
        bits_8to6(&mut v);
        bits_6to8(&mut v);
        // Each value is floored to its multiple of 4: 255->252, 5->4, 7->4, 200->200.
        assert_eq!(v, [252, 4, 4, 200]);
    }

    // --- pad_uchar_image: centered copy inside a constant-fill border -------------------------
    #[test]
    fn pad_uchar_image_centers_and_fills() {
        // 2x2 image, pad 1 -> 4x4 with the image in the center and PAD_VALUE (128) elsewhere.
        let img = [1u8, 2, 3, 4];
        let (pdata, pw, ph) = pad_uchar_image(&img, 2, 2, 1, 128);
        assert_eq!((pw, ph), (4, 4));
        #[rustfmt::skip]
        let expected = [
            128, 128, 128, 128,
            128,   1,   2, 128,
            128,   3,   4, 128,
            128, 128, 128, 128,
        ];
        assert_eq!(pdata, expected);
    }

    #[test]
    fn pad_uchar_image_zero_pad_is_identity() {
        let img = [10u8, 20, 30, 40, 50, 60];
        let (pdata, pw, ph) = pad_uchar_image(&img, 3, 2, 0, 128);
        assert_eq!((pw, ph), (3, 2));
        assert_eq!(pdata, img);
    }

    #[test]
    fn pad_uchar_image_nonsquare_and_pad2() {
        // 3x1 image, pad 2 -> 7x5. Only row 2 (the centered scanline) holds the image.
        let img = [7u8, 8, 9];
        let (pdata, pw, ph) = pad_uchar_image(&img, 3, 1, 2, 128);
        assert_eq!((pw, ph), (7, 5));
        // The three input pixels land at offset pad*pw + pad = 2*7 + 2 = 16.
        assert_eq!(&pdata[16..19], &[7, 8, 9]);
        // Everything else is PAD_VALUE.
        assert!(pdata[..16].iter().all(|&b| b == 128));
        assert!(pdata[19..].iter().all(|&b| b == 128));
    }

    // --- pad_gray_image: front-end seam -------------------------------------------------------
    #[test]
    fn pad_gray_image_pads_with_pad_value() {
        let data = [1u8, 2, 3, 4];
        // 2x2 is below the detection floor; construct unchecked to test the padding seam in isolation.
        let img = GrayImage::from_parts_unchecked(&data, 2, 2, 500);
        let (pdata, pw, ph) = pad_gray_image(&img, 1);
        assert_eq!((pw, ph), (4, 4));
        // Center holds the image; border is PAD_VALUE (128).
        assert_eq!(&pdata[5..7], &[1, 2]);
        assert_eq!(&pdata[9..11], &[3, 4]);
        assert_eq!(pdata[0], PAD_VALUE as u8);
    }

    #[test]
    fn pad_gray_image_zero_maxpad_copies_unchanged() {
        let data = [11u8, 22, 33, 44, 55, 66];
        // 3x2 is below the detection floor; construct unchecked to test the copy path in isolation.
        let img = GrayImage::from_parts_unchecked(&data, 3, 2, 500);
        let (pdata, pw, ph) = pad_gray_image(&img, 0);
        assert_eq!((pw, ph), (3, 2));
        assert_eq!(pdata, data);
    }
}
