// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Mutated FP3 blobs: **`from_bytes` answers `Ok` or `Err` for every byte string, and
//! `to_bytes` accepts back everything `from_bytes` returns.**
//!
//! `SECURITY.md` puts the FP3 codec parsing untrusted template bytes first in scope. That threat
//! is a total-function claim about `from_bytes`, and a total-function claim is proved by feeding
//! it bytes it did not expect. The seeds are this crate's committed goldens, so mutations land
//! inside the GVariant framing arithmetic rather than bouncing off the magic check — a random
//! byte string fails at byte 0 and proves nothing about offset tables.
//!
//! The second assertion is the load-bearing one. `from_bytes` returning `Ok` means it decided a
//! `Print`, and **a `Print` the codec produced must be one the codec can serialize** — otherwise
//! hostile bytes reach a panic through the *encoder*, one `fp_print_serialize` away, and the
//! decoder's own robustness proves nothing. That round-trip is what catches an encoder that
//! wraps rather than errors on a value only a decoder would ever mint.
//!
//! ## Limits
//!
//! This is a seeded sweep with a fixed [`SEED`], not a fuzzer: no framework, no nightly, no
//! corpus to lose or to keep in sync, and it runs in the normal `cargo test` in well under a
//! second. It buys reproducibility and a permanent regression guard, not coverage-guided search.
//! It cannot prove the absence of a panic — it proves these [`ITERATIONS`] mutations of these
//! seeds do not reach one, and it re-proves it identically on every machine and every run.
//! A mutation that lands is worth a named test of its own beside it.
//!
//! Non-termination is caught by `cargo test`'s own timeout rather than asserted: a hang has no
//! return value to check.

use fprint_fp3::{from_bytes, to_bytes};
use fprint_testkit::{ByteSource, Lcg};

/// The one seed the whole file is reproducible from.
const SEED: u64 = 0xFB30_0BAD;

/// Mutations per seed blob per strategy. The suite runs about 10k decodes in total.
const ITERATIONS: usize = 400;

// The seed corpus: this crate's committed inline goldens, one per interesting shape. They are
// copied rather than imported because `src/codec.rs`'s test module is private to the crate.

/// Full NBIS: every metadata field set, samples of 2/1/3 minutiae, a real enroll date.
const GOLDEN_NBIS_FULL: &[u8] = &[
    0x46, 0x50, 0x33, 0x02, 0x00, 0x00, 0x00, 0x67, 0x6f, 0x6f, 0x64, 0x69, 0x78, 0x00, 0x30, 0x30,
    0x30, 0x30, 0x00, 0x00, 0x07, 0x61, 0x6c, 0x69, 0x63, 0x65, 0x00, 0x00, 0x77, 0x6f, 0x72, 0x6b,
    0x20, 0x6c, 0x61, 0x70, 0x74, 0x6f, 0x70, 0x00, 0x00, 0x00, 0x00, 0xe4, 0x49, 0x0b, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x05,
    0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x10, 0x08, 0x00, 0x00, 0x07,
    0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x08, 0x04, 0x00, 0x00, 0x0a,
    0x00, 0x00, 0x00, 0x0d, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0x0b, 0x00, 0x00, 0x00, 0x0e,
    0x00, 0x00, 0x00, 0xfe, 0xff, 0xff, 0xff, 0x0c, 0x00, 0x00, 0x00, 0x0f, 0x00, 0x00, 0x00, 0xfd,
    0xff, 0xff, 0xff, 0x18, 0x0c, 0x1a, 0x2a, 0x52, 0x00, 0x28, 0x61, 0x28, 0x61, 0x69, 0x61, 0x69,
    0x61, 0x69, 0x29, 0x29, 0x30, 0x26, 0x19, 0x10, 0x0b,
];

/// RAW whose inner variant is `(su)` — the match-on-chip shape, payload copied verbatim.
const GOLDEN_RAW_SU: &[u8] = &[
    0x46, 0x50, 0x33, 0x01, 0x00, 0x00, 0x00, 0x65, 0x6c, 0x61, 0x6e, 0x00, 0x00, 0x01, 0x03, 0x00,
    0x00, 0x00, 0x80, 0x73, 0x6c, 0x6f, 0x74, 0x00, 0x00, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00, 0x05,
    0x00, 0x28, 0x73, 0x75, 0x29, 0x10, 0x0c, 0x0c, 0x0a, 0x09,
];

/// Both maybe-strings present, single-minutia NBIS — the `ms` framing.
const GOLDEN_MAYBE_BOTH: &[u8] = &[
    0x46, 0x50, 0x33, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x62, 0x6f, 0x62, 0x00, 0x00,
    0x64, 0x65, 0x73, 0x63, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x08, 0x04, 0x0e, 0x00, 0x28, 0x61, 0x28, 0x61, 0x69,
    0x61, 0x69, 0x61, 0x69, 0x29, 0x29, 0x18, 0x13, 0x0d, 0x06, 0x05,
];

/// Both maybe-strings absent, zero-sample NBIS — the shortest well-formed NBIS blob.
const GOLDEN_MAYBE_NONE: &[u8] = &[
    0x46, 0x50, 0x33, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x80, 0x00,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x08,
    0x04, 0x0e, 0x00, 0x28, 0x61, 0x28, 0x61, 0x69, 0x61, 0x69, 0x61, 0x69, 0x29, 0x29, 0x10, 0x08,
    0x08, 0x06, 0x05,
];

/// A real libfprint blob, so the corpus is not only our own encoder's output.
const FIXTURE_NBIS: &[u8] = include_bytes!("fixtures/libfprint_virtual_image_nbis.fp3");
const FIXTURE_RAW: &[u8] = include_bytes!("fixtures/libfprint_virtual_device.fp3");

/// Every seed blob. Each is well-formed, so every mutation starts one edit from valid.
const SEEDS: [&[u8]; 6] = [
    GOLDEN_NBIS_FULL,
    GOLDEN_RAW_SU,
    GOLDEN_MAYBE_BOTH,
    GOLDEN_MAYBE_NONE,
    FIXTURE_NBIS,
    FIXTURE_RAW,
];

/// The whole property, applied to one candidate blob: **decoding never panics, and whatever it
/// decodes, the encoder accepts back.**
///
/// `to_bytes` may legitimately refuse a decoded print — there is no such thing here, since every
/// decoded print carries a `Template` the encoder can write — but it may never panic, and it may
/// never silently corrupt. Both would show up as a panic in this call under the debug profile's
/// overflow checks, which is the profile `cargo test` runs.
fn decode_then_reencode_never_panics(bytes: &[u8], what: &str) {
    if let Ok(print) = from_bytes(bytes) {
        let reencoded = to_bytes(&print).unwrap_or_else(|e| {
            panic!("{what}: from_bytes accepted a print to_bytes refuses: {e}")
        });
        // A print that decoded and re-encoded must decode again to the same print: the codec's
        // output is always in its own input language.
        let again = from_bytes(&reencoded)
            .unwrap_or_else(|e| panic!("{what}: the codec's own output does not decode: {e}"));
        assert_eq!(again, print, "{what}: re-decoding changed the print");
    }
}

/// One byte, one new value. The cheapest mutation and the one most likely to leave the blob
/// plausible enough to reach deep into the framing.
#[test]
fn single_byte_flips_never_panic() {
    let mut lcg = Lcg::new(SEED);
    for (s, seed_blob) in SEEDS.iter().enumerate() {
        for i in 0..ITERATIONS {
            let mut blob = seed_blob.to_vec();
            let pos = lcg.in_range(0, blob.len() as i32 - 1) as usize;
            blob[pos] = lcg.u8();
            decode_then_reencode_never_panics(&blob, &format!("seed {SEED} blob {s} flip {i}"));
        }
    }
}

/// **Every** truncation of every seed, not a sampled few: the lengths are cheap and a prefix is
/// the likeliest thing a corrupt store hands the parser.
#[test]
fn truncation_at_every_length_never_panics() {
    for (s, seed_blob) in SEEDS.iter().enumerate() {
        for n in 0..=seed_blob.len() {
            decode_then_reencode_never_panics(
                &seed_blob[..n],
                &format!("blob {s} truncated to {n}"),
            );
        }
    }
}

/// The framing offsets live in the tail of each container, so the last bytes are where the
/// arithmetic is steered from. Corrupting them directly aims at `walk_tuple` and
/// `read_var_array` rather than at the payload.
#[test]
fn framing_offset_corruption_never_panics() {
    let mut lcg = Lcg::new(SEED ^ 0xF1A3);
    for (s, seed_blob) in SEEDS.iter().enumerate() {
        for i in 0..ITERATIONS {
            let mut blob = seed_blob.to_vec();
            // The trailing offset table: the last few bytes of the top-level tuple.
            let tail = blob.len().saturating_sub(8);
            let pos = lcg.in_range(tail as i32, blob.len() as i32 - 1) as usize;
            // Values a framing offset must survive: past the end, zero, and the extremes.
            blob[pos] = match lcg.in_range(0, 3) {
                0 => 0,
                1 => 0xff,
                2 => lcg.u8(),
                _ => blob.len() as u8,
            };
            decode_then_reencode_never_panics(&blob, &format!("seed blob {s} offset {i}"));
        }
    }
}

/// Two blobs of different shapes joined at a random point: a header promising one payload over a
/// body that is another. Splices reach type/payload disagreements that no single edit does.
#[test]
fn splices_between_seeds_never_panic() {
    let mut lcg = Lcg::new(SEED ^ 0x5B1C);
    for i in 0..ITERATIONS * 2 {
        let a = SEEDS[lcg.in_range(0, SEEDS.len() as i32 - 1) as usize];
        let b = SEEDS[lcg.in_range(0, SEEDS.len() as i32 - 1) as usize];
        let cut_a = lcg.in_range(0, a.len() as i32) as usize;
        let cut_b = lcg.in_range(0, b.len() as i32) as usize;
        let mut blob = a[..cut_a].to_vec();
        blob.extend_from_slice(&b[cut_b..]);
        decode_then_reencode_never_panics(&blob, &format!("seed {SEED} splice {i}"));
    }
}

/// Several edits at once, drifting further from well-formed than a single flip reaches.
#[test]
fn multi_byte_corruption_never_panics() {
    let mut lcg = Lcg::new(SEED ^ 0xD1E5);
    for (s, seed_blob) in SEEDS.iter().enumerate() {
        for i in 0..ITERATIONS {
            let mut blob = seed_blob.to_vec();
            let edits = lcg.in_range(2, 8);
            for _ in 0..edits {
                let pos = lcg.in_range(0, blob.len() as i32 - 1) as usize;
                blob[pos] = lcg.u8();
            }
            // Keep the magic, so the blob still reaches the framing rather than the magic check.
            blob[..3].copy_from_slice(b"FP3");
            decode_then_reencode_never_panics(&blob, &format!("seed blob {s} multi {i}"));
        }
    }
}

/// The degenerate inputs a length-driven parser is likeliest to trip on, pinned by name rather
/// than left to the sweep to stumble across. Unlike the sweeps above, these are asserted to be
/// **rejected**, not merely survived: none of them is an FP3 template, so `Ok` would be a bug
/// in itself rather than a shape the mutation harness must tolerate.
#[test]
fn degenerate_blobs_are_rejected_not_panicked() {
    for blob in [
        b"".as_slice(),
        b"F",
        b"FP",
        b"FP3", // the bare magic: no version, no tuple
        b"FP4", // one byte off the magic
        b"XXXX",
        &[0xff; 64],
        &[0x00; 64],
    ] {
        decode_then_reencode_never_panics(blob, "degenerate");
        assert!(
            from_bytes(blob).is_err(),
            "{blob:?} is not an FP3 template and must be refused"
        );
    }
}
