// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The NBIS-template matcher seam — the back half of the pipeline, onto [`fprint_bozorth3`].
//!
//! It converts an [`fprint_core::Template::Nbis`] (host-side minutiae, one inner vector per
//! enrolled capture) into `fprint_bozorth3::Minutia` and scores a probe against an enrolled
//! template, taking the **maximum** score over enrolled samples — libfprint/NBIS's verify
//! semantics.
//!
//! The conversion lives here rather than in `fprint-bozorth3` so the matcher stays a self-contained,
//! dependency-free arithmetic kernel — the `xyt` triple is the only shared fact. A match-on-chip
//! `Raw` handle is opaque and never host-matched.

use fprint_core::Template;

/// The outcome of scoring one template against another.
///
/// [`nbis_match_score`] returns this rather than a bare `u32` so a caller cannot mistake "cannot be
/// host-matched" for "scored zero". The two are genuinely different: [`Scored(0)`](MatchScore::Scored)
/// is a real comparison that found nothing in common, while
/// [`Incomparable`](MatchScore::Incomparable) means there were no host-side minutiae to compare at all
/// (a match-on-chip handle, or an un-enrolled template).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MatchScore {
    /// Both templates carried host-side NBIS minutiae and were scored by BOZORTH3. The value is the
    /// raw score; `0` is a valid outcome — a comparison that matched nothing.
    Scored(u32),
    /// The pair cannot be host-matched: at least one template is a match-on-chip `Raw` handle or an
    /// `Undefined` template, so there are no host minutiae for BOZORTH3 to compare.
    Incomparable,
}

impl MatchScore {
    /// The BOZORTH3 score if the pair was comparable, else `None`.
    #[must_use]
    pub fn score(self) -> Option<u32> {
        match self {
            MatchScore::Scored(score) => Some(score),
            MatchScore::Incomparable => None,
        }
    }

    /// Whether this outcome clears `threshold`: a [`Scored(s)`](MatchScore::Scored) with
    /// `s >= threshold`. An [`Incomparable`](MatchScore::Incomparable) pair never accepts.
    #[must_use]
    pub fn accepts(self, threshold: u32) -> bool {
        matches!(self, MatchScore::Scored(score) if score >= threshold)
    }
}

/// Score a `scanned` probe template against an `enrolled` one.
///
/// When **both** are [`Template::Nbis`], returns [`MatchScore::Scored`] with the BOZORTH3 match score
/// — the maximum over every (enrolled-sample, scanned-sample) pair, since each capture is an
/// independent minutiae set (libfprint/NBIS verify semantics). When either is a match-on-chip
/// [`Template::Raw`] handle or [`Template::Undefined`], there are no host minutiae to compare and the
/// result is [`MatchScore::Incomparable`]. Turn a score into a decision with
/// [`accepts`](MatchScore::accepts) against a driver threshold (or use [`nbis_verify`]).
#[must_use]
pub fn nbis_match_score(enrolled: &Template, scanned: &Template) -> MatchScore {
    let (Template::Nbis(enrolled_samples), Template::Nbis(scanned_samples)) = (enrolled, scanned)
    else {
        return MatchScore::Incomparable;
    };

    let mut best = 0;
    for probe in scanned_samples {
        let probe: Vec<fprint_bozorth3::Minutia> = probe.iter().map(to_bz).collect();
        for gallery in enrolled_samples {
            let gallery: Vec<fprint_bozorth3::Minutia> = gallery.iter().map(to_bz).collect();
            best = best.max(fprint_bozorth3::match_score(&probe, &gallery));
        }
    }
    MatchScore::Scored(best)
}

/// Verify (1:1): whether `scanned` matches `enrolled` at or above `threshold`.
///
/// The one-line decision form of [`nbis_match_score`] for the common verify path, mirroring
/// [`nbis_identify`]'s 1:N shape. An [`Incomparable`](MatchScore::Incomparable) pair never verifies.
#[must_use]
pub fn nbis_verify(enrolled: &Template, scanned: &Template, threshold: u32) -> bool {
    nbis_match_score(enrolled, scanned).accepts(threshold)
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
        let Some(score) = nbis_match_score(enrolled, scanned).score() else {
            continue; // an incomparable pair is never a candidate
        };
        if score >= threshold && best.is_none_or(|(_, b)| score > b) {
            best = Some((i, score));
        }
    }
    best.map(|(i, _)| i)
}

/// Convert a slice of domain [`fprint_core::Minutia`] into `fprint_bozorth3::Minutia`s ready to score.
///
/// One enrolled or scanned capture is one such slice; the result is the input BOZORTH3's
/// [`match_score`](fprint_bozorth3::match_score) takes. Only the `xyt` triple crosses the boundary —
/// the same interoperability fact [`nbis_match_score`] applies internally per sample.
#[must_use]
pub fn minutiae_to_bozorth(ms: &[fprint_core::Minutia]) -> Vec<fprint_bozorth3::Minutia> {
    ms.iter().map(to_bz).collect()
}

/// Convert one domain minutia to the matcher's xyt triple (an interoperability fact, not coupling).
#[inline]
fn to_bz(m: &fprint_core::Minutia) -> fprint_bozorth3::Minutia {
    fprint_bozorth3::Minutia {
        x: m.x,
        y: m.y,
        theta: m.theta,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fprint_core::Minutia;

    fn nbis(pts: Vec<Minutia>) -> Template {
        Template::Nbis(vec![pts])
    }

    #[test]
    fn a_raw_or_undefined_template_is_incomparable() {
        let host = nbis(vec![Minutia {
            x: 1,
            y: 2,
            theta: 3,
        }]);

        assert_eq!(
            nbis_match_score(&Template::Undefined, &host),
            MatchScore::Incomparable
        );
        assert_eq!(
            nbis_match_score(&host, &Template::Undefined),
            MatchScore::Incomparable
        );
        assert_eq!(
            nbis_match_score(&Template::Raw(vec![0xAB]), &host),
            MatchScore::Incomparable
        );

        // Incomparable never yields a score and never verifies, at any threshold.
        assert_eq!(nbis_match_score(&Template::Undefined, &host).score(), None);
        assert!(!nbis_verify(&Template::Raw(vec![0xAB]), &host, 0));
    }

    #[test]
    fn two_nbis_templates_are_scored_even_when_empty() {
        // Two empty Nbis templates are comparable: the score is a real `0`, not "incomparable".
        let empty = Template::Nbis(vec![]);
        assert_eq!(nbis_match_score(&empty, &empty), MatchScore::Scored(0));
        assert_eq!(nbis_match_score(&empty, &empty).score(), Some(0));
    }

    #[test]
    fn accepts_needs_a_score_at_or_over_threshold() {
        assert!(MatchScore::Scored(50).accepts(40));
        assert!(MatchScore::Scored(40).accepts(40));
        assert!(!MatchScore::Scored(39).accepts(40));
        assert!(!MatchScore::Incomparable.accepts(0));
    }
}
