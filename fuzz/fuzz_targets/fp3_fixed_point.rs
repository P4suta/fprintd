// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! **A `Print` the codec decoded re-encodes, and decodes back to itself.**
//!
//! Two claims, and the second is the load-bearing one:
//!
//! * `from_bytes(b) == Ok(p)` implies `to_bytes(&p)` is `Ok`. A `Print` the decoder minted must be
//!   one the encoder accepts, or hostile bytes reach a panic *through the encoder* one
//!   `fp_print_serialize` away, and the decoder's own robustness proves nothing.
//! * `from_bytes(to_bytes(p)) == p` — the **fixed point at the second application**.
//!
//! ## Why not `to_bytes(from_bytes(b)) == b`
//!
//! Because it is false, and asserting it would make this target a corpus of non-findings.
//! `src/gvariant.rs`'s `chosen_offset_size` picks the *minimal* framing width, so a blob framed
//! wider decodes fine and re-encodes narrower — same value, different bytes. `to_bytes` also
//! normalises `driver: Some("")` to absent, because GVariant `s` is not nullable and the empty
//! string is how the format spells "unset" (`src/lib.rs` states both collapses).
//!
//! Encoding is therefore not injective on blobs and `b` is not a fixed point. `p` is: the
//! normalisation happens once, on the first encode, and everything after it is stable. That is the
//! real invariant, and it is the one a caller depends on — a template written back after a read
//! must mean what it meant.
//!
//! ## Limits
//!
//! This says nothing about blobs that fail to decode; `fp3_from_bytes` covers those. It proves the
//! fixed point only for `Print`s reachable *through the decoder*, which is the set that matters
//! here and is narrower than the domain model — `tests/property.rs` sweeps the other direction.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(print) = fprint_fp3::from_bytes(data) else {
        // Not an FP3 blob. `fp3_from_bytes` is the target that cares.
        return;
    };

    let encoded = fprint_fp3::to_bytes(&print)
        .expect("a Print the decoder minted must be one the encoder accepts");

    let decoded =
        fprint_fp3::from_bytes(&encoded).expect("bytes this codec just wrote must decode");

    assert_eq!(
        print, decoded,
        "decode is not a fixed point of encode∘decode: a Print survived one round trip and \
         changed on the next, so re-saving a template rewrites what it means"
    );
});
