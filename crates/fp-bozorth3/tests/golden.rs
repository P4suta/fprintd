// SPDX-FileCopyrightText: NIST NBIS — U.S. Government work, public domain (title 17 §105)
//
// SPDX-License-Identifier: LicenseRef-NBIS-PD

//! Golden cross-implementation oracle: our [`fp_bozorth3::match_score`] reproduces the **stock C
//! BOZORTH3** score, verified pair-for-pair against the frozen corpus.
//!
//! The corpus (`tests/fixtures/*.xyt` + `pairs.txt`) and the reference scores (`expected.tsv`) are
//! produced by the stock NBIS tool in Docker (`mise run bozorth3-oracle`) and committed as permanent
//! oracles — regenerate them only deliberately. This test needs no Docker: it reads the same `.xyt`
//! files the C read.
//!
//! ## Reproduction guarantee (and its one honest limit)
//!
//! The compatibility tables (stages 1–2) are **bit-identical** to the reference (verified: the
//! `(probe_web_len, gallery_web_len, num_edges)` triple matches exactly), and the score matches
//! **exactly** on every non-trivial match — including the largest, most cluster-heavy ones. A
//! handful of tiny near-tolerance-boundary pairs can differ by **±1**, because the *reference itself
//! is not deterministic there*: stock `bz_match_score` reads uninitialized stack locals in a rare
//! boundary path (undefined behaviour), so its score is build-dependent — the same source+compiler
//! scores these pairs differently depending only on object-file link order (see
//! `docs/bozorth3-algorithm.md`). `fp-bozorth3` is deterministic; its value equals one valid C build
//! and is ≤1 from the other. Those pairs are enumerated in [`REFERENCE_UNSTABLE`]; every other pair
//! must match to the integer, and no divergence may exceed 1 — a precise regression guard, not a
//! blanket tolerance.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use fp_bozorth3::{match_score, Minutia};

/// Pairs where the stock C reference is itself build-nondeterministic (uninitialized-read UB in
/// `bz_match_score`), so its score is only defined up to ±1. Our deterministic result may differ
/// from the committed (reference-link-order) fixture by 1 here. Any *new* divergence, or one larger
/// than 1, fails the test.
const REFERENCE_UNSTABLE: &[&str] = &["jit_10s2", "jitedge_10s2", "jit_12s2"];

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Parse a 3-column `.xyt` file ("x y theta" per line) into minutiae.
fn load_xyt(path: &Path) -> Vec<Minutia> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let mut it = l.split_whitespace();
            let x = it.next().unwrap().parse().unwrap();
            let y = it.next().unwrap().parse().unwrap();
            let theta = it.next().unwrap().parse().unwrap();
            Minutia { x, y, theta }
        })
        .collect()
}

#[test]
fn matches_stock_nbis_scores_exactly() {
    let dir = fixtures_dir();

    // expected.tsv: "tag\tscore" per line.
    let expected_text = std::fs::read_to_string(dir.join("expected.tsv"))
        .expect("expected.tsv missing — run `mise run bozorth3-oracle`");
    let expected: BTreeMap<&str, u32> = expected_text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let (tag, score) = l.split_once('\t').expect("bad expected.tsv line");
            (tag, score.trim().parse().expect("bad score"))
        })
        .collect();

    let pairs_text = std::fs::read_to_string(dir.join("pairs.txt")).expect("pairs.txt missing");

    let mut checked = 0usize;
    let mut exact = 0usize;
    let mut mismatches = Vec::new();
    for line in pairs_text.lines().filter(|l| !l.trim().is_empty()) {
        let mut it = line.split_whitespace();
        let tag = it.next().unwrap();
        let a = it.next().unwrap();
        let b = it.next().unwrap();

        let probe = load_xyt(&dir.join(a));
        let gallery = load_xyt(&dir.join(b));
        let got = match_score(&probe, &gallery) as i64;
        let want = i64::from(
            *expected
                .get(tag)
                .unwrap_or_else(|| panic!("no expected score for {tag}")),
        );

        let allowed = REFERENCE_UNSTABLE.contains(&tag);
        if got == want {
            exact += 1;
        } else if allowed && (got - want).abs() <= 1 {
            // Reference is UB-nondeterministic here; within the documented ±1. OK.
        } else {
            mismatches.push(format!(
                "{tag}: got {got}, want {want}{}",
                if allowed {
                    " (known ±1 case exceeded 1!)"
                } else {
                    ""
                }
            ));
        }
        checked += 1;
    }

    assert!(checked > 0, "no pairs checked — corpus missing?");
    assert!(
        mismatches.is_empty(),
        "{}/{} pairs diverged beyond the documented ±1 boundary set:\n  {}",
        mismatches.len(),
        checked,
        mismatches.join("\n  ")
    );
    // Regression guard: the vast majority must be bit-exact (only the small enumerated set may
    // differ, and only by ±1). If a real bug crept in, many pairs would diverge and this trips.
    assert!(
        exact >= checked - REFERENCE_UNSTABLE.len(),
        "only {exact}/{checked} pairs were bit-exact; expected at least {}",
        checked - REFERENCE_UNSTABLE.len()
    );
}
