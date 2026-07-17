// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The minutiae detector seam — the front half of the pipeline, onto [`fprint_mindtct`].
//!
//! It turns a captured 8-bit grayscale frame into an [`fprint_core::Template::Nbis`] by calling
//! [`fprint_mindtct::detect_minutiae`] and converting each public-domain
//! [`fprint_mindtct::Minutia`] into an [`fprint_core::Minutia`], so a caller can go
//! **image → minutiae → match** end-to-end (the back half is [`crate::matcher`]).
//!
//! The conversion lives here rather than in `fprint-mindtct`: that crate is a self-contained,
//! dependency-free public-domain kernel that does not know `fprint-core`, and the only thing
//! crossing the boundary is the `xyt` triple — an interoperability *fact*, not a code coupling.
//! Keeping the `Minutia → fprint_core::Minutia` mapping in this pipeline crate lets the PD detector
//! and PD matcher each define their own `Minutia`.

use fprint_core::{Minutia, Template};

/// Detect the minutiae in a captured 8-bit grayscale frame, as [`fprint_core::Minutia`]s.
///
/// Runs the full MINDTCT pipeline ([`fprint_mindtct::detect_minutiae`]) over `img` and projects each
/// detected point onto the domain minutia. The `xyt` triple (`x`, `y`, `theta`) is carried through
/// verbatim — the same interoperability fact the matcher consumes — and MINDTCT's per-point
/// `quality` is dropped, because `fprint_core::Minutia` (like libfprint's `xyt_struct` for BOZORTH3)
/// does not carry it. An image with no detectable ridge structure yields an empty vector.
#[must_use]
pub fn extract_minutiae(img: fprint_mindtct::GrayImage<'_>) -> Vec<Minutia> {
    fprint_mindtct::detect_minutiae(img)
        .iter()
        .map(minutia_to_core)
        .collect()
}

/// Build a host-side [`fprint_core::Template::Nbis`] from a set of captured frames.
///
/// Each frame is detected independently ([`extract_minutiae`]) and becomes one inner minutiae vector
/// — the `Vec<Vec<Minutia>>` shape an image-capture sensor enrolls, one sample per capture. This is
/// always an `Nbis` template (even for a single frame, and even if a frame detects zero minutiae:
/// that empty sample is preserved so the sample count mirrors the captures); the matcher takes the
/// maximum score over the samples, so more captures only ever help.
#[must_use]
pub fn template_from_images(images: &[fprint_mindtct::GrayImage<'_>]) -> Template {
    Template::Nbis(images.iter().map(|&img| extract_minutiae(img)).collect())
}

/// Convert one MINDTCT [`fprint_mindtct::Minutia`] into an [`fprint_core::Minutia`].
///
/// The `xyt` triple (`x`, `y`, `theta`) is carried through verbatim — the interoperability fact both
/// the detector and the matcher share — and MINDTCT's per-point `quality` is dropped, because
/// `fprint_core::Minutia` (like libfprint's `xyt_struct` for BOZORTH3) does not carry it. This is the
/// public form of the `mindtct → core` seam that [`extract_minutiae`] applies per detected point.
#[inline]
#[must_use]
pub fn minutia_to_core(m: &fprint_mindtct::Minutia) -> Minutia {
    Minutia {
        x: m.x,
        y: m.y,
        theta: m.theta,
    }
}
