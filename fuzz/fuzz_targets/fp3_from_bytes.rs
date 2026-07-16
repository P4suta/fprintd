// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! **`fprint_fp3::from_bytes` answers `Ok` or `Err` for every byte string, never a panic.**
//!
//! `SECURITY.md`'s first in-scope item is this parser reading attacker-influenced template bytes,
//! so this is the highest-value target in the set: a stored `.fp3` file is the one input to the
//! stack that an attacker can write directly.
//!
//! `crates/fprint-fp3/tests/malformed.rs` makes the same claim from a seeded `Lcg` sweep — fixed,
//! fast, and reproducible on every machine. This one is the coverage-guided search behind it: the
//! seeds are that crate's committed fixtures, so mutations land inside the GVariant framing
//! arithmetic rather than bouncing off the magic check at byte 0.
//!
//! ## Limits
//!
//! A crash here is a finding; silence is not a proof. Anything this finds is frozen as an ordinary
//! `#[test]` in `crates/fprint-fp3/tests/regressions.rs`, which needs no nightly and no corpus.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // The whole claim is that this returns. Both arms are correct answers; which one is a
    // decision about the bytes, and `tests/malformed.rs` owns that.
    let _ = fprint_fp3::from_bytes(data);
});
