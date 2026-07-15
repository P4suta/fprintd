// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [`SyntheticFrameSource`]: a deterministic, hardware-free [`FrameSource`].
//!
//! It renders a reproducible synthetic fingerprint once (a tiny LCG grating: parallel sinusoidal
//! ridges with scattered ridge-dislocation dipoles that plant real ridge-ending / bifurcation
//! minutiae — exactly the structure MINDTCT is built to find) and hands out that same frame on every
//! [`capture`](FrameSource::capture). Because the frame is byte-stable, a self-capture matches
//! strongly through the real detector + matcher, while a different preset (a different seed / ridge
//! field) is a genuine stranger. This is the [`crate::ImageDevice`] counterpart of the LCG grating
//! `tests/end_to_end.rs` exercises directly, promoted out of test code so a device can be driven with
//! no sensor.
//!
//! Scripted [`Capture::Retry`] outcomes ([`SyntheticFrameSource::with_retries`]) model weak captures
//! at chosen capture indices, so enrollment retry handling can be exercised deterministically. Every
//! `capture` awaits `crate::yield_now` once, keeping one poll boundary per stage — the strict
//! drop-cancellation point of [`crate::ImageDevice::enroll`].

use fprint_core::{Result, RetryReason};

use crate::frame::Frame;
use crate::frame_source::{Capture, FrameSource};

/// Scan resolution recorded in every synthetic frame (500 ppi, as in the oracle corpus).
const PPI: u16 = 500;

/// A deterministic capture source: one pre-rendered frame plus an optional retry script.
#[derive(Clone)]
pub struct SyntheticFrameSource {
    data: Vec<u8>,
    width: usize,
    height: usize,
    ppi: u16,
    /// Capture indices (0-based, counting every `capture` call) that yield a [`Capture::Retry`].
    retries: Vec<(usize, RetryReason)>,
    /// How many times `capture` has been called.
    count: usize,
}

impl SyntheticFrameSource {
    /// A healthy, minutiae-rich reference finger (self-captures match strongly).
    pub fn reference() -> Self {
        Self::from_grating(&reference_finger())
    }

    /// An unrelated finger: a different seed, spacing, and orientation (a true stranger).
    pub fn stranger() -> Self {
        Self::from_grating(&other_finger())
    }

    /// Script weak captures: each `(index, reason)` makes the `index`-th `capture` return
    /// [`Capture::Retry`] instead of a frame.
    #[must_use]
    pub fn with_retries(mut self, retries: Vec<(usize, RetryReason)>) -> Self {
        self.retries = retries;
        self
    }

    /// Render a grating once into a reusable source.
    fn from_grating(g: &Grating) -> Self {
        SyntheticFrameSource {
            data: grating(g),
            width: g.width,
            height: g.height,
            ppi: PPI,
            retries: Vec::new(),
            count: 0,
        }
    }
}

impl FrameSource for SyntheticFrameSource {
    async fn capture(&mut self) -> Result<Capture> {
        // One poll boundary per capture: the strict drop-cancel point for the enroll loop.
        crate::yield_now::yield_now().await;

        let idx = self.count;
        self.count += 1;
        if let Some(&(_, reason)) = self.retries.iter().find(|&&(i, _)| i == idx) {
            return Ok(Capture::Retry(reason));
        }

        Ok(Capture::Frame(Frame {
            data: self.data.clone(),
            width: self.width,
            height: self.height,
            ppi: self.ppi,
        }))
    }
}

// --- Deterministic synthetic-fingerprint generator (LCG grating) ---------------------------------
// A faithful port of the grating in `tests/end_to_end.rs` (itself a port of the oracle corpus
// generator): parallel sinusoidal ridges, gently curved, with scattered ridge-dislocation dipoles
// that plant ridge-ending / bifurcation minutiae. A tiny LCG (no RNG crate) keeps it byte-stable.

struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Lcg(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1))
    }
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
}

/// Round-to-nearest into the 8-bit range.
fn clamp8(v: f64) -> u8 {
    (v + 0.5).floor().clamp(0.0, 255.0) as u8
}

/// The parameters of one synthetic ridge field (see [`grating`]).
struct Grating {
    width: usize,
    height: usize,
    seed: u64,
    /// Ridge spacing in pixels.
    period: f64,
    /// Ridge-field orientation in degrees.
    angle_deg: f64,
    /// Quadratic bend of the ridge normal (0 = perfectly straight ridges).
    curve: f64,
    /// Additive noise half-range in gray levels.
    noise: i64,
    /// Number of ridge-dislocation dipoles (each plants a resolvable minutia).
    disloc: usize,
}

/// Pick `disloc` ridge-dislocation dipoles inside the image, positions drawn from the LCG. Each
/// dipole is a +1/-1 pair of phase singularities a ridge-period apart; overlaid on the grating it
/// inserts one extra half-ridge — a clean ridge ending / bifurcation.
fn dislocations(g: &Grating) -> Vec<(f64, f64, f64)> {
    let mut r = Lcg::new(g.seed ^ 0x0D15_104A);
    let margin = (2.0 * g.period) as usize + 10;
    let mut sings = Vec::new();
    if g.width <= 2 * margin || g.height <= 2 * margin {
        return sings;
    }
    let sep = (g.period.round() as usize).max(4);
    for _ in 0..g.disloc {
        let sx = margin + (r.next() as usize) % (g.width - 2 * margin);
        let sy = margin + (r.next() as usize) % (g.height - 2 * margin);
        let ox = sep + (r.next() as usize) % 3;
        let oy = (r.next() as i64) % 3 - 1;
        sings.push((sx as f64, sy as f64, 1.0));
        sings.push(((sx + ox) as f64, (sy as i64 + oy) as f64, -1.0));
    }
    sings
}

/// Render a [`Grating`] to a row-major 8-bit grayscale buffer.
fn grating(g: &Grating) -> Vec<u8> {
    const AMP: f64 = 95.0;
    const DC: f64 = 128.0;
    let mut r = Lcg::new(g.seed);
    let (cx, cy) = (g.width as f64 / 2.0, g.height as f64 / 2.0);
    let (ca, sa) = (
        g.angle_deg.to_radians().cos(),
        g.angle_deg.to_radians().sin(),
    );
    let kf = 2.0 * std::f64::consts::PI / g.period;
    let sings = if g.disloc > 0 {
        dislocations(g)
    } else {
        Vec::new()
    };
    let mut data = vec![0u8; g.width * g.height];
    for y in 0..g.height {
        for x in 0..g.width {
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            // Coordinate along the ridge normal, bent by the quadratic curvature term.
            let u = dx * ca + dy * sa;
            let v = -dx * sa + dy * ca;
            let mut phase = kf * (u + g.curve * v * v);
            for &(sx, sy, ch) in &sings {
                phase += ch * (y as f64 - sy).atan2(x as f64 - sx);
            }
            let mut val = DC + AMP * phase.cos();
            if g.noise != 0 {
                val += ((r.next() as i64) % (2 * g.noise + 1) - g.noise) as f64;
            }
            data[y * g.width + x] = clamp8(val);
        }
    }
    data
}

/// A healthy, minutiae-rich reference finger (sized to clear BOZORTH3's minutiae floor with margin).
fn reference_finger() -> Grating {
    Grating {
        width: 256,
        height: 256,
        seed: 0x1111,
        period: 9.0,
        angle_deg: 20.0,
        curve: 0.0018,
        noise: 18,
        disloc: 10,
    }
}

/// An unrelated finger: a different seed (different dislocations), spacing, and orientation.
fn other_finger() -> Grating {
    Grating {
        width: 256,
        height: 256,
        seed: 0x2222,
        period: 10.0,
        angle_deg: 55.0,
        curve: 0.0036,
        noise: 22,
        disloc: 14,
    }
}
