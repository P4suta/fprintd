// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Invariants the generated [`device_db`](fprint_backend_native::device_db) must hold.
//!
//! The table is machine-written from the libfprint id-tables, so these guard its shape rather than
//! its every row: it must be non-empty, keyed uniquely, and sorted (the binary search in `lookup`
//! depends on the order). Two anchors pin the classification itself to known facts — one host-image
//! device and one match-on-chip device — so a regeneration that flips a family is caught. No
//! reference tree or hardware is needed at test time; the facts are compiled in.

use fprint_backend_native::device_db::{all, lookup, Family};

#[test]
fn table_is_non_empty() {
    assert!(!all().is_empty(), "the device table should not be empty");
}

#[test]
fn keys_are_sorted_and_unique() {
    let mut prev: Option<(u16, u16)> = None;
    for r in all() {
        let key = (r.vid, r.pid);
        if let Some(prev) = prev {
            assert!(
                prev < key,
                "DEVICES must be strictly increasing by (vid, pid): {prev:04x?} then {key:04x?}"
            );
        }
        prev = Some(key);
    }
}

#[test]
fn lookup_agrees_with_the_table() {
    for r in all() {
        assert_eq!(
            lookup(r.vid, r.pid),
            Some(r),
            "lookup should find every listed device"
        );
    }
    // A pair no driver claims resolves to nothing.
    assert_eq!(lookup(0x0000, 0x0000), None);
}

/// Anchor: 138a:0011 is the vfs5011 validity sensor, an `FpImageDevice` — reachable by the
/// host-image seam.
#[test]
fn vfs5011_is_host_image() {
    let record = lookup(0x138a, 0x0011).expect("vfs5011 138a:0011 should be known");
    assert_eq!(record.driver, "vfs5011");
    assert_eq!(record.family, Family::HostImage);
}

/// Anchor: 27c6:5840 is the first entry of the goodixmoc id-table — a match-on-chip device the
/// host-image seam cannot reach.
#[test]
fn goodixmoc_is_match_on_chip() {
    let record = lookup(0x27c6, 0x5840).expect("goodixmoc 27c6:5840 should be known");
    assert_eq!(record.driver, "goodixmoc");
    assert_eq!(record.family, Family::MatchOnChip);
}
