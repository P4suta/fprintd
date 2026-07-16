// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! # fprint-bozorth3
//!
//! A pure-Rust, dependency-free reimplementation of **BOZORTH3** — NIST NBIS's minutiae fingerprint
//! matcher, which **tolerates rotation and translation** between two impressions of a finger. Given
//! two prints as lists of minutiae, it returns an integer match score reproducing the stock NBIS
//! tool's output; higher means more corresponding ridge structure.
//!
//! The two tolerances are not the same strength, and `tests/properties.rs` holds each to its own
//! claim:
//!
//! * **Translation is exact.** Shifting a print by any whole number of pixels does not move the
//!   score at all — stage 1 reads only `dx`/`dy`, and the one absolute-coordinate reader (the
//!   cluster centroid) enters the score only as a difference between two centroids of the same
//!   print. The centroid divides with truncation, so this holds while coordinate sums stay
//!   non-negative, which every real print satisfies.
//! * **Rotation is a threshold.** Rotating a print runs its coordinates through trig and rounds them
//!   back to integers, which changes the `(x, y)` sort order and so the rows stage 1 emits. A
//!   rotated impression therefore does *not* reproduce the score; it keeps enough of it to
//!   out-score an unrelated print, which is what a matcher is for.
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
//! That conversion is the whole boundary — one `map` from whatever the caller calls a minutia:
//!
//! ```
//! use fprint_bozorth3::{match_score, Minutia, MIN_COMPUTABLE_BOZORTH_MINUTIAE};
//!
//! // Your type, whatever it is.
//! struct MyMinutia { col: i32, row: i32, angle_deg: i32 }
//!
//! fn to_bozorth(mine: &[MyMinutia]) -> Vec<Minutia> {
//!     mine.iter()
//!         .map(|m| Minutia { x: m.col, y: m.row, theta: m.angle_deg })
//!         .collect()
//! }
//!
//! let mine: Vec<MyMinutia> = (0..12)
//!     .map(|i| MyMinutia { col: 20 + i * 9, row: 30 + (i * 17) % 40, angle_deg: (i * 30) % 360 })
//!     .collect();
//! let print = to_bozorth(&mine);
//!
//! // Check the count before reading the score: under the minimum, 0 means "cannot tell".
//! assert!(print.len() >= MIN_COMPUTABLE_BOZORTH_MINUTIAE);
//! assert!(match_score(&print, &print) > 0);
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod cluster;
mod consts;
mod inter;
mod intra;
mod xyt;

pub use consts::{DEFAULT_BOZORTH_MINUTIAE, MAX_BOZORTH_MINUTIAE, MIN_COMPUTABLE_BOZORTH_MINUTIAE};

/// One detected minutia: pixel position and ridge direction.
///
/// Matches the shape of `fprint_core::Minutia` (the consumer converts).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Minutia {
    /// Column, in pixels, increasing right.
    pub x: i32,
    /// Row, in pixels, increasing down.
    pub y: i32,
    /// Ridge orientation in integer **degrees**, canonically `0..=359`.
    ///
    /// Normalization is the reference's *single conditional subtract* (`t > 180 ? t - 360 : t`),
    /// not a modulo, so canonical input lands in `(-180, 180]` — the representation stage 1
    /// expects. Only canonical input is normalized: `720` becomes `360` and `-270` stays `-270`,
    /// both outside that range.
    ///
    /// **Give this canonical input.** A full turn is not the identity here — `theta` and
    /// `theta - 360` are one direction and two different scores — and a `theta` beyond about
    /// ±46340 panics or wraps, per [`match_score`]'s *Panics*. Neither is rejected; both are the
    /// reference's arithmetic, reproduced.
    pub theta: i32,
}

impl Minutia {
    /// Construct a minutia from the `xyt` triple. The value is stored as given — see the `theta`
    /// field for what "canonical" means and why this reader does not normalize.
    #[must_use]
    pub const fn from_xyt(x: i32, y: i32, theta: i32) -> Self {
        Self { x, y, theta }
    }

    /// The `(x, y, theta)` triple, for moving a minutia between kernels that name the same fact.
    #[must_use]
    pub const fn as_xyt(&self) -> (i32, i32, i32) {
        (self.x, self.y, self.theta)
    }
}

impl From<(i32, i32, i32)> for Minutia {
    fn from((x, y, theta): (i32, i32, i32)) -> Self {
        Self { x, y, theta }
    }
}

/// The BOZORTH3 match score between a `probe` and a `gallery` minutia set.
///
/// Runs the full stock pipeline: build each print's intra-comparison Web (capped at
/// [`DEFAULT_BOZORTH_MINUTIAE`] minutiae, pruned by edge length), form the inter-print compatibility
/// list, then cluster it. Returns `0` when either print has fewer than
/// [`MIN_COMPUTABLE_BOZORTH_MINUTIAE`] minutiae, and `4000` (`QQ_OVERFLOW_SCORE`) if the internal
/// work queue overflows on a pathological input. The caller decides a match/non-match threshold.
///
/// The score is asymmetric by construction — stage 2's loop bounds are — so `match_score(a, b)` and
/// `match_score(b, a)` need not agree. Pick an order and keep it.
///
/// # Panics
///
/// This is a faithful port, and it reproduces the reference's unguarded arithmetic. In a build with
/// overflow checks on (`debug_assertions`), `i32` overflow panics; with them off it wraps, which is
/// quieter and worse — a wrapped-negative squared distance reads as a *short* edge. Neither is
/// reachable from a real reader, and both are reachable from a caller that does not bound its input:
///
/// * Two minutiae more than **46340 pixels** apart on either axis: the squared distance is computed
///   before the length guard that would reject them, so `dx * dx` overflows.
/// * A `theta` beyond about **±46340**: the relative angle is computed from an unfolded value (see
///   [`Minutia::theta`]), so its square overflows in the same way.
///
/// Both are recorded in `tests/totality.rs`.
///
/// ```
/// use fprint_bozorth3::{match_score, Minutia};
///
/// let probe: Vec<Minutia> = (0..12)
///     .map(|i| Minutia { x: 20 + i * 9, y: 30 + (i * 17) % 40, theta: (i * 30) % 360 })
///     .collect();
/// let gallery: Vec<Minutia> = probe
///     .iter()
///     .map(|m| Minutia { x: m.x + 2, y: m.y + 2, theta: m.theta })
///     .collect();
///
/// assert!(match_score(&probe, &gallery) > 0);
/// ```
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
/// for a pair — stages 1 and 2, which `tests/golden.rs` checks against the stock C triple to
/// localize any divergence to a stage.
///
/// Unconditional, mirroring the reference's `bozorth_probe_init` / `bozorth_gallery_init` /
/// `bz_match`, none of which consults [`MIN_COMPUTABLE_BOZORTH_MINUTIAE`]. That guard is a
/// *scoring* decision and lives in [`match_score`]; these sizes are facts about the tables.
#[doc(hidden)]
#[must_use]
pub fn debug_pipeline(probe: &[Minutia], gallery: &[Minutia]) -> (usize, usize, usize) {
    let p = xyt::prepare(probe, DEFAULT_BOZORTH_MINUTIAE);
    let g = xyt::prepare(gallery, DEFAULT_BOZORTH_MINUTIAE);
    let p_comp = intra::comp(&p);
    let g_comp = intra::comp(&g);
    let p_len = intra::prune_len(&p_comp);
    let g_len = intra::prune_len(&g_comp);
    let colp = inter::bz_match(&p_comp[..p_len], &g_comp[..g_len]);
    (p_len, g_len, colp.len())
}
