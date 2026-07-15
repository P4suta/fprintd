// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Docker-free regression against **real libfprint FP3 blobs**, one per template kind.
//!
//! Both fixtures were produced by the C libfprint (`fp_print_serialize`) and frozen by the
//! shim's Docker tests with `FP3_FREEZE_FIXTURES=1`, which also assert byte-identity live:
//! `virtual_device` (`tests/virtual.rs`) for the opaque `Raw` path, `virtual_image`
//! (`tests/virtual_image.rs`) for the NBIS minutiae path. These tests pin the interop on any
//! platform, with no Docker: `fprint-fp3` decodes a real blob and **re-encodes it
//! byte-for-byte**.
//!
//! Neither fixture contains biometric data: the NBIS one is minutiae extracted from a synthetic
//! image in `fprint-mindtct`'s golden corpus.

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

    // The load-bearing assertion: the re-encoding is byte-identical to libfprint's own output.
    let reencoded = fprint_fp3::to_bytes(&print).expect("re-encode");
    assert_eq!(
        reencoded.as_slice(),
        blob.as_slice(),
        "fprint-fp3 must reproduce libfprint's FP3 bytes exactly"
    );
}

/// The NBIS half: a print whose payload is nested minutiae arrays rather than one opaque blob,
/// so it exercises framing the `Raw` fixture never reaches.
#[test]
fn decodes_and_reencodes_real_libfprint_nbis_fp3_byte_for_byte() {
    let blob = include_bytes!("fixtures/libfprint_virtual_image_nbis.fp3");

    let print =
        fprint_fp3::from_bytes(blob).expect("fprint-fp3 must decode real libfprint NBIS FP3");

    assert!(
        blob.starts_with(fprint_fp3::MAGIC),
        "fixture must carry the FP3 magic"
    );
    assert_eq!(
        print.driver.as_ref().map(|d| d.0.as_str()),
        Some("virtual_image")
    );
    let Template::Nbis(captures) = &print.template else {
        panic!(
            "virtual_image yields an NBIS template, got {:?}",
            print.template
        );
    };
    assert!(
        captures.iter().any(|c| !c.is_empty()),
        "the fixture must carry real minutiae, or it guards nothing"
    );

    let reencoded = fprint_fp3::to_bytes(&print).expect("re-encode");
    assert_eq!(
        reencoded.as_slice(),
        blob.as_slice(),
        "fprint-fp3 must reproduce libfprint's NBIS FP3 bytes exactly"
    );
}
