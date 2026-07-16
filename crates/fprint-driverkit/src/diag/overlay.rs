// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [`render_overlay`]: draw detected minutiae onto the frame the way a driver author reads them.
//!
//! The result is what "the developer sees": the grayscale capture in color, each minutia a marker
//! at its `(x, y)`, a short tick along its `theta`, tinted on a fixed reliability ramp so a glance
//! separates strong detections from weak. The function is pure — the same frame and minutiae always
//! render the same pixels — which is what lets a golden freeze it.
//!
//! MINDTCT's coordinates are NIST-internal: origin **bottom-left**, `y` upward, `theta` measured
//! counter-clockwise from east. The image crate's origin is top-left with `y` downward, so both the
//! point and the tick are flipped vertically as they are drawn.

use image::{Rgb, RgbImage};
use imageproc::drawing::{draw_filled_circle_mut, draw_hollow_circle_mut, draw_line_segment_mut};

use fprint_backend_native::Frame;
use fprint_mindtct::Minutia;

/// Knobs for [`render_overlay`].
#[derive(Clone, Copy, Debug)]
pub struct OverlayOptions {
    /// Radius of the hollow ring drawn around each minutia, in pixels.
    pub marker_radius: i32,
    /// Length of the orientation tick drawn from each minutia, in pixels.
    pub tick_len: f64,
    /// Whether to draw the orientation tick at all.
    pub draw_orientation: bool,
}

impl Default for OverlayOptions {
    /// A ring wide enough to read over ridge detail, with an orientation tick.
    fn default() -> Self {
        Self {
            marker_radius: 3,
            tick_len: 8.0,
            draw_orientation: true,
        }
    }
}

/// Render `minutiae` onto `frame` and return the color overlay.
///
/// The grayscale frame becomes the RGB background; each minutia is a filled dot and a hollow ring at
/// its point, plus an optional tick along `theta`, all tinted by [`reliability_color`]. The public
/// [`Minutia`] carries no ridge-ending / bifurcation kind, so every marker is the same glyph; the
/// reliability tint is what distinguishes them.
#[must_use]
pub fn render_overlay(frame: &Frame, minutiae: &[Minutia], opts: &OverlayOptions) -> RgbImage {
    let (w, h) = (frame.width, frame.height);
    let mut canvas = RgbImage::new(w as u32, h as u32);
    for (x, y, pixel) in canvas.enumerate_pixels_mut() {
        let g = frame.data[y as usize * w + x as usize];
        *pixel = Rgb([g, g, g]);
    }

    let flip_y = h as i32 - 1;
    for m in minutiae {
        let color = reliability_color(m.quality);
        let px = m.x;
        let py = flip_y - m.y;

        if opts.draw_orientation {
            let rad = f64::from(m.theta).to_radians();
            let ex = px + (opts.tick_len * rad.cos()).round() as i32;
            // `y` is flipped, so a counter-clockwise angle ticks upward in image space.
            let ey = py - (opts.tick_len * rad.sin()).round() as i32;
            draw_line_segment_mut(
                &mut canvas,
                (px as f32, py as f32),
                (ex as f32, ey as f32),
                color,
            );
        }

        draw_filled_circle_mut(&mut canvas, (px, py), 1, color);
        draw_hollow_circle_mut(&mut canvas, (px, py), opts.marker_radius, color);
    }
    canvas
}

/// The reliability ramp: `quality` 0..=100 maps red (weak) to green (strong).
///
/// `quality` is clamped into range first, so an out-of-range value pins to an endpoint rather than
/// wrapping the channel arithmetic.
#[must_use]
pub fn reliability_color(quality: i32) -> Rgb<u8> {
    let t = f64::from(quality.clamp(0, 100)) / 100.0;
    let r = ((1.0 - t) * 255.0).round() as u8;
    let g = (t * 255.0).round() as u8;
    Rgb([r, g, 0])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: usize, height: usize) -> Frame {
        Frame {
            data: vec![100u8; width * height],
            width,
            height,
            ppi: 500,
        }
    }

    #[test]
    fn overlay_matches_frame_dimensions() {
        let f = frame(40, 30);
        let img = render_overlay(&f, &[], &OverlayOptions::default());
        assert_eq!(img.dimensions(), (40, 30));
    }

    #[test]
    fn empty_minutiae_leave_the_gray_background() {
        let f = frame(8, 8);
        let img = render_overlay(&f, &[], &OverlayOptions::default());
        assert!(img.pixels().all(|p| *p == Rgb([100, 100, 100])));
    }

    #[test]
    fn reliability_ramp_spans_red_to_green() {
        assert_eq!(reliability_color(0), Rgb([255, 0, 0]));
        assert_eq!(reliability_color(100), Rgb([0, 255, 0]));
        assert_eq!(reliability_color(150), Rgb([0, 255, 0]));
        assert_eq!(reliability_color(-5), Rgb([255, 0, 0]));
    }

    #[test]
    fn a_minutia_paints_over_the_background() {
        let f = frame(32, 32);
        let m = [Minutia {
            x: 16,
            y: 16,
            theta: 0,
            quality: 100,
        }];
        let img = render_overlay(&f, &m, &OverlayOptions::default());
        assert!(
            img.pixels().any(|p| *p != Rgb([100, 100, 100])),
            "the marker must change some pixels"
        );
    }
}
