// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Decode frozen FP3 fixtures with the independent zgvariant GVariant implementation.
//!
//! Compare every shared metadata field and NBIS minutiae column with [`fprint_fp3::from_bytes`] while preserving the original fixture bytes.

use std::collections::HashMap;

use fprint_core::{Finger, Minutia, Template};
use zgvariant::serialized::{Context, Data};
use zgvariant::{Endian, OwnedVariant, Variant};

/// The FP3 magic prefix; the GVariant body begins after it.
const MAGIC_LEN: usize = 3;

/// libfprint's `G_MININT32` "date unset" sentinel.
const G_MININT32: i32 = i32::MIN;

/// The FP3 top-level tuple `(issbymsmsia{sv}v)`, as zgvariant reads it.
/// Its Rust shape fixes the GVariant signature zgvariant decodes against: `Option<String>` is `ms`, `HashMap` is `a{sv}`,
/// `OwnedVariant` is `v`.
/// The dict and variant slots are owned so the decoded tuple borrows nothing from the deserializer.
type Fp3Tuple = (
    i32,                           // `i`  kind (FpiPrintType)
    String,                        // `s`  driver
    String,                        // `s`  device_id
    bool,                          // `b`  device_stored
    u8,                            // `y`  finger
    Option<String>,                // `ms` username
    Option<String>,                // `ms` description
    i32,                           // `i`  enroll_date (Julian day, or G_MININT32)
    HashMap<String, OwnedVariant>, // `a{sv}` reserved (empty)
    OwnedVariant,                  // `v`  payload
);

/// `FpiPrintType::RAW`.
const KIND_RAW: i32 = 1;
/// `FpiPrintType::NBIS`.
const KIND_NBIS: i32 = 2;

/// Decode the GVariant body (magic stripped) with zgvariant, asserting it consumes every byte.
fn zgvariant_decode(blob: &[u8]) -> Fp3Tuple {
    let body = &blob[MAGIC_LEN..];
    let ctxt = Context::new(Endian::Little, 0);
    let data = Data::new(body, ctxt);
    let (tuple, consumed): (Fp3Tuple, usize) = data
        .deserialize()
        .expect("zgvariant must decode the FP3 GVariant body");
    assert_eq!(
        consumed,
        body.len(),
        "zgvariant must consume the whole body, leaving no trailing bytes"
    );
    tuple
}

/// GVariant `s` spells "unset" as the empty string; the domain model spells it `None`.
fn non_empty(s: &str) -> Option<&str> {
    (!s.is_empty()).then_some(s)
}

/// Cross-check every metadata field both readers expose, on any fixture.
fn assert_metadata_agrees(blob: &[u8]) {
    let zt = zgvariant_decode(blob);
    let print = fprint_fp3::from_bytes(blob).expect("fprint-fp3 must decode the fixture");

    // kind <-> template variant
    match zt.0 {
        KIND_RAW => assert!(
            matches!(print.template, Template::Raw(_)),
            "kind RAW must decode to Template::Raw"
        ),
        KIND_NBIS => assert!(
            matches!(print.template, Template::Nbis(_)),
            "kind NBIS must decode to Template::Nbis"
        ),
        other => panic!("unexpected FP3 kind {other}"),
    }

    // driver / device_id, with the empty-string => None collapse both readers must share
    assert_eq!(
        non_empty(&zt.1),
        print.driver.as_ref().map(|d| d.as_str()),
        "driver disagrees"
    );
    assert_eq!(
        non_empty(&zt.2),
        print.device_id.as_ref().map(|d| d.as_str()),
        "device_id disagrees"
    );

    // device_stored
    assert_eq!(zt.3, print.device_stored, "device_stored disagrees");

    // finger byte
    assert_eq!(
        Finger::from_u8(zt.4),
        print.finger,
        "finger disagrees (byte {})",
        zt.4
    );

    // maybe-strings
    assert_eq!(
        zt.5.as_deref(),
        print.username.as_deref(),
        "username disagrees"
    );
    assert_eq!(
        zt.6.as_deref(),
        print.description.as_deref(),
        "description disagrees"
    );

    // enroll_date: sentinel => None, else a present date
    let date_present = zt.7 != G_MININT32;
    assert_eq!(
        date_present,
        print.enroll_date.is_some(),
        "enroll_date presence disagrees (julian {})",
        zt.7
    );

    // the reserved vardict is always empty in a real FP3 blob
    assert!(zt.8.is_empty(), "reserved a{{sv}} must be empty");
}

/// Pull an `ai` (array of int32) out of a zgvariant [`Variant`].
fn i32_column(v: &Variant) -> Vec<i32> {
    let Variant::Array(arr) = v else {
        panic!(
            "expected an `ai` array, got signature {}",
            v.value_signature()
        );
    };
    arr.inner()
        .iter()
        .map(|e| match e {
            Variant::I32(n) => *n,
            other => panic!("expected i32, got {}", other.value_signature()),
        })
        .collect()
}

/// Reconstruct the minutiae samples from an NBIS payload `(a(aiaiai))` [`Variant`].
fn zgvariant_minutiae(payload: &Variant) -> Vec<Vec<Minutia>> {
    let Variant::Structure(outer) = payload else {
        panic!(
            "NBIS payload must be a `(a(aiaiai))` structure, got {}",
            payload.value_signature()
        );
    };
    let [Variant::Array(samples)] = outer.fields() else {
        panic!("NBIS payload structure must hold exactly one array field");
    };
    samples
        .inner()
        .iter()
        .map(|sample| {
            let Variant::Structure(cols) = sample else {
                panic!("each sample must be an `(aiaiai)` structure");
            };
            let [x, y, theta] = cols.fields() else {
                panic!("each sample must have three int32 columns");
            };
            let (xs, ys, ts) = (i32_column(x), i32_column(y), i32_column(theta));
            assert_eq!(xs.len(), ys.len(), "x/y columns differ in length");
            assert_eq!(ys.len(), ts.len(), "y/theta columns differ in length");
            xs.into_iter()
                .zip(ys)
                .zip(ts)
                .map(|((x, y), theta)| Minutia { x, y, theta })
                .collect()
        })
        .collect()
}

#[test]
fn raw_fixture_metadata_agrees() {
    let blob = include_bytes!("fixtures/libfprint_virtual_device.fp3");
    assert_metadata_agrees(blob);

    // The RAW payload is itself an opaque, self-describing standalone variant.
    let zt = zgvariant_decode(blob);
    assert_eq!(
        zt.9.value_signature().to_string(),
        "v",
        "the virtual_device RAW payload is a nested variant"
    );
}

#[test]
fn nbis_fixture_metadata_agrees() {
    let blob = include_bytes!("fixtures/libfprint_virtual_image_nbis.fp3");
    assert_metadata_agrees(blob);
}

#[test]
fn nbis_fixture_minutiae_agree() {
    let blob = include_bytes!("fixtures/libfprint_virtual_image_nbis.fp3");

    let zt = zgvariant_decode(blob);
    assert_eq!(
        zt.9.value_signature().to_string(),
        "(a(aiaiai))",
        "the virtual_image payload is the NBIS minutiae structure"
    );
    let from_zgvariant = zgvariant_minutiae(&zt.9);

    let print = fprint_fp3::from_bytes(blob).expect("fprint-fp3 must decode the NBIS fixture");
    let Template::Nbis(from_codec) = &print.template else {
        panic!("virtual_image must decode to an NBIS template");
    };

    assert_eq!(
        &from_zgvariant, from_codec,
        "the two GVariant readers must recover identical minutiae from the frozen bytes"
    );
    assert!(
        from_codec.iter().any(|s| !s.is_empty()),
        "the fixture must carry real minutiae, or the comparison guards nothing"
    );
}
