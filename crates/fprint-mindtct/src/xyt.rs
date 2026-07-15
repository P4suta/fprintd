// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Coordinate and angle conversion to NIST `xyt` output (`lfs2nist_minutia_XYT` and the M1 variant):
//! `y = ih - y`, `theta = (270 - sround(dir * degrees_per_unit)) % 360`. This is the one place the
//! port uses `f32` (`degrees_per_unit`), matching the stock reference exactly.
//!
//! Ports `xytreps.c` (`lfs2nist_minutia_XYT` L80, `lfs2m1_minutia_XYT` L122) and the output-side
//! quality/order contract of `results.c` (`write_minutiae_XYTQ` L268, `q = sround(reliability*100)`,
//! list order preserved). A [`DetMinutia`] carries LFS-native attributes — pixel coordinates with
//! origin top-left, an integer `direction` on `0..2*num_directions`, and a `reliability` in
//! `[0.0, 1.0]`; these routines convert one such point (or a whole list) into the crate's public
//! [`Minutia`] in the requested representation. The seam consumes the NIST form.

use crate::consts::NUM_DIRECTIONS;
use crate::detect::DetMinutia;
use crate::num::sround;
use crate::Minutia;

/// Degrees per quantized direction unit — stock `degrees_per_unit = 180 / (float)NUM_DIRECTIONS`
/// (`xytreps.c` L98/L138), evaluated in **single precision** to match the reference bit-for-bit.
///
/// This is the sole `f32` in the port. With `NUM_DIRECTIONS == 16` the value is exactly `11.25`,
/// which (like every `direction * 11.25` product over the valid `0..2*num_directions` range) is
/// representable in both `f32` and `f64`, so the single-precision evaluation never diverges from a
/// double-precision one here — the `f32` is kept purely for faithful transcription.
#[inline]
fn degrees_per_unit() -> f32 {
    180.0 / NUM_DIRECTIONS as f32
}

/// `sround(direction * degrees_per_unit)` — the rounded integer degrees for a quantized direction.
///
/// Faithful to the C evaluation order: `direction * degrees_per_unit` is computed in `f32` (the int
/// operand is promoted to the `float` `degrees_per_unit`), then the `sround` macro's `± 0.5` add and
/// `(int)` truncation happen after promotion to `f64` — exactly `sround(f64::from(prod))`.
#[inline]
fn direction_degrees(direction: i32) -> i32 {
    let prod: f32 = direction as f32 * degrees_per_unit();
    sround(f64::from(prod))
}

/// `q = sround(reliability * 100.0)` — the integer minutia quality on `[0..100]`
/// (`results.c` L321 / `xytreps.c` L174). `reliability` arithmetic is `f64`, as in stock.
#[inline]
fn quality(reliability: f64) -> i32 {
    sround(reliability * 100.0)
}

/// Convert one minutia to the **NIST internal** `xyt` representation — port of `lfs2nist_minutia_XYT`
/// (`xytreps.c` L80), with the quality added from `results.c` L321.
///
/// NIST internal rep: pixel origin **bottom-left** (`y = ih - y`), orientation in whole degrees on
/// `[0..360)` with 0 east and increasing counter-clockwise, pointing out and away from the ridge
/// ending / bifurcation valley. `iw` is unused by the stock routine (only `ih` flips the origin), so
/// it is omitted from the signature.
pub(crate) fn lfs2nist_minutia_xyt(minutia: &DetMinutia, ih: i32) -> Minutia {
    // PORT L95-L96: x unchanged; y flipped to a bottom-left origin.
    let x = minutia.x;
    let y = ih - minutia.y;

    // PORT L100-L103: (270 - degrees) mod 360, folded back onto [0..360).
    let mut t = (270 - direction_degrees(minutia.direction)) % 360;
    if t < 0 {
        t += 360;
    }

    Minutia {
        x,
        y,
        theta: t,
        quality: quality(minutia.reliability),
    }
}

/// Convert one minutia to the **M1 (ANSI INCITS 378-2004)** `xyt` representation — port of
/// `lfs2m1_minutia_XYT` (`xytreps.c` L122), with the quality added from `results.c` L321.
///
/// M1 rep: pixel origin **top-left** (`x`, `y` unchanged), direction pointing *up* the ridge ending /
/// bifurcation valley (opposite the NIST form), and orientation quantized in units of 2° so the final
/// angle is halved onto `[0..179]`.
///
/// `dead_code`: the NIST-internal form ([`lfs2nist_format`]) is the seam's only consumed
/// representation. M1 (ANSI INCITS 378-2004) is an external-interchange format, and the crate exposes
/// no public export API, so nothing outside `#[cfg(test)]` reaches these converters — the test suite
/// pins their arithmetic. Paired with the allow on [`lfs2m1_format`].
#[allow(dead_code)]
pub(crate) fn lfs2m1_minutia_xyt(minutia: &DetMinutia) -> Minutia {
    // PORT L135-L136: coordinates unchanged (top-left origin).
    let x = minutia.x;
    let y = minutia.y;

    // PORT L139-L142: (90 - degrees) mod 360, folded back onto [0..360).
    let mut t = (90 - direction_degrees(minutia.direction)) % 360;
    if t < 0 {
        t += 360;
    }
    // PORT L145: angles are in units of 2 degrees, so range is 0..179.
    t /= 2;

    Minutia {
        x,
        y,
        theta: t,
        quality: quality(minutia.reliability),
    }
}

/// Convert a whole minutiae list to the NIST internal `xyt` representation, list order preserved —
/// the `write_minutiae_XYTQ(NIST_INTERNAL_XYT_REP)` contract (`results.c` L268, L304-L324). This is
/// the form the native seam consumes.
pub(crate) fn lfs2nist_format(minutiae: &[DetMinutia], ih: i32) -> Vec<Minutia> {
    minutiae
        .iter()
        .map(|m| lfs2nist_minutia_xyt(m, ih))
        .collect()
}

/// Convert a whole minutiae list to the M1 `xyt` representation, list order preserved — the
/// `write_minutiae_XYTQ(M1_XYT_REP)` contract (`results.c` L268, L304-L324).
///
/// `dead_code`: see [`lfs2m1_minutia_xyt`] — the M1 batch converter has no lib caller; the
/// `#[cfg(test)]` suite pins its arithmetic.
#[allow(dead_code)]
pub(crate) fn lfs2m1_format(minutiae: &[DetMinutia]) -> Vec<Minutia> {
    minutiae.iter().map(lfs2m1_minutia_xyt).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `DetMinutia` carrying only the fields the `xyt` stage reads; the rest are inert.
    fn det(x: i32, y: i32, direction: i32, reliability: f64) -> DetMinutia {
        DetMinutia {
            x,
            y,
            ex: 0,
            ey: 0,
            direction,
            reliability,
            kind: 0,
            appearing: true,
            feature_id: 0,
            nbrs: Vec::new(),
            ridge_counts: Vec::new(),
        }
    }

    /// `degrees_per_unit` is exactly `11.25` at `NUM_DIRECTIONS == 16`, in `f32`.
    #[test]
    fn degrees_per_unit_is_11_25_f32() {
        assert_eq!(degrees_per_unit(), 11.25_f32);
    }

    /// `direction * 11.25` is exact for the whole valid `0..2*num_directions` range, so the `f32`
    /// evaluation equals a straight `f64` computation — no rounding surprises before `sround`.
    #[test]
    fn direction_degrees_matches_exact_scaling() {
        for direction in 0..2 * NUM_DIRECTIONS {
            let expect = sround(f64::from(direction) * 11.25);
            assert_eq!(direction_degrees(direction), expect, "dir={direction}");
        }
        // Spot values: 8 -> 90, 12 -> 135, 24 -> 270, 25 -> sround(281.25) == 281.
        assert_eq!(direction_degrees(8), 90);
        assert_eq!(direction_degrees(12), 135);
        assert_eq!(direction_degrees(24), 270);
        assert_eq!(direction_degrees(25), 281);
    }

    /// NIST form: `x` unchanged, `y` flipped by `ih`, `theta = (270 - degrees) mod 360`.
    #[test]
    fn nist_coordinate_and_angle() {
        // direction 0 -> degrees 0 -> theta 270; y flips about ih=100.
        let m = lfs2nist_minutia_xyt(&det(30, 40, 0, 1.0), 100);
        assert_eq!((m.x, m.y, m.theta), (30, 60, 270));

        // direction 8 -> degrees 90 -> theta 180.
        assert_eq!(lfs2nist_minutia_xyt(&det(0, 0, 8, 1.0), 100).theta, 180);

        // direction 12 -> degrees 135 -> theta 135.
        assert_eq!(lfs2nist_minutia_xyt(&det(0, 0, 12, 1.0), 100).theta, 135);
    }

    /// NIST form: an angle whose raw `270 - degrees` goes negative must wrap by `+360`.
    #[test]
    fn nist_angle_wraps_positive() {
        // direction 25 -> degrees 281 -> 270-281 = -11 -> +360 = 349.
        assert_eq!(lfs2nist_minutia_xyt(&det(0, 0, 25, 1.0), 100).theta, 349);
        // direction 24 -> degrees 270 -> theta 0 (boundary, no wrap).
        assert_eq!(lfs2nist_minutia_xyt(&det(0, 0, 24, 1.0), 100).theta, 0);
    }

    /// M1 form: coordinates unchanged (top-left origin), `theta = ((90 - degrees) mod 360) / 2`.
    #[test]
    fn m1_coordinate_and_angle() {
        // direction 0 -> degrees 0 -> (90) / 2 = 45; coordinates pass through.
        let m = lfs2m1_minutia_xyt(&det(30, 40, 0, 1.0));
        assert_eq!((m.x, m.y, m.theta), (30, 40, 45));

        // direction 8 -> degrees 90 -> (0) / 2 = 0.
        assert_eq!(lfs2m1_minutia_xyt(&det(0, 0, 8, 1.0)).theta, 0);

        // direction 24 -> degrees 270 -> 90-270 = -180 -> +360 = 180 -> /2 = 90.
        assert_eq!(lfs2m1_minutia_xyt(&det(0, 0, 24, 1.0)).theta, 90);
    }

    /// Quality is `sround(reliability * 100)` — half rounds away from zero.
    #[test]
    fn quality_rounds_reliability() {
        assert_eq!(lfs2nist_minutia_xyt(&det(0, 0, 0, 0.99), 100).quality, 99);
        assert_eq!(lfs2nist_minutia_xyt(&det(0, 0, 0, 0.995), 100).quality, 100);
        assert_eq!(lfs2nist_minutia_xyt(&det(0, 0, 0, 0.0), 100).quality, 0);
        assert_eq!(lfs2m1_minutia_xyt(&det(0, 0, 0, 1.0)).quality, 100);
    }

    /// Both batch converters preserve list order and match the per-minutia routines.
    #[test]
    fn format_preserves_order() {
        let list = [det(1, 2, 0, 1.0), det(3, 4, 8, 0.5), det(5, 6, 12, 0.25)];

        let nist = lfs2nist_format(&list, 100);
        assert_eq!(nist.len(), 3);
        for (i, m) in list.iter().enumerate() {
            assert_eq!(nist[i], lfs2nist_minutia_xyt(m, 100));
        }

        let m1 = lfs2m1_format(&list);
        assert_eq!(m1.len(), 3);
        for (i, m) in list.iter().enumerate() {
            assert_eq!(m1[i], lfs2m1_minutia_xyt(m));
        }
    }
}
