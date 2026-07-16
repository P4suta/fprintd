// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! **`fprint_bozorth3::match_score` terminates and is deterministic over generated minutia sets.**
//!
//! The input is `arbitrary::Unstructured` seen through `fprint_testkit`'s `ByteSource`, so this
//! target drives the same `gen::xyt` / `gen::xyt_jittered` generators `tests/properties.rs` drives
//! from an `Lcg` — one generator, two search strategies.
//!
//! ## What is asserted, and what is deliberately not
//!
//! Determinism is the only claim. The two obvious-looking alternatives are **false**:
//!
//! * **Not symmetry.** `src/inter.rs:22-24` reproduces the reference's asymmetric loop bounds (`k`
//!   up to `probe.len()-1`, `j` up to `gallery.len()`) exactly. `match_score(a, b)` and
//!   `match_score(b, a)` are not the same computation and are not required to agree.
//! * **Not self-match maximality.** `match_score(a, a) >= match_score(a, b)` is not a theorem of
//!   this algorithm, and asserting it would report a property nobody proved.
//!
//! So a crash is the finding here, and determinism is the cheap oracle riding along.
//!
//! ## Why the input is bounded the way it is
//!
//! [`FIELD`] is 400 because `DM` is 125: an edge longer than that is discarded, so minutiae
//! scattered over a field much wider than a finger form no edges at all and the pipeline is
//! skipped. A 40000-wide field would score a constant 0 — no edge survives — so widening it buys no
//! coverage. `match_score` squares its coordinate and angle deltas in `i64` (`src/intra.rs`,
//! `src/inter.rs`), so an extreme span or an unfolded `theta` yields a well-defined score rather than
//! an overflow; the field bound is about coverage and runtime, not about staying clear of a panic.
//!
//! [`MIN_SEPARATION`] and [`MAX_MINUTIAE`] bound the *density*, which is what bounds the runtime.
//! Coincident minutiae make every pair a zero-length, mutually compatible edge; stage 2 then
//! saturates its `TABLE_OVERFLOW_LIMIT` table and the cluster stage walks it from every seed.
//! Measured: 10 identical minutiae cost 2s and 16 cost 49s, while 40 minutiae on an 8-pixel grid —
//! the densest set this filter admits — cost 3.5ms. The filter is what makes this a fuzzer rather
//! than a timeout generator, and it costs no realism: no reader emits two minutiae at one pixel,
//! and ridge spacing at 500ppi is about 9 pixels.
//!
//! ## Limits
//!
//! Sets denser than [`MIN_SEPARATION`] are unreachable here, so this target says nothing about
//! them; `tests/totality.rs` owns that end of the domain and pins it by hand.

#![no_main]

use arbitrary::Unstructured;
use fprint_bozorth3::{match_score, Minutia};
use fprint_fuzz::Bytes;
use fprint_testkit::{gen, ByteSource};
use libfuzzer_sys::fuzz_target;

/// The coordinate field. Wide enough to be a fingerprint, narrow enough that `DM` (125) admits
/// edges, and two orders of magnitude below the 46340 the distance squaring overflows at.
const FIELD: i32 = 400;

/// Minutiae per print. With [`MIN_SEPARATION`], the worst case measures 3.5ms.
const MAX_MINUTIAE: i32 = 40;

/// Minimum pixel distance between two minutiae of one print. Bounds the density, and with it the
/// runtime — see the module docs.
const MIN_SEPARATION: i32 = 8;

/// Whether every pair of `m` is at least [`MIN_SEPARATION`] apart.
///
/// The squares are safe: coordinates are bounded by [`FIELD`] plus a jitter radius, far inside the
/// range a square overflows at.
fn well_separated(m: &[(i32, i32, i32)]) -> bool {
    m.iter().enumerate().all(|(i, &(ax, ay, _))| {
        m[..i].iter().all(|&(bx, by, _)| {
            let (dx, dy) = (ax - bx, ay - by);
            dx * dx + dy * dy >= MIN_SEPARATION * MIN_SEPARATION
        })
    })
}

/// The testkit yields interoperability tuples, never a domain type — that is what lets it depend on
/// nothing. Each caller maps them in a line; this is that line.
fn as_minutiae(xyt: &[(i32, i32, i32)]) -> Vec<Minutia> {
    xyt.iter()
        .map(|&(x, y, theta)| Minutia { x, y, theta })
        .collect()
}

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let mut src = Bytes(&mut u);

    let n = src.in_range(0, MAX_MINUTIAE) as usize;
    let probe = gen::xyt(&mut src, n, FIELD, FIELD);

    // A second impression of the same finger, or an unrelated print. The first reaches the cluster
    // stage (an unrelated pair usually dies in stage 2); the second is what a rejection looks like.
    let gallery = if src.ratio(3, 4) {
        let radius = src.in_range(0, 16);
        gen::xyt_jittered(&mut src, &probe, radius)
    } else {
        let m = src.in_range(0, MAX_MINUTIAE) as usize;
        gen::xyt(&mut src, m, FIELD, FIELD)
    };

    // The density filter runs after the jitter: `xyt_jittered` clamps at zero, so it can merge two
    // minutiae that were separated before it ran.
    if !well_separated(&probe) || !well_separated(&gallery) {
        return;
    }

    let (probe, gallery) = (as_minutiae(&probe), as_minutiae(&gallery));
    let first = match_score(&probe, &gallery);
    let second = match_score(&probe, &gallery);
    assert_eq!(
        first, second,
        "match_score is not deterministic: the same two prints scored {first} then {second}, so \
         state survives a call and a verify depends on how many came before it"
    );
});
