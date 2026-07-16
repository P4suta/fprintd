// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Generators for the two interoperability facts this workspace's kernels consume.
//!
//! Both yield plain tuples and bytes, never a domain type. Each caller maps them onto its own
//! `Minutia` or image in a line, which is what lets this crate depend on nothing — see the crate
//! docs.

use crate::bytes::ByteSource;

/// The angle range an `xyt` triple's theta is canonically drawn from.
const THETA: (i32, i32) = (0, 359);

/// `n` minutiae as `(x, y, theta)` triples, scattered over a `width`×`height` field.
///
/// Coordinates are non-negative, which several callers need: `fprint-bozorth3`'s translation
/// invariance is exact only while the coordinate sums stay non-negative, because its cluster
/// centroid divides with truncation.
pub fn xyt(src: &mut impl ByteSource, n: usize, width: i32, height: i32) -> Vec<(i32, i32, i32)> {
    (0..n)
        .map(|_| {
            (
                src.in_range(0, width),
                src.in_range(0, height),
                src.in_range(THETA.0, THETA.1),
            )
        })
        .collect()
}

/// `base`, jittered: each triple moved by up to `radius` pixels and `radius` degrees.
///
/// A second impression of the same finger, as far as a matcher is concerned. Coordinates are
/// clamped at zero rather than allowed negative, keeping the same guarantee [`xyt`] gives.
pub fn xyt_jittered(
    src: &mut impl ByteSource,
    base: &[(i32, i32, i32)],
    radius: i32,
) -> Vec<(i32, i32, i32)> {
    base.iter()
        .map(|&(x, y, t)| {
            (
                (x + src.in_range(-radius, radius)).max(0),
                (y + src.in_range(-radius, radius)).max(0),
                (t + src.in_range(-radius, radius)).rem_euclid(360),
            )
        })
        .collect()
}

/// `width * height` bytes of 8-bit grayscale.
///
/// Noise, not a fingerprint: the callers that want ridge structure want a *specific* one and build
/// it themselves. This is for the questions noise can answer — that a detector terminates, stays in
/// bounds, and does not panic.
pub fn gray_image(src: &mut impl ByteSource, width: usize, height: usize) -> Vec<u8> {
    let mut buf = vec![0u8; width.saturating_mul(height)];
    src.fill(&mut buf);
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytes::Lcg;

    #[test]
    fn xyt_stays_inside_the_field_and_the_angle_range() {
        let mut lcg = Lcg::new(3);
        let m = xyt(&mut lcg, 200, 100, 80);
        assert_eq!(m.len(), 200);
        for (x, y, t) in m {
            assert!((0..=100).contains(&x), "x {x}");
            assert!((0..=80).contains(&y), "y {y}");
            assert!((THETA.0..=THETA.1).contains(&t), "theta {t}");
        }
    }

    #[test]
    fn xyt_is_reproducible_from_its_seed() {
        assert_eq!(
            xyt(&mut Lcg::new(5), 32, 200, 200),
            xyt(&mut Lcg::new(5), 32, 200, 200)
        );
    }

    #[test]
    fn xyt_jittered_stays_non_negative_and_in_angle_range() {
        let mut lcg = Lcg::new(8);
        // A base hard against the origin, so an unclamped jitter would go negative.
        let base = vec![(0, 0, 0); 64];
        for (x, y, t) in xyt_jittered(&mut lcg, &base, 5) {
            assert!(x >= 0 && y >= 0, "jitter went negative: ({x}, {y})");
            assert!((0..360).contains(&t), "theta {t}");
        }
    }

    #[test]
    fn gray_image_is_exactly_width_times_height() {
        let mut lcg = Lcg::new(2);
        assert_eq!(gray_image(&mut lcg, 16, 9).len(), 144);
        assert_eq!(gray_image(&mut lcg, 0, 9).len(), 0);
        assert_eq!(gray_image(&mut lcg, 16, 0).len(), 0);
    }
}
