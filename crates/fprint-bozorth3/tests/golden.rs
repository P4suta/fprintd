// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Golden cross-implementation oracle: our [`fprint_bozorth3::match_score`] reproduces the **stock C
//! BOZORTH3** score, verified pair-for-pair against the frozen corpus.
//!
//! The corpus (`tests/fixtures/*.xyt` + `pairs.txt`) and the reference scores (`expected.tsv`) are
//! produced by the stock NBIS tool in Docker (`mise run bozorth3-oracle`) and committed as permanent
//! oracles — regenerate them only deliberately. This test needs no Docker: it reads the same `.xyt`
//! files the C read.
//!
//! ## Reproduction guarantee (and its one honest limit)
//!
//! The compatibility tables (stages 1–2) are **bit-identical** to the reference — the
//! `(probe_web_len, gallery_web_len, num_edges)` triple is frozen in `stages.tsv` and checked here
//! by [`stage1_web_lengths_match_stock`] and [`stage2_edge_count_matches_stock`] — and the score matches
//! **exactly** on every non-trivial match — including the largest, most cluster-heavy ones. A
//! handful of tiny near-tolerance-boundary pairs can differ by **±1**, because the *reference itself
//! is not deterministic there*: stock `bz_match_score` reads uninitialized stack locals in a rare
//! boundary path (undefined behaviour), so its score is build-dependent — the same source+compiler
//! scores these pairs differently depending only on object-file link order (see
//! `docs/bozorth3-algorithm.md`). `fprint-bozorth3` is deterministic; its value equals one valid C build
//! and is ≤1 from the other. Those pairs are enumerated in [`REFERENCE_UNSTABLE`]; every other pair
//! must match to the integer, and no divergence may exceed 1 — a precise regression guard, not a
//! blanket tolerance.
//!
//! The stage tests are what make that limit an argument rather than a hope: they hold on **every**
//! pair, including the three in [`REFERENCE_UNSTABLE`]. Stages 1–2 exact and only the score
//! divergent confines the divergence to `bz_match_score`, which is where the reference's
//! uninitialized read is. Without them, a bug in stage 1 that happened to shift those three scores
//! by 1 would be indistinguishable from the known UB.
//!
//! Each stage is its own `#[test]` so a divergence names the stage it happened in; the score test
//! alone can only say a number changed.
//!
//! The stage tests read the pipeline sizes through `debug_pipeline`, which lives behind the
//! `unstable-diagnostics` feature, so they compile and run under `--all-features` (CI, `bacon`,
//! `mise run test`). The score test needs no feature and runs in a plain `cargo test`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use fprint_bozorth3::{match_score, Minutia};

/// Cap on reported divergences: the first few localize the bug, and a wall of them does not.
#[cfg(feature = "unstable-diagnostics")]
const MAX_REPORT: usize = 12;

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

/// One pipeline's `(probe_web_len, gallery_web_len, num_edges)` — the triple `stages.tsv` freezes.
#[cfg(feature = "unstable-diagnostics")]
type StageSizes = (usize, usize, usize);

/// Every pair's [`StageSizes`], ours beside the stock C's.
///
/// `stages.tsv` is written by the oracle's `BOZORTH3_DUMP_STAGES` driver, which calls the same
/// `bozorth_probe_init` / `bozorth_gallery_init` / `bz_match` that `bozorth_main` does.
#[cfg(feature = "unstable-diagnostics")]
fn stage_sizes() -> Vec<(String, StageSizes, StageSizes)> {
    let dir = fixtures_dir();
    let stages_text = std::fs::read_to_string(dir.join("stages.tsv"))
        .expect("stages.tsv missing — run `mise run bozorth3-oracle`");
    let want: BTreeMap<&str, StageSizes> = stages_text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let mut it = l.split('\t');
            let tag = it.next().expect("bad stages.tsv line");
            let mut n = || {
                it.next()
                    .expect("bad stages.tsv line")
                    .trim()
                    .parse()
                    .unwrap()
            };
            (tag, (n(), n(), n()))
        })
        .collect();

    let pairs_text = std::fs::read_to_string(dir.join("pairs.txt")).expect("pairs.txt missing");
    let mut out = Vec::new();
    for line in pairs_text.lines().filter(|l| !l.trim().is_empty()) {
        let mut it = line.split_whitespace();
        let tag = it.next().unwrap();
        let probe = load_xyt(&dir.join(it.next().unwrap()));
        let gallery = load_xyt(&dir.join(it.next().unwrap()));
        let got = fprint_bozorth3::debug_pipeline(&probe, &gallery);
        let want = *want
            .get(tag)
            .unwrap_or_else(|| panic!("no expected stage sizes for {tag}"));
        out.push((tag.to_string(), got, want));
    }
    assert!(!out.is_empty(), "no pairs checked — corpus missing?");
    out
}

/// Report the first [`MAX_REPORT`] divergences of one stage, or pass.
#[cfg(feature = "unstable-diagnostics")]
fn assert_stage(stage: &str, pick: impl Fn(StageSizes) -> usize) {
    let sizes = stage_sizes();
    let checked = sizes.len();
    let diverged: Vec<String> = sizes
        .iter()
        .filter(|(_, got, want)| pick(*got) != pick(*want))
        .take(MAX_REPORT)
        .map(|(tag, got, want)| format!("{tag}: got {}, want {}", pick(*got), pick(*want)))
        .collect();
    assert!(
        diverged.is_empty(),
        "{stage} diverges from stock NBIS on {}+ of {checked} pairs:\n  {}",
        diverged.len(),
        diverged.join("\n  ")
    );
}

#[cfg(feature = "unstable-diagnostics")]
#[test]
fn stage1_web_lengths_match_stock() {
    // The pruned Web length is bz_comp + bz_find + the FDD floor. Both prints, so a probe-only
    // divergence cannot hide behind a matching gallery.
    assert_stage("stage 1 probe web length", |(p, _, _)| p);
    assert_stage("stage 1 gallery web length", |(_, g, _)| g);
}

#[cfg(feature = "unstable-diagnostics")]
#[test]
fn stage2_edge_count_matches_stock() {
    assert_stage("stage 2 compatible edge count", |(_, _, n)| n);
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
