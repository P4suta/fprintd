// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! # fprint-mindtct
//!
//! A pure-Rust, dependency-free reimplementation of **MINDTCT** — NIST NBIS's minutiae detector.
//! Given an 8-bit grayscale fingerprint image it produces a list of [`Minutia`] (`x`, `y`, `theta`,
//! `quality`) reproducing the stock NBIS tool's `xyt` output. The `unstable-diagnostics` feature
//! additionally exposes the intermediate block maps and binarized image used to reach them.
//!
//! ## Provenance
//!
//! MINDTCT is public-domain U.S. Government software (title 17 §105). This crate is a **faithful
//! port** of the **stock upstream NBIS** algorithm (`reference/nbis-stock/mindtct/`, see
//! `docs/mindtct-algorithm.md`), verified black-box against the stock C tool — reproducing its xyt
//! output bit-for-bit requires following its arithmetic *and its ordering* closely, which public
//! domain permits. It is deliberately **not** derived from libfprint's patched `nbis/mindtct/` copy,
//! whose changes carry LGPL terms.
//!
//! The crate carries `MIT OR Apache-2.0` like the rest of the project: public domain imposes no
//! conditions, so it constrains neither the port nor the licence we put on it. The NBIS lineage is
//! provenance, not a licence. See `ARCHITECTURE.md` §Provenance & licensing.
//!
//! ## Shape
//!
//! The crate takes its own [`GrayImage`] and returns its own [`Minutia`] — the `xyt` triple is an
//! interoperability fact, so the detector stays a self-contained image-processing kernel with no
//! dependency on the domain model. A consumer (e.g. `fprint-backend-native`) converts to its
//! `fprint_core::Minutia` at the boundary.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

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
    /// Pixel column, origin bottom-left, increasing rightward.
    pub x: i32,
    /// Pixel row, origin bottom-left, increasing upward.
    pub y: i32,
    /// Ridge orientation in degrees, `0..=359`.
    pub theta: i32,
    /// Reliability estimate; higher is better.
    pub quality: i32,
}

impl Minutia {
    /// The `(x, y, theta)` triple, dropping `quality` — the interoperability fact a matcher
    /// (BOZORTH3) or the domain model names. There is no `from_xyt`: a bare triple cannot say how
    /// reliable the detection was, and this detector is the thing that decides `quality`.
    #[must_use]
    pub const fn as_xyt(&self) -> (i32, i32, i32) {
        (self.x, self.y, self.theta)
    }
}

/// The smallest image dimension MINDTCT can process, in pixels.
///
/// Below this in either axis there is nowhere to place the `windowsize × windowsize` block-map window
/// clear of the padding (the constraint is `dimension ≥ windowsize + 1`), nor room for a single
/// `blocksize × blocksize` block — so detection has no defined output. Derived from the shipping
/// `LFSPARMS_V2` (`blocksize = 8`, `windowsize = 24`), it is the tightest bound of the two.
/// [`GrayImage::new`] rejects any image below it, which is what lets [`detect_minutiae`] be total.
pub const MIN_DETECTABLE_DIM: usize = {
    let blocksize = crate::params::LFSPARMS_V2.blocksize;
    let window = crate::params::LFSPARMS_V2.windowsize + 1;
    (if blocksize > window {
        blocksize
    } else {
        window
    }) as usize
};

/// An 8-bit grayscale fingerprint image, row-major, one byte per pixel.
///
/// `data` holds at least `width * height` bytes (0 = black, 255 = white). `ppi` is the scan
/// resolution in pixels-per-inch, carried because several MINDTCT thresholds are resolution-relative.
///
/// Build one with [`GrayImage::new`], which rejects an image MINDTCT cannot process — too small to
/// carry a block-map window ([`MIN_DETECTABLE_DIM`]), too large for its `i32` pixel arithmetic, or a
/// buffer shorter than `width * height`. The fields are private, so a value can only exist around a
/// detectable image; a *longer* `data` is accepted and its trailing bytes are ignored.
#[derive(Clone, Copy)]
pub struct GrayImage<'a> {
    /// The pixels, row-major, one byte each (0 = black, 255 = white).
    data: &'a [u8],
    /// Image width in pixels.
    width: usize,
    /// Image height in pixels.
    height: usize,
    /// Scan resolution in pixels-per-inch.
    ppi: u16,
}

impl core::fmt::Debug for GrayImage<'_> {
    /// Print geometry, not the pixel buffer.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "GrayImage {{ {}x{} @ {}ppi }}",
            self.width, self.height, self.ppi
        )
    }
}

impl<'a> GrayImage<'a> {
    /// Wrap a row-major 8-bit grayscale buffer, checking it can hold the stated image.
    ///
    /// `data` must be at least `width * height` bytes; a longer buffer is accepted and its trailing
    /// bytes are ignored. `ppi` is the scan resolution in pixels-per-inch.
    ///
    /// # Errors
    ///
    /// - [`ImageError::TooLarge`] when either dimension exceeds `i32::MAX`, the range MINDTCT's
    ///   internal pixel arithmetic addresses.
    /// - [`ImageError::TooSmall`] when either dimension is below [`MIN_DETECTABLE_DIM`]: MINDTCT
    ///   cannot place a block-map window on it, so detection has no defined output.
    /// - [`ImageError::BufferTooShort`] when `data.len() < width * height`.
    ///
    /// A successfully constructed `GrayImage` is therefore guaranteed detectable: an empty
    /// [`detect_minutiae`] result then means "no minutiae", never "image rejected".
    pub fn new(
        data: &'a [u8],
        width: usize,
        height: usize,
        ppi: u16,
    ) -> Result<GrayImage<'a>, ImageError> {
        if width > i32::MAX as usize || height > i32::MAX as usize {
            return Err(ImageError::TooLarge { width, height });
        }
        if width
            .checked_mul(height)
            .is_none_or(|area| area > i32::MAX as usize)
        {
            return Err(ImageError::TooLarge { width, height });
        }
        if width < MIN_DETECTABLE_DIM || height < MIN_DETECTABLE_DIM {
            return Err(ImageError::TooSmall {
                width,
                height,
                min: MIN_DETECTABLE_DIM,
            });
        }
        if data.len() < width.saturating_mul(height) {
            return Err(ImageError::BufferTooShort {
                width,
                height,
                got: data.len(),
            });
        }
        Ok(GrayImage {
            data,
            width,
            height,
            ppi,
        })
    }

    /// Construct without the detectability checks `new` enforces, for crate-internal tests that
    /// exercise sub-detection geometry (e.g. the padding seam) on images below [`MIN_DETECTABLE_DIM`].
    ///
    /// Kept `pub(crate)` and test-only on purpose: the invariant `new` upholds — a `GrayImage` a
    /// caller holds is always detectable — must not be circumventable from outside the crate.
    #[cfg(test)]
    pub(crate) fn from_parts_unchecked(
        data: &'a [u8],
        width: usize,
        height: usize,
        ppi: u16,
    ) -> GrayImage<'a> {
        GrayImage {
            data,
            width,
            height,
            ppi,
        }
    }

    /// Image width in pixels.
    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }

    /// Image height in pixels.
    #[must_use]
    pub fn height(&self) -> usize {
        self.height
    }

    /// Scan resolution in pixels-per-inch.
    #[must_use]
    pub fn ppi(&self) -> u16 {
        self.ppi
    }

    /// The pixel buffer, row-major, one byte per pixel.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        self.data
    }
}

/// The error [`GrayImage::new`] returns when the supplied image cannot be processed by MINDTCT.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ImageError {
    /// The pixel buffer is shorter than `width * height`.
    BufferTooShort {
        /// Stated image width in pixels.
        width: usize,
        /// Stated image height in pixels.
        height: usize,
        /// Length of the buffer supplied.
        got: usize,
    },
    /// A dimension is below [`MIN_DETECTABLE_DIM`]: there is nowhere to place the block-map window,
    /// so detection has no defined output.
    TooSmall {
        /// Stated image width in pixels.
        width: usize,
        /// Stated image height in pixels.
        height: usize,
        /// The smallest dimension MINDTCT can process ([`MIN_DETECTABLE_DIM`]).
        min: usize,
    },
    /// A dimension, or the product `width * height`, exceeds `i32::MAX` — the range MINDTCT's
    /// internal pixel arithmetic addresses.
    TooLarge {
        /// Stated image width in pixels.
        width: usize,
        /// Stated image height in pixels.
        height: usize,
    },
}

impl core::fmt::Display for ImageError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            ImageError::BufferTooShort { width, height, got } => write!(
                f,
                "grayscale buffer holds {got} bytes, need {} for a {width}x{height} image",
                width.saturating_mul(height)
            ),
            ImageError::TooSmall { width, height, min } => write!(
                f,
                "image {width}x{height} is too small to detect minutiae: each dimension must be at least {min} pixels"
            ),
            ImageError::TooLarge { width, height } => write!(
                f,
                "image {width}x{height} is too large: each dimension must be at most {}",
                i32::MAX
            ),
        }
    }
}

impl std::error::Error for ImageError {}

// `DebugMaps` is the front-end's map container, used by the core pipeline on every run. Its public
// diagnostic identity is gated: `unstable-diagnostics` exports it, otherwise it stays crate-internal.
// The single definition lives in `diag` so the two visibilities share one struct.
#[cfg(feature = "unstable-diagnostics")]
#[doc(hidden)]
pub use diag::DebugMaps;
#[cfg(not(feature = "unstable-diagnostics"))]
pub(crate) use diag::DebugMaps;

mod diag {
    /// The intermediate block maps and binarized image produced along the way to the minutiae.
    ///
    /// Exposed through [`debug_maps`](crate::debug_maps) for cross-implementation verification against
    /// the stock C tool's map dumps. All four block maps are `map_w * map_h` in row-major block order;
    /// `binarized` is a full-resolution `width * height` image (0 = ridge, 255 = valley — the
    /// pre-`gray2bin` form).
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
/// of [`detect_minutiae`] and the diagnostic entry points.
///
/// Reuses [`build_maps`] for the pad → 6-bit scale → block maps → directional binarization stages,
/// converts the binary image to the detector's `0 == valley` / `1 == ridge` convention
/// (stock `gray2bin(1, 1, 0)`, `detect.c` L614), then runs `detect_minutiae_V2` over the unpadded
/// image and block maps. Returns [`None`] on the internal size/error paths — an empty front-end map
/// or a `detect_minutiae_V2` error. Those paths are unreachable for a value produced by
/// [`GrayImage::new`], which rejects any image below [`MIN_DETECTABLE_DIM`]; the [`None`] arm is kept
/// as a defensive net, not a reachable outcome.
fn run_detect(img: GrayImage<'_>) -> Option<DetectState> {
    use crate::detect::detect_minutiae_v2;
    use crate::params::LFSPARMS_V2;

    // Verified front-end: block maps + the pre-gray2bin binary image.
    let maps = build_maps(img);
    if maps.binarized.is_empty() {
        return None;
    }

    // stock `gray2bin(1, 1, 0)`: ridge (0) → 1, valley (255) → 0.
    let mut bdata: Vec<u8> = maps.binarized.iter().map(|&p| u8::from(p < 1)).collect();

    let p = &LFSPARMS_V2;
    // `width()`/`height() as i32` here and throughout this module are lossless: `GrayImage::new`
    // rejects any dimension above `i32::MAX`, so the value always fits the detector's i32 arithmetic.
    let minutiae = detect_minutiae_v2(
        &mut bdata,
        img.width() as i32,
        img.height() as i32,
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
#[cfg(feature = "unstable-diagnostics")]
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
/// (the unpadded pixels) and the block maps at the scan resolution `ppmm = ppi / 25.4`, exactly as
/// stock `combined_minutia_quality` consumes `idata`. Returns an empty list when the image has no
/// detectable ridge structure — that is the *only* meaning of an empty result. An image too small to
/// carry a block-map window is rejected earlier, by [`GrayImage::new`] ([`MIN_DETECTABLE_DIM`]), so it
/// never reaches this function; a realistic fingerprint is far above that floor.
///
/// # Examples
///
/// A procedural image: dark horizontal ridges on a light field, with a gap cut into every other
/// ridge. The gap ends a ridge, and a ridge ending is a minutia — plain stripes have none, and would
/// make the loop below assert nothing.
///
/// The example asserts the shape of the answer, not the count: that detection finds *something*, and
/// that each minutia sits inside the image with a documented angle and quality. The exact count is a
/// property of the pixels and belongs in the golden suite.
///
/// ```
/// use fprint_mindtct::{detect_minutiae, GrayImage};
///
/// let (width, height) = (128, 128);
/// let data: Vec<u8> = (0..width * height)
///     .map(|i| {
///         let (x, y) = (i % width, i / width);
///         let on_ridge = (y % 8) < 4;
///         let gap = (48..80).contains(&x) && (y / 8) % 2 == 0;
///         if on_ridge && !gap { 32 } else { 224 }
///     })
///     .collect();
///
/// let img = GrayImage::new(&data, width, height, 500).expect("buffer holds the image");
/// let minutiae = detect_minutiae(img);
///
/// assert!(!minutiae.is_empty(), "the ridge gaps must yield minutiae");
/// for m in &minutiae {
///     assert!((0..width as i32).contains(&m.x));
///     assert!((0..height as i32).contains(&m.y));
///     assert!((0..360).contains(&m.theta));
///     assert!((0..=100).contains(&m.quality));
/// }
/// ```
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
    let (iw, ih) = (img.width() as i32, img.height() as i32);
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
    let ppmm = f64::from(img.ppi()) / 25.4;
    if combined_minutia_quality(
        &mut st.minutiae,
        &quality_map,
        mw,
        mh,
        p.blocksize,
        img.data(),
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
#[cfg(feature = "unstable-diagnostics")]
#[doc(hidden)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RawMinutia {
    /// Pixel x, origin top-left.
    pub x: i32,
    /// Pixel y, origin top-left.
    pub y: i32,
    /// Integer direction on `0..=31` (`0..2*num_directions`).
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
#[cfg(feature = "unstable-diagnostics")]
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
#[cfg(feature = "unstable-diagnostics")]
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
    let (iw, ih) = (img.width() as i32, img.height() as i32);
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

/// Diagnostic (hidden): how many minutiae each numbered stage of `remove_false_minutia_V2` dropped
/// for an input, indexed by the reference's own stage number minus one (slot `0` is stage 1, slot
/// `9` is stage 10). Returns all zeros on the (size) error paths the pipeline can surface.
///
/// Runs the same front-end and `detect_minutiae_V2` as [`debug_removed_minutiae`], then the ten
/// removal stages, measuring the list length across each. Exposed so a test can ask which stages the
/// corpus actually exercises — a stage that never drops a minutia is untested by the `.rmin2` golden
/// however green it reads.
///
/// Slot `0` is the sort, which permutes the list and so is always `0`. Slot `5` (stage 6,
/// `remove_or_adjust_side_minutiae_V2`) both removes and *adjusts*; an adjust leaves the length
/// alone, so the slot counts its remove path only.
#[cfg(feature = "unstable-diagnostics")]
#[doc(hidden)]
#[must_use]
pub fn debug_removal_tally(img: GrayImage<'_>) -> [usize; 10] {
    use crate::params::LFSPARMS_V2;
    use crate::remove::remove_false_minutia_v2;

    let Some(mut st) = run_detect(img) else {
        return [0; 10];
    };

    let p = &LFSPARMS_V2;
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    let (mw, mh) = (st.maps.map_w as i32, st.maps.map_h as i32);

    remove_false_minutia_v2(
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
    .unwrap_or([0; 10])
}

/// Diagnostic (hidden): the intermediate maps and binarized image for an input, used by verification
/// tooling to localize any divergence from the stock C reference before the minutiae stage.
///
/// A thin wrapper over [`build_maps`], exposed only under `unstable-diagnostics`. Returns empty maps
/// on the (size) error paths `gen_image_maps` can surface.
#[cfg(feature = "unstable-diagnostics")]
#[doc(hidden)]
#[must_use]
pub fn debug_maps(img: GrayImage<'_>) -> DebugMaps {
    build_maps(img)
}

/// The `lfs_detect_minutiae_V2` front-end up to and including the binarization stage
/// (`detect.c` L455–L582): build the V2 lookup tables, pad the image by the max padding, scale it to
/// 6 bits, run the block-map pipeline (`gen_image_maps`), then directionally binarize the padded image
/// against the direction map (`binarize_V2`). Fills the four block maps, their dimensions, and the
/// full-resolution `binarized` image (`0` = ridge / `255` = valley, the pre-`gray2bin` form). Returns
/// empty maps on the (size) error paths `gen_image_maps` can surface. Shared by [`run_detect`] and, as
/// the public [`debug_maps`], by the diagnostic surface.
fn build_maps(img: GrayImage<'_>) -> DebugMaps {
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
        img.width() as i32,
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
                img.width() as i32,
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
