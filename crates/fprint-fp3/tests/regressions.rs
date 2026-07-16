// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Frozen fuzz findings: **every input under `tests/regressions/` decodes or errors without
//! panicking, and each one that decodes is a fixed point of decode∘encode.**
//!
//! This is where a fuzz campaign's findings become permanent. `fuzz/fuzz_targets/fp3_from_bytes.rs`
//! and `fp3_fixed_point.rs` state the same two claims and search for counterexamples; they need a
//! nightly toolchain, Docker, a network fetch and a corpus. **This file needs none of them.** It is
//! an ordinary `#[test]`: it runs on every platform, in every `cargo test`, forever, and it is what
//! stops a fixed bug from coming back.
//!
//! ## The directory
//!
//! Each file is one input that once broke something. The **name carries the error class** —
//! `0001_<error-class>.fp3` — so a failure names the bug rather than a number. The directory is
//! written by hand from a campaign's artifacts; it is not `tests/fixtures/`, which is frozen
//! goldens and belongs to the oracles.
//!
//! **An empty directory passes.** That is the honest state after a campaign that found nothing, and
//! the alternative — inventing an input to justify the file — would be a lie that also passes.
//! [`the_regression_directory_is_readable`] keeps the walk itself honest: a directory that vanished
//! would otherwise make this file silently vacuous.
//!
//! ## Limits
//!
//! These are regression guards, not a search. Each input proves one bug is gone; together they
//! prove nothing about the bugs nobody has found. The search lives in `fuzz/`, and it is a
//! deliberate act — see `cargo xtask fuzz`.

use std::path::{Path, PathBuf};

use fprint_fp3::{from_bytes, to_bytes};

fn regressions_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/regressions")
}

/// Every frozen input, sorted so a failure is reproducible and the report reads in name order.
fn inputs() -> Vec<PathBuf> {
    let dir = regressions_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| p.extension().is_some_and(|e| e == "fp3"))
        .collect();
    paths.sort();
    paths
}

/// The walk must be able to find its own directory. Without this, deleting it would turn every test
/// below into a green vacuum.
#[test]
fn the_regression_directory_is_readable() {
    let dir = regressions_dir();
    assert!(
        dir.is_dir(),
        "{} is missing — the frozen fuzz findings live there, and a walk that finds nothing must \
         mean the campaign found nothing, not that the directory went away",
        dir.display()
    );
}

#[test]
fn no_frozen_input_panics_the_decoder() {
    for path in inputs() {
        let bytes = std::fs::read(&path).expect("a frozen input is readable");
        // `Ok` or `Err`, never a panic. Which one is not asserted: several of these are malformed
        // by construction, and `Err` is the right answer for them.
        let _ = from_bytes(&bytes);
    }
}

#[test]
fn every_frozen_input_that_decodes_is_a_fixed_point() {
    for path in inputs() {
        let bytes = std::fs::read(&path).expect("a frozen input is readable");
        let Ok(print) = from_bytes(&bytes) else {
            continue;
        };
        let name = path.file_name().unwrap_or_default().to_string_lossy();

        let encoded = to_bytes(&print)
            .unwrap_or_else(|e| panic!("{name}: decoded, then would not re-encode: {e}"));
        let decoded = from_bytes(&encoded).unwrap_or_else(|e| {
            panic!("{name}: bytes this codec just wrote would not decode: {e}")
        });

        // The fixed point at the *second* application. `to_bytes(from_bytes(b)) == b` is false by
        // design — `src/gvariant.rs`'s `chosen_offset_size` picks the minimal framing width, so a
        // wider-framed blob re-encodes narrower, and `driver: Some("")` normalises to absent.
        assert_eq!(
            print, decoded,
            "{name}: decode is not a fixed point of encode∘decode"
        );
    }
}
