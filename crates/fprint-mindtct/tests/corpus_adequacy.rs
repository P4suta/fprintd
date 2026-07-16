// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Does the corpus actually exercise false-minutia removal? **Each of the nine removal stages in
//! `remove_false_minutia_V2` either drops a minutia on at least one corpus image, or is named in
//! [`CORPUS_BLIND`].**
//!
//! `remove_false_minutia_V2` runs its ten numbered stages unconditionally, so every image reaches
//! every stage and line coverage reads ~100% for the whole file. That number measures nothing here:
//! a stage is only *tested* when it changes the result. If `remove_pores_V2` never drops a minutia,
//! deleting its body outright leaves the `.rmin2` golden green — the golden pins what the stages
//! collectively produced, not that each one contributed.
//!
//! This file asks the question coverage cannot: which stages *fire*. It reads the per-stage drop
//! counts from `debug_removal_tally`, sums them over the corpus, and requires each stage to be
//! reached or listed. [`CORPUS_BLIND`] is measured, not assumed — shrinking it means adding an image
//! that reaches a blind stage, which is visible, gradeable work.
//!
//! ## Honest limits
//!
//! * The tally is a **length diff**, so it sees removals only. Stage 6
//!   (`remove_or_adjust_side_minutiae_V2`) both removes and *adjusts*, and an adjust relocates a
//!   minutia without changing the list length. A non-zero count for stage 6 proves its remove path
//!   is reached and says nothing about its adjust path — which is not idle: `remove_or_adjust`'s
//!   adjust-then-drop branch is unreached by this corpus, and unreached code in a published crate is
//!   where `detect_minutiae` can still hang (see the `SideAdjust::Removed` arm in `src/remove.rs`).
//! * "Fires at least once" is a floor, not adequacy. It refutes "this stage is dead weight"; it does
//!   not show the stage is *correct*, which is the `.rmin2` golden's job.
//! * Stage 1 is the sort. It permutes the list and can never change its length, so its slot is
//!   structurally zero rather than a gap in the corpus — [`STAGE_NAMES`] records that.

use std::path::{Path, PathBuf};

use fprint_mindtct::{debug_removal_tally, GrayImage};

/// The reference's ten numbered stages, indexed by stage number minus one — the layout of a
/// `debug_removal_tally` result.
const STAGE_NAMES: [&str; 10] = [
    "1 sort_minutiae_y_x (a permutation: never drops, structurally zero)",
    "2 remove_islands_and_lakes",
    "3 remove_holes",
    "4 remove_pointing_invblock_V2",
    "5 remove_near_invblock_V2",
    "6 remove_or_adjust_side_minutiae_V2 (remove path only; an adjust keeps the length)",
    "7 remove_hooks",
    "8 remove_overlaps",
    "9 remove_malformations",
    "10 remove_pores_V2",
];

/// Stages no corpus image drives to drop a minutia, by index into [`STAGE_NAMES`]. Measured, and a
/// standing debt: each entry is a stage whose deletion the `.rmin2` golden would not notice.
///
/// * `0` — the sort. Not a removal at all; it can never appear outside this list.
/// * `2` — `remove_holes`. It drops a minutia sitting on a single-pixel hole in a ridge. The corpus
///   is procedurally generated and clean, so no image carries one.
/// * `9` — `remove_pores_V2`. It needs a pore: a small valley enclosed by a ridge, with the contour
///   span ratio the stage measures. No corpus image has one.
///
/// Removing an entry requires a fixture that reaches the stage, which the frozen corpus cannot gain
/// without a deliberate oracle regeneration.
const CORPUS_BLIND: &[usize] = &[0, 2, 9];

/// Every blind stage must index a real stage.
const _: () = {
    let mut i = 0;
    while i < CORPUS_BLIND.len() {
        assert!(
            CORPUS_BLIND[i] < STAGE_NAMES.len(),
            "CORPUS_BLIND indexes a stage that does not exist"
        );
        i += 1;
    }
};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The corpus image basenames, in `manifest.txt` order.
fn corpus_names() -> Vec<String> {
    let dir = fixtures_dir();
    let text = std::fs::read_to_string(dir.join("manifest.txt"))
        .expect("manifest.txt missing — run `mise run mindtct-oracle`");
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

/// One image's per-stage drop counts.
fn image_tally(name: &str) -> [usize; 10] {
    let dir = fixtures_dir();
    let text = std::fs::read_to_string(dir.join(format!("{name}.manifest")))
        .unwrap_or_else(|e| panic!("read {name}.manifest: {e}"));
    let mut it = text.split_whitespace();
    let width: usize = it.next().expect("manifest: width").parse().expect("width");
    let height: usize = it
        .next()
        .expect("manifest: height")
        .parse()
        .expect("height");
    let ppi: u16 = it.next().expect("manifest: ppi").parse().expect("ppi");
    let data = std::fs::read(dir.join(format!("{name}.raw")))
        .unwrap_or_else(|e| panic!("read {name}.raw: {e}"));
    debug_removal_tally(GrayImage {
        data: &data,
        width,
        height,
        ppi,
    })
}

/// Sum the per-stage drop counts over the whole corpus.
fn corpus_tally() -> [usize; 10] {
    let mut total = [0usize; 10];
    let mut checked = 0usize;
    for name in corpus_names() {
        for (slot, n) in image_tally(&name).iter().enumerate() {
            total[slot] += n;
        }
        checked += 1;
    }
    assert!(checked > 0, "no images checked — corpus missing?");
    total
}

/// Every stage not in [`CORPUS_BLIND`] drops at least one minutia on at least one corpus image, so
/// deleting that stage would fail the `.rmin2` golden.
#[test]
fn every_removal_stage_fires_on_some_corpus_image() {
    let total = corpus_tally();
    let unreached: Vec<String> = (0..STAGE_NAMES.len())
        .filter(|slot| total[*slot] == 0 && !CORPUS_BLIND.contains(slot))
        .map(|slot| format!("stage {} never fired", STAGE_NAMES[slot]))
        .collect();
    assert!(
        unreached.is_empty(),
        "{} removal stage(s) are untested by the corpus but absent from CORPUS_BLIND — a mutant \
         deleting one would survive the .rmin2 golden:\n  {}\n(tally: {total:?})",
        unreached.len(),
        unreached.join("\n  ")
    );
}

/// [`CORPUS_BLIND`] is exact: every stage listed there really is unreached. An entry that starts
/// firing is progress, and this test makes the list shrink rather than quietly go stale.
#[test]
fn corpus_blind_stages_are_still_blind() {
    let total = corpus_tally();
    let now_firing: Vec<String> = CORPUS_BLIND
        .iter()
        .filter(|slot| total[**slot] > 0)
        .map(|slot| {
            format!(
                "stage {} fired {} time(s)",
                STAGE_NAMES[*slot], total[*slot]
            )
        })
        .collect();
    assert!(
        now_firing.is_empty(),
        "{} stage(s) in CORPUS_BLIND now fire — remove them from the list:\n  {}\n(tally: {total:?})",
        now_firing.len(),
        now_firing.join("\n  ")
    );
}

/// The sort is not a removal: its slot is zero for every image individually, not merely in total.
/// This is a structural property of stage 1, so it is checked per image rather than on the sum.
#[test]
fn sort_stage_never_changes_the_list_length() {
    for name in corpus_names() {
        let tally = image_tally(&name);
        assert_eq!(
            tally[0], 0,
            "{name}: the sort stage dropped {} minutia(e); it must only permute",
            tally[0]
        );
    }
}
