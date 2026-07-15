// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! # fprint-bozorth3
//!
//! A pure-Rust, dependency-free reimplementation of **BOZORTH3** — NIST NBIS's rotation- and
//! translation-invariant minutiae fingerprint matcher. Given two prints as lists of minutiae, it
//! returns an integer match score reproducing the stock NBIS tool's output; higher means more
//! corresponding ridge structure.
//!
//! ## Provenance
//!
//! BOZORTH3 is public-domain U.S. Government software (title 17 §105). This crate is a **faithful
//! port** of the **stock upstream NBIS** algorithm (see `docs/bozorth3-algorithm.md`), verified
//! black-box against the stock C tool — score-exactness requires following its arithmetic closely,
//! which public domain permits. It is deliberately **not** derived from libfprint's patched `nbis/`
//! copy, whose changes carry LGPL terms.
//!
//! The crate carries `MIT OR Apache-2.0` like the rest of the project: public domain imposes no
//! conditions, so it constrains neither the port nor the licence we put on it. The NBIS lineage is
//! provenance, not a licence. See `ARCHITECTURE.md` §Provenance & licensing.
//!
//! ## Shape
//!
//! The crate takes its own [`Minutia`] (`{ x, y, theta }`) — the `xyt` triple is an interoperability
//! fact, so the matcher stays a self-contained arithmetic kernel with no dependency on the domain
//! model. A consumer (e.g. `fprint-backend-native`) converts its `fprint_core::Minutia` at the boundary.
//!
//! ```
//! use fprint_bozorth3::Minutia;
//! let probe = [Minutia { x: 10, y: 20, theta: 90 } /* … */];
//! let gallery = [Minutia { x: 11, y: 19, theta: 92 } /* … */];
//! let score = fprint_bozorth3::match_score(&probe, &gallery);
//! // Prints with < 10 minutiae are "not computable" and score 0.
//! assert_eq!(score, 0);
//! ```

#![forbid(unsafe_code)]

mod cluster;
mod consts;
mod inter;
mod intra;
mod xyt;

pub use consts::{DEFAULT_BOZORTH_MINUTIAE, MAX_BOZORTH_MINUTIAE, MIN_COMPUTABLE_BOZORTH_MINUTIAE};

/// One detected minutia: pixel position and ridge direction.
///
/// `theta` is the ridge angle in integer **degrees**; any integer is accepted and folded into
/// `0..=359` internally. Matches the shape of `fprint_core::Minutia` (the consumer converts).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Minutia {
    pub x: i32,
    pub y: i32,
    /// Ridge orientation in degrees.
    pub theta: i32,
}

/// The BOZORTH3 match score between a `probe` and a `gallery` minutia set.
///
/// Runs the full stock pipeline: build each print's intra-comparison Web (capped at
/// [`DEFAULT_BOZORTH_MINUTIAE`] minutiae, pruned by edge length), form the inter-print compatibility
/// list, then cluster it. Returns `0` when either print has fewer than
/// [`MIN_COMPUTABLE_BOZORTH_MINUTIAE`] minutiae, and `4000` (`QQ_OVERFLOW_SCORE`) if the internal
/// work queue overflows on a pathological input. The caller decides a match/non-match threshold.
#[must_use]
pub fn match_score(probe: &[Minutia], gallery: &[Minutia]) -> u32 {
    let p = xyt::prepare(probe, DEFAULT_BOZORTH_MINUTIAE);
    let g = xyt::prepare(gallery, DEFAULT_BOZORTH_MINUTIAE);
    if p.nrows < MIN_COMPUTABLE_BOZORTH_MINUTIAE || g.nrows < MIN_COMPUTABLE_BOZORTH_MINUTIAE {
        return 0;
    }

    let p_comp = intra::comp(&p);
    let g_comp = intra::comp(&g);
    let p_len = intra::prune_len(&p_comp);
    let g_len = intra::prune_len(&g_comp);

    let colp = inter::bz_match(&p_comp[..p_len], &g_comp[..g_len]);
    let score = cluster::match_score(&colp, &p, &g);
    score.max(0) as u32
}

/// Diagnostic (hidden): the pipeline sizes `(probe_web_len, gallery_web_len, num_compat_edges)`
/// for a pair — used by verification tooling to localize any divergence from the C reference.
#[doc(hidden)]
#[must_use]
pub fn debug_pipeline(probe: &[Minutia], gallery: &[Minutia]) -> (usize, usize, usize) {
    let p = xyt::prepare(probe, DEFAULT_BOZORTH_MINUTIAE);
    let g = xyt::prepare(gallery, DEFAULT_BOZORTH_MINUTIAE);
    if p.nrows < MIN_COMPUTABLE_BOZORTH_MINUTIAE || g.nrows < MIN_COMPUTABLE_BOZORTH_MINUTIAE {
        return (0, 0, 0);
    }
    let p_comp = intra::comp(&p);
    let g_comp = intra::comp(&g);
    let p_len = intra::prune_len(&p_comp);
    let g_len = intra::prune_len(&g_comp);
    let colp = inter::bz_match(&p_comp[..p_len], &g_comp[..g_len]);
    (p_len, g_len, colp.len())
}
