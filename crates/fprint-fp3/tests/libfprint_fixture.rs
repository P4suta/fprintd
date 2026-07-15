// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Docker-free regression against a **real libfprint FP3 blob**.
//!
//! `tests/fixtures/libfprint_virtual_device.fp3` was produced by the actual C libfprint
//! (`fp_print_serialize` on a `virtual_device` enrollment) and frozen by the shim's Docker test
//! (`fprint-backend-libfprint`, `FP3_FREEZE_FIXTURES=1`, which also asserts byte-identity live). This
//! test pins the interop the other way — on any platform, with no Docker — proving `fprint-fp3` decodes
//! that real blob and **re-encodes it byte-for-byte**. It is the M2 FP3 byte-compatibility guard.

use fprint_core::Template;

#[test]
fn decodes_and_reencodes_real_libfprint_fp3_byte_for_byte() {
    let blob = include_bytes!("fixtures/libfprint_virtual_device.fp3");

    // Decodes as a valid FP3 print...
    let print = fprint_fp3::from_bytes(blob).expect("fprint-fp3 must decode real libfprint FP3");

    // ...bound to the virtual_device driver, carrying an opaque Raw (driver-blob) template.
    assert!(
        blob.starts_with(fprint_fp3::MAGIC),
        "fixture must carry the FP3 magic"
    );
    assert_eq!(
        print.driver.as_ref().map(|d| d.0.as_str()),
        Some("virtual_device")
    );
    assert!(
        matches!(print.template, Template::Raw(_)),
        "virtual_device yields an opaque Raw template, got {:?}",
        print.template
    );

    // The load-bearing assertion: our re-encoding is byte-identical to libfprint's own output.
    let reencoded = fprint_fp3::to_bytes(&print).expect("re-encode");
    assert_eq!(
        reencoded.as_slice(),
        blob.as_slice(),
        "fprint-fp3 must reproduce libfprint's FP3 bytes exactly"
    );
}
