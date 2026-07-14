// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The real NBIS-template matcher a host-image driver uses — the seam onto [`fp_bozorth3`].
//!
//! This is where `fp_core`'s domain model meets the public-domain matcher: it converts an
//! [`fp_core::Template::Nbis`] (host-side minutiae, one inner vector per enrolled capture) into
//! `fp_bozorth3::Minutia` and scores a probe against an enrolled template, taking the **maximum**
//! score over enrolled samples — libfprint/NBIS's verify semantics.
//!
//! The [`VirtualDevice`](crate::VirtualDevice) itself keeps its deterministic, non-biometric
//! byte-equality stub (see [`crate::synth`]); a genuine USB image driver, when it lands, matches
//! through *this* function. The conversion lives here (not in `fp-bozorth3`) so the matcher stays a
//! self-contained, dependency-free arithmetic kernel — the xyt triple is the only shared fact.

use fp_core::Template;

/// Score a `scanned` probe template against an `enrolled` one, both `Template::Nbis`.
///
/// Returns the BOZORTH3 match score — the maximum over every (enrolled-sample, scanned-sample) pair
/// (each capture is an independent minutiae set). Returns `0` unless **both** templates are `Nbis`
/// (a match-on-chip `Raw` handle is opaque and never host-matched). The caller compares the score
/// against a driver threshold to decide match / non-match.
#[must_use]
pub fn nbis_match_score(enrolled: &Template, scanned: &Template) -> u32 {
    let (Template::Nbis(enrolled_samples), Template::Nbis(scanned_samples)) = (enrolled, scanned)
    else {
        return 0;
    };

    let mut best = 0;
    for probe in scanned_samples {
        let probe: Vec<fp_bozorth3::Minutia> = probe.iter().map(to_bz).collect();
        for gallery in enrolled_samples {
            let gallery: Vec<fp_bozorth3::Minutia> = gallery.iter().map(to_bz).collect();
            best = best.max(fp_bozorth3::match_score(&probe, &gallery));
        }
    }
    best
}

/// Identify (1:N): the `gallery` index whose enrolled template best matches `scanned` and clears
/// `threshold`, or `None` if none does.
///
/// This is the host-image `identify` semantics: score the probe against each enrolled template
/// ([`nbis_match_score`]) and take the strongest above the driver threshold. Ties resolve to the
/// **lowest** index (first-best), so the result is deterministic.
#[must_use]
pub fn nbis_identify(scanned: &Template, gallery: &[Template], threshold: u32) -> Option<usize> {
    let mut best: Option<(usize, u32)> = None;
    for (i, enrolled) in gallery.iter().enumerate() {
        let score = nbis_match_score(enrolled, scanned);
        if score >= threshold && best.is_none_or(|(_, b)| score > b) {
            best = Some((i, score));
        }
    }
    best.map(|(i, _)| i)
}

/// Convert one domain minutia to the matcher's xyt triple (an interoperability fact, not coupling).
#[inline]
fn to_bz(m: &fp_core::Minutia) -> fp_bozorth3::Minutia {
    fp_bozorth3::Minutia {
        x: m.x,
        y: m.y,
        theta: m.theta,
    }
}
