// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! # fprint-pipeline
//!
//! The host-image fingerprint pipeline. It joins the three published leaves —
//! [`fprint_mindtct`] (minutiae detection), [`fprint_bozorth3`] (minutiae matching) and
//! [`fprint_core`] (the domain [`Print`](fprint_core::Print) / [`Template`](fprint_core::Template) /
//! [`Minutia`](fprint_core::Minutia)) — into the one path a host-image sensor walks:
//! **image → minutiae → template → match**.
//!
//! The two kernels are dependency-free and each defines its own `Minutia` (the `xyt` triple is an
//! interoperability fact, not a shared type). This crate owns the small conversions between them
//! and the domain model:
//!
//! - [`extract_minutiae`] / [`template_from_images`] — the front half, over [`fprint_mindtct`].
//! - [`nbis_match_score`] (→ [`MatchScore`]) / [`nbis_verify`] / [`nbis_identify`] — the back half,
//!   over [`fprint_bozorth3`].
//! - [`minutia_to_core`] / [`minutiae_to_bozorth`] — the boundary conversions, exposed for callers
//!   that assemble their own pipeline.
//!
//! Add just this crate: it re-exports [`fprint_core`], [`fprint_mindtct`] and [`fprint_bozorth3`], so
//! the domain types and the kernels are reachable without naming them as separate dependencies.
//!
//! ## Image → template → match
//!
//! ```
//! use fprint_pipeline::{
//!     nbis_identify, nbis_match_score, nbis_verify, template_from_images, GrayImage,
//! };
//!
//! // A procedural fingerprint-like frame: dark horizontal ridges with a gap cut into every other
//! // ridge. A gap ends a ridge, and a ridge ending is a minutia (plain stripes would have none).
//! fn synthetic_frame() -> Vec<u8> {
//!     let (w, h) = (128usize, 128usize);
//!     (0..w * h)
//!         .map(|i| {
//!             let (x, y) = (i % w, i / w);
//!             let on_ridge = (y % 8) < 4;
//!             let gap = (48..80).contains(&x) && (y / 8) % 2 == 0;
//!             if on_ridge && !gap { 32 } else { 224 }
//!         })
//!         .collect()
//! }
//!
//! // Enroll one capture, then take a second capture of the same finger as the probe.
//! let frame = synthetic_frame();
//! let img = GrayImage::new(&frame, 128, 128, 500).expect("buffer holds the image");
//! let enrolled = template_from_images(&[img]);
//!
//! let frame2 = synthetic_frame();
//! let scan = GrayImage::new(&frame2, 128, 128, 500).expect("buffer holds the image");
//! let scanned = template_from_images(&[scan]);
//!
//! // Verify (1:1): a bool decision against a driver threshold, or read the raw score.
//! let threshold = 40;
//! let _accepted: bool = nbis_verify(&enrolled, &scanned, threshold);
//! let score: Option<u32> = nbis_match_score(&enrolled, &scanned).score();
//!
//! // A same-finger recapture out-scores an unrelated print; the caller picks the accept threshold.
//! let stranger = template_from_images(&[]);
//! assert!(score >= nbis_match_score(&enrolled, &stranger).score());
//!
//! // Identify (1:N): the gallery index whose enrolled template best matches the probe.
//! let gallery = vec![enrolled, stranger];
//! assert_eq!(nbis_identify(&scanned, &gallery, 0), Some(0));
//! ```
//!
//! ## Persistence
//!
//! This crate stops at the in-memory [`Template`](fprint_core::Template). To write an enrolled print
//! to disk in the format libfprint reads and writes, add `fprint-fp3` and call its `to_bytes` /
//! `from_bytes` on a [`Print`](fprint_core::Print) carrying the template — persistence is a
//! separate, single-purpose crate.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod detector;
mod matcher;

pub use detector::{extract_minutiae, minutia_to_core, template_from_images};
pub use matcher::{minutiae_to_bozorth, nbis_identify, nbis_match_score, nbis_verify, MatchScore};

// The input type a caller builds a frame with, its constructor error, and the detection floor that
// error names — pulled to the top so the common path needs to name only this crate.
pub use fprint_mindtct::{GrayImage, ImageError, MIN_DETECTABLE_DIM};

// The layers this crate joins, re-exported so `fprint_pipeline::fprint_core::Print` (and the two
// kernels) resolve without adding them as separate dependencies. The glue produces and consumes
// `fprint_core` types, so a caller reaches for them constantly.
pub use fprint_bozorth3;
pub use fprint_core;
pub use fprint_mindtct;

/// The handful of names the common path uses: the pipeline functions, the [`GrayImage`] input, and
/// the domain [`Print`](fprint_core::Print) / [`Template`](fprint_core::Template) /
/// [`Minutia`](fprint_core::Minutia) the functions produce and consume.
///
/// `use fprint_pipeline::prelude::*;` brings in enough to go from a frame to a match without naming
/// the leaf crates.
pub mod prelude {
    pub use crate::{
        extract_minutiae, nbis_identify, nbis_match_score, nbis_verify, template_from_images,
        GrayImage, MatchScore,
    };
    pub use fprint_core::{Finger, Minutia, Print, Template};
}
