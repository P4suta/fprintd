// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! **`fpdev import`'s capture parser answers `Ok` or `Err` for every byte string, never a panic.**
//!
//! A capture file is the widest attacker-controlled surface `fpdev` exposes: `fpdev import` reads an
//! arbitrary `.pcapng`, classic `.pcap`, or `usbmon` text log off disk and folds it into a
//! `Session`. Every layer that touches those bytes — the container framing (`pcap-file`), the USB
//! pseudo-header decoders (usbmon binary, USBPcap), and the usbmon text scanner — runs on data the
//! author of the file chose.
//!
//! `parse_bytes` is that path minus the filesystem read and the device filter, and the claim is only
//! that it returns: `Ok(session)` or `Err`, never a panic. Which answer a given input earns is a
//! decision about the bytes, and `crates/fprint-driverkit/src/capture/tests.rs` owns that.
//!
//! `ImportFormat::Auto` sniffs the container from the leading magic, so a single format reaches all
//! three decoders; the seeds are the committed capture fixtures, so mutations land inside real USB
//! framing rather than bouncing off the sniff at byte 0 — the same seeding `fp3_from_bytes` uses.
//!
//! ## Limits
//!
//! A crash here is a finding; silence is not a proof. Anything this finds is frozen as an ordinary
//! `#[test]` alongside the capture tests, which needs no nightly and no corpus.

#![no_main]

use fprint_driverkit::capture::{parse_bytes, ImportFormat};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // The whole claim is that this returns. Both arms are correct answers; which one is a decision
    // about the bytes, and the capture tests own that. `Auto` routes through every decoder via the
    // magic sniff, which is the default path `fpdev import` runs.
    let _ = parse_bytes(data, ImportFormat::Auto);
});
