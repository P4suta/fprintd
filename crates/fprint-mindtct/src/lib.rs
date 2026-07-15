// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! # fprint-mindtct
//!
//! A pure-Rust, dependency-free reimplementation of **MINDTCT** — NIST NBIS's minutiae detector.
//! Given an 8-bit grayscale fingerprint image it produces a list of [`Minutia`] (`x`, `y`, `theta`,
//! `quality`) reproducing the stock NBIS tool's `xyt` output, plus (via [`debug_maps`]) the
//! intermediate block maps and binarized image used to reach them.
//!
//! ## Provenance
//!
//! MINDTCT is public-domain U.S. Government software (title 17 §105). This crate is a **faithful
//! port** of the **stock upstream NBIS** algorithm (`reference/nbis-stock/mindtct/`, see
//! `docs/mindtct-algorithm.md`), verified black-box against the stock C tool — reproducing its xyt
//! output bit-for-bit required following its arithmetic *and its ordering* closely, which is
//! deliberate and which public domain permits. It is deliberately **not** derived from libfprint's
//! patched `nbis/mindtct/` copy, whose changes carry LGPL terms.
//!
//! The crate carries `MIT OR Apache-2.0` like the rest of the project: public domain grants without
//! demanding, so it constrains neither the port nor the licence we put on it, and there is nothing
//! to quarantine against. The NBIS lineage is provenance, not a licence. See `ARCHITECTURE.md`
//! §Provenance & licensing.
//!
//! ## Shape
//!
//! The crate takes its own [`GrayImage`] and returns its own [`Minutia`] — the `xyt` triple is an
//! interoperability fact, so the detector stays a self-contained image-processing kernel with no
//! dependency on the domain model. A consumer (e.g. `fprint-backend-native`) converts to its
//! `fprint_core::Minutia` at the boundary.

#![forbid(unsafe_code)]

mod binarize;
mod block;
mod consts;
mod detect;
mod image;
mod init;
mod maps;
mod num;
mod params;
mod quality;
mod remove;
mod ridges;
mod util;
mod xyt;

/// One detected minutia in NIST `xyt` form.
///
/// Coordinates use the NIST internal convention: origin **bottom-left**, `x` rightward, `y` upward.
/// `theta` is the ridge orientation in integer **degrees** on `0..=359`, 0 pointing east and
/// increasing counter-clockwise (the `lfs2nist` representation). `quality` is a reliability estimate
/// (higher is better). Mirrors the shape of `fprint_core::Minutia` (the consumer converts).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Minutia {
    pub x: i32,
    pub y: i32,
    /// Ridge orientation in degrees, `0..=359`.
    pub theta: i32,
    /// Reliability estimate; higher is better.
    pub quality: i32,
}

/// An 8-bit grayscale fingerprint image, row-major, one byte per pixel.
///
/// `data` is exactly `width * height` bytes (0 = black, 255 = white). `ppi` is the scan resolution
/// in pixels-per-inch, carried because several MINDTCT thresholds are resolution-relative.
#[derive(Clone, Copy, Debug)]
pub struct GrayImage<'a> {
    pub data: &'a [u8],
    pub width: usize,
    pub height: usize,
    /// Scan resolution in pixels-per-inch.
    pub ppi: u16,
}

/// The intermediate block maps and binarized image produced along the way to the minutiae.
///
/// Exposed through [`debug_maps`] for cross-implementation verification against the stock C tool's
/// map dumps. All four block maps are `map_w * map_h` in row-major block order; `binarized` is a
/// full-resolution `width * height` image (0 = ridge, 255 = valley — the pre-`gray2bin` form).
///
/// This struct is intentionally minimal for now and will grow as the pipeline is filled in.
#[derive(Clone, Debug, Default)]
pub struct DebugMaps {
    /// Per-block ridge flow direction: `-1` (invalid) or a valid direction index.
    pub direction_map: Vec<i32>,
    /// Per-block low-contrast flag (`TRUE`/`FALSE` as `1`/`0`).
    pub low_contrast_map: Vec<i32>,
    /// Per-block low-ridge-flow flag (`TRUE`/`FALSE` as `1`/`0`).
    pub low_flow_map: Vec<i32>,
    /// Per-block high-curvature flag (`TRUE`/`FALSE` as `1`/`0`).
    pub high_curve_map: Vec<i32>,
    /// Width of the block maps, in blocks.
    pub map_w: usize,
    /// Height of the block maps, in blocks.
    pub map_h: usize,
    /// Binarized image, full resolution, `0` = ridge and `255` = valley — the output of the
    /// directional-binarization stage (`binarize_V2`), i.e. **before** the detection/false-minutia
    /// removal stages edit the binary image in place. (The stock tool's `.brw` dump is captured
    /// after removal, so the two agree only where removal is a no-op.)
    pub binarized: Vec<u8>,
}

/// The shared detection front-end carried between the pipeline stages: the block maps and their
/// dimensions ([`DebugMaps`]), the in-place-edited binary image in the detector's `0 == valley` /
/// `1 == ridge` convention, and the current minutiae list. Produced by [`run_detect`] and threaded
/// through false-minutia removal, ridge counting, and quality assignment.
struct DetectState {
    maps: DebugMaps,
    /// Binary image, `iw`×`ih`, stock `gray2bin(1, 1, 0)` convention (`0 == valley`, `1 == ridge`).
    bdata: Vec<u8>,
    minutiae: Vec<crate::detect::DetMinutia>,
}

/// Run the stock `lfs_detect_minutiae_V2` front-end through `detect_minutiae_V2` — the shared prefix
/// of [`detect_minutiae`], [`debug_raw_minutiae`], and [`debug_removed_minutiae`].
///
/// Reuses [`debug_maps`] for the pad → 6-bit scale → block maps → directional binarization stages,
/// converts the binary image to the detector's `0 == valley` / `1 == ridge` convention
/// (stock `gray2bin(1, 1, 0)`, `detect.c` L614), then runs `detect_minutiae_V2` over the unpadded
/// image and block maps. Returns [`None`] on the (size) error paths the pipeline can surface — an
/// empty front-end map or a `detect_minutiae_V2` error.
fn run_detect(img: GrayImage<'_>) -> Option<DetectState> {
    use crate::detect::detect_minutiae_v2;
    use crate::params::LFSPARMS_V2;

    // Verified front-end: block maps + the pre-gray2bin binary image.
    let maps = debug_maps(img);
    if maps.binarized.is_empty() {
        return None;
    }

    // stock `gray2bin(1, 1, 0)`: ridge (0) → 1, valley (255) → 0.
    let mut bdata: Vec<u8> = maps.binarized.iter().map(|&p| u8::from(p < 1)).collect();

    let p = &LFSPARMS_V2;
    let minutiae = detect_minutiae_v2(
        &mut bdata,
        img.width as i32,
        img.height as i32,
        &maps.direction_map,
        &maps.low_flow_map,
        &maps.high_curve_map,
        maps.map_w as i32,
        maps.map_h as i32,
        p,
    )
    .ok()?;

    Some(DetectState {
        maps,
        bdata,
        minutiae,
    })
}

/// Project a detector minutiae list onto the diagnostic [`RawMinutia`] rows, list order preserved.
fn to_raw(minutiae: &[crate::detect::DetMinutia]) -> Vec<RawMinutia> {
    minutiae
        .iter()
        .map(|m| RawMinutia {
            x: m.x,
            y: m.y,
            direction: m.direction,
            kind: m.kind,
            appearing: i32::from(m.appearing),
        })
        .collect()
}

/// Detect the minutiae in a grayscale fingerprint image.
///
/// Runs the stock `get_minutiae` / `lfs_detect_minutiae_V2` pipeline (pad → 6-bit scale → block maps
/// → binarize → detect → false-minutia removal → ridge counts → integrated quality → `xyt` output)
/// and returns the minutiae in NIST internal `xyt` form (origin bottom-left, `theta` in whole
/// degrees on `0..=359`, `quality` on `0..=100`). See `docs/mindtct-algorithm.md`.
///
/// The reliability that becomes each minutia's `quality` is derived from the **original** 8-bit image
/// (`img.data`, unpadded) and the block maps at the scan resolution `ppmm = img.ppi / 25.4`, exactly
/// as stock `combined_minutia_quality` consumes `idata`. Returns an empty list on the (size) error
/// paths the pipeline can surface.
#[must_use]
pub fn detect_minutiae(img: GrayImage<'_>) -> Vec<Minutia> {
    use crate::params::LFSPARMS_V2;
    use crate::quality::{combined_minutia_quality, gen_quality_map};
    use crate::remove::remove_false_minutia_v2;
    use crate::ridges::count_minutiae_ridges;
    use crate::xyt::lfs2nist_format;

    // Front-end + detection (detect.c L455–633).
    let Some(mut st) = run_detect(img) else {
        return Vec::new();
    };

    let p = &LFSPARMS_V2;
    let (iw, ih) = (img.width as i32, img.height as i32);
    let (mw, mh) = (st.maps.map_w as i32, st.maps.map_h as i32);

    // False-minutia removal (detect.c L639): edits the minutiae list and the binary image in place.
    if remove_false_minutia_v2(
        &mut st.minutiae,
        &mut st.bdata,
        iw,
        ih,
        &st.maps.direction_map,
        &st.maps.low_flow_map,
        &st.maps.high_curve_map,
        mw,
        mh,
        p,
    )
    .is_err()
    {
        return Vec::new();
    }

    // Neighbor ridge counts (detect.c L662): sorts + dedups the list — fixing the final order and
    // count — over the `0 == valley` / `1 == ridge` binary image left by removal.
    if count_minutiae_ridges(&mut st.minutiae, &st.bdata, iw, ih, p).is_err() {
        return Vec::new();
    }

    // Integrated quality map (getmin.c: gen_quality_map), then per-minutia reliability blended from
    // the ORIGINAL 8-bit unpadded image at the scan resolution (combined_minutia_quality).
    let quality_map = gen_quality_map(
        &st.maps.direction_map,
        &st.maps.low_contrast_map,
        &st.maps.low_flow_map,
        &st.maps.high_curve_map,
        mw,
        mh,
    );
    let ppmm = f64::from(img.ppi) / 25.4;
    if combined_minutia_quality(
        &mut st.minutiae,
        &quality_map,
        mw,
        mh,
        p.blocksize,
        img.data,
        iw,
        ih,
        ppmm,
    )
    .is_err()
    {
        return Vec::new();
    }

    // Final `xyt` conversion, list order preserved (results.c write_minutiae_XYTQ).
    lfs2nist_format(&st.minutiae, ih)
}

/// One raw minutia exactly as `detect_minutiae_V2` emits it — the diagnostic row of a `.rmin` dump.
///
/// Captured **before** false-minutia removal, list order preserved. All fields are in the LFS-internal
/// representation: `x`/`y` are pixel coordinates with origin top-left; `direction` is the integer
/// direction on `0..2*num_directions` (`0..=31`); `kind` is the stock `type` (`0 == BIFURCATION`,
/// `1 == RIDGE_ENDING`); `appearing` is `1` (appearing) or `0` (disappearing). Mirrors the oracle's
/// `.rmin` line `"x y direction type appearing"`.
#[doc(hidden)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RawMinutia {
    pub x: i32,
    pub y: i32,
    pub direction: i32,
    /// Stock `type`: `0 == BIFURCATION`, `1 == RIDGE_ENDING`.
    pub kind: i32,
    /// `1 == APPEARING`, `0 == DISAPPEARING`.
    pub appearing: i32,
}

/// Diagnostic (hidden): the raw minutiae for an input exactly as `detect_minutiae_V2` produces them,
/// **before** false-minutia removal and with list order preserved — the golden contract behind the
/// oracle's `.rmin` dump.
///
/// Runs the same front-end as [`debug_maps`] (pad → 6-bit scale → block maps → directional
/// binarization), then converts the binary image to the detector's `0 == valley` / `1 == ridge`
/// convention (stock `gray2bin(1, 1, 0)`, `detect.c` L614) and runs `detect_minutiae_V2` over the
/// unpadded image and block maps. Returns each detected point as a [`RawMinutia`]; returns an empty
/// list on the (size) error paths the pipeline can surface.
#[doc(hidden)]
#[must_use]
pub fn debug_raw_minutiae(img: GrayImage<'_>) -> Vec<RawMinutia> {
    // Shared front-end + `detect_minutiae_V2`; the raw list is captured before removal.
    match run_detect(img) {
        Some(st) => to_raw(&st.minutiae),
        None => Vec::new(),
    }
}

/// Diagnostic (hidden): the minutiae for an input **after** `remove_false_minutia_V2`, list order
/// preserved — the golden contract behind the oracle's `.rmin2` dump.
///
/// Runs the same front-end and `detect_minutiae_V2` as [`debug_raw_minutiae`], then applies the ten
/// false-minutia removal stages (`remove_false_minutia_V2`) over the same in-place-edited binary image
/// and the **block** maps (not pixelized), before the ridge-count stage. Returns each surviving point
/// as a [`RawMinutia`] in the same `(x, y, direction, type, appearing)` contract as `.rmin`; returns
/// an empty list on the (size) error paths the pipeline can surface.
#[doc(hidden)]
#[must_use]
pub fn debug_removed_minutiae(img: GrayImage<'_>) -> Vec<RawMinutia> {
    use crate::params::LFSPARMS_V2;
    use crate::remove::remove_false_minutia_v2;

    // Shared front-end + `detect_minutiae_V2`.
    let Some(mut st) = run_detect(img) else {
        return Vec::new();
    };

    let p = &LFSPARMS_V2;
    let (iw, ih) = (img.width as i32, img.height as i32);
    let (mw, mh) = (st.maps.map_w as i32, st.maps.map_h as i32);

    // The ten false-minutia removal stages, over the same in-place-edited binary and block maps.
    if remove_false_minutia_v2(
        &mut st.minutiae,
        &mut st.bdata,
        iw,
        ih,
        &st.maps.direction_map,
        &st.maps.low_flow_map,
        &st.maps.high_curve_map,
        mw,
        mh,
        p,
    )
    .is_err()
    {
        return Vec::new();
    }

    to_raw(&st.minutiae)
}

/// Diagnostic (hidden): the intermediate maps and binarized image for an input, used by verification
/// tooling to localize any divergence from the stock C reference before the minutiae stage.
///
/// Reproduces the `lfs_detect_minutiae_V2` front-end up to and including the binarization stage
/// (`detect.c` L455–L582): build the V2 lookup tables, pad the image by the max padding, scale it
/// to 6 bits, run the block-map pipeline (`gen_image_maps`), then directionally binarize the padded
/// image against the direction map (`binarize_V2`). Fills the four block maps, their dimensions, and
/// the full-resolution `binarized` image (`0` = ridge / `255` = valley, the pre-`gray2bin` form).
/// Returns empty maps on the (size) error paths `gen_image_maps` can surface.
#[doc(hidden)]
#[must_use]
pub fn debug_maps(img: GrayImage<'_>) -> DebugMaps {
    use crate::binarize::binarize_v2;
    use crate::consts::DFT_COEFS;
    use crate::image::{bits_8to6, pad_gray_image};
    use crate::init::{get_max_padding_V2, init_dftwaves, init_dir2rad, init_rotgrids, Relative2};
    use crate::maps::gen_image_maps;
    use crate::params::LFSPARMS_V2;

    let p = &LFSPARMS_V2;

    // Max padding required to support the rotated DFT windows / dirbin grids.
    let maxpad = get_max_padding_V2(
        p.windowsize,
        p.windowoffset,
        p.dirbin_grid_w,
        p.dirbin_grid_h,
    );

    // V2 lookup tables (direction->radians, DFT waves, rotated DFT grids).
    let dir2rad = init_dir2rad(p.num_directions);
    let dftwaves = init_dftwaves(&DFT_COEFS, p.num_dft_waves, p.windowsize);
    let dftgrids = init_rotgrids(
        img.width as i32,
        Some(maxpad),
        p.start_dir_angle,
        p.num_directions,
        p.windowsize,
        p.windowsize,
        Relative2::Origin,
    );

    // Pad, then scale to 6 bits [0..63].
    let (mut pdata, pw, ph) = pad_gray_image(&img, maxpad as usize);
    bits_8to6(&mut pdata);

    match gen_image_maps(
        &pdata, pw as i32, ph as i32, &dir2rad, &dftwaves, &dftgrids, p,
    ) {
        Ok(m) => {
            // Binarization stage (detect.c L557): the dir-bin rotated grids share the single
            // `maxpad` so the one padded image serves both the DFT and dir-bin passes. Then
            // `binarize_V2` produces the (unpadded) black-ridge/white-valley image.
            let dirbingrids = init_rotgrids(
                img.width as i32,
                Some(maxpad),
                p.start_dir_angle,
                p.num_directions,
                p.dirbin_grid_w,
                p.dirbin_grid_h,
                Relative2::Center,
            );
            let bin = binarize_v2(
                &pdata,
                pw as i32,
                ph as i32,
                &m.direction_map,
                m.map_w,
                &dirbingrids,
                p,
            );
            DebugMaps {
                direction_map: m.direction_map,
                low_contrast_map: m.low_contrast_map,
                low_flow_map: m.low_flow_map,
                high_curve_map: m.high_curve_map,
                map_w: m.map_w as usize,
                map_h: m.map_h as usize,
                binarized: bin.data,
            }
        }
        Err(_) => DebugMaps::default(),
    }
}
