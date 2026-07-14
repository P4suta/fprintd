// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The two verbs: [`to_bytes`] and [`from_bytes`].
//!
//! This is the orchestration layer — it maps the pure [`fp_core::Print`] model onto the FP3
//! GVariant tuple `(issbymsmsia{sv}v)`, delegating date math to [`crate::date`] and the byte
//! framing to [`crate::gvariant`]. Every FP3-specific decision (the magic prefix, the empty
//! reserved vardict, the payload-per-type dispatch, the "empty string ⇒ absent" convention
//! for the driver/device ids) is made here and nowhere else.

use fp_core::{DeviceId, DriverId, Finger, Minutia, Print, Template};

use crate::date;
use crate::error::{Fp3Error, Result};
use crate::gvariant::{self, Spec, Val};
use crate::MAGIC;

/// `FpiPrintType::UNDEFINED` — a fresh print; never serialized.
const FPI_PRINT_UNDEFINED: i32 = 0;
/// `FpiPrintType::RAW` — the payload is the driver's own opaque variant.
const FPI_PRINT_RAW: i32 = 1;
/// `FpiPrintType::NBIS` — the payload is host-side minutiae samples.
const FPI_PRINT_NBIS: i32 = 2;

/// The GVariant type signature of the NBIS payload's inner value.
const NBIS_SIGNATURE: &[u8] = b"(a(aiaiai))";

/// The static shape of the top-level tuple `(issbymsmsia{sv}v)`, in member order.
const TOP_SPECS: [Spec; 10] = [
    Spec::fixed(4, 4), // 0 `i`  kind
    Spec::var(1),      // 1 `s`  driver
    Spec::var(1),      // 2 `s`  device_id
    Spec::fixed(1, 1), // 3 `b`  device_stored
    Spec::fixed(1, 1), // 4 `y`  finger
    Spec::var(1),      // 5 `ms` username
    Spec::var(1),      // 6 `ms` description
    Spec::fixed(4, 4), // 7 `i`  enroll_date
    Spec::var(8),      // 8 `a{sv}` reserved
    Spec::var(8),      // 9 `v`  payload
];

/// Serialize a [`Print`] to an FP3 blob: `"FP3"` magic followed by the GVariant tuple.
///
/// Errors with [`Fp3Error::UndefinedTemplate`] for an un-enrolled print — it has no on-disk
/// form.
pub fn to_bytes(print: &Print) -> Result<Vec<u8>> {
    let (kind, payload) = match &print.template {
        Template::Undefined => return Err(Fp3Error::UndefinedTemplate),
        Template::Nbis(samples) => {
            let inner = gvariant::tuple(
                vec![gvariant::array(
                    samples.iter().map(|s| sample_to_wire(s)).collect(),
                    4,
                )],
                4,
            );
            (FPI_PRINT_NBIS, gvariant::variant(inner, NBIS_SIGNATURE))
        }
        // `raw` is a self-describing standalone `v`; write it into the tuple slot verbatim.
        Template::Raw(raw) => (FPI_PRINT_RAW, gvariant::raw_variant(raw.clone())),
        // `Template` is `#[non_exhaustive]`; an unknown future kind has no FP3 form.
        _ => return Err(Fp3Error::UndefinedTemplate),
    };

    let tuple = gvariant::tuple(
        vec![
            gvariant::int32(kind),
            gvariant::string(print.driver.as_ref().map_or("", |d| d.0.as_str())),
            gvariant::string(print.device_id.as_ref().map_or("", |d| d.0.as_str())),
            gvariant::boolean(print.device_stored),
            gvariant::byte(print.finger.map_or(0, Finger::as_u8)),
            gvariant::maybe_string(print.username.as_deref()),
            gvariant::maybe_string(print.description.as_deref()),
            gvariant::int32(print.enroll_date.map_or(date::G_MININT32, date::to_julian)),
            gvariant::empty_vardict(),
            payload,
        ],
        8,
    );

    let body = tuple.into_bytes();
    let mut out = Vec::with_capacity(MAGIC.len() + body.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&body);
    Ok(out)
}

/// Parse an FP3 blob back into a [`Print`].
///
/// The inverse of [`to_bytes`] for every print this crate can produce (see the round-trip
/// tests). Note that an all-zero `finger` byte decodes to `Some(Finger::Unknown)` and empty
/// driver/device ids decode to `None` — the deliberate conventions this codec commits to.
pub fn from_bytes(bytes: &[u8]) -> Result<Print> {
    if bytes.len() < MAGIC.len() {
        return Err(Fp3Error::Truncated);
    }
    if &bytes[..MAGIC.len()] != MAGIC.as_slice() {
        return Err(Fp3Error::BadMagic);
    }

    let members = gvariant::walk_tuple(&bytes[MAGIC.len()..], &TOP_SPECS)?;
    let kind = gvariant::read_i32(members[0])?;
    let driver = gvariant::read_string(members[1])?;
    let device_id = gvariant::read_string(members[2])?;
    let device_stored = gvariant::read_bool(members[3])?;
    let finger_byte = gvariant::read_byte(members[4])?;
    let username = gvariant::read_maybe_string(members[5])?;
    let description = gvariant::read_maybe_string(members[6])?;
    let enroll_date = gvariant::read_i32(members[7])?;
    // members[8] is the reserved `a{sv}`; always empty, ignored on read.
    let payload = members[9];

    let template = match kind {
        FPI_PRINT_NBIS => {
            let (child, signature) = gvariant::split_variant(payload)?;
            if signature != NBIS_SIGNATURE {
                return Err(Fp3Error::PayloadType);
            }
            Template::Nbis(gvariant::read_var_array(child, 4, sample_from_wire)?)
        }
        // Preserve the driver's opaque variant exactly by keeping its bytes verbatim.
        FPI_PRINT_RAW => Template::Raw(payload.to_vec()),
        // UNDEFINED is never serialized; treat it and any stray discriminant alike.
        FPI_PRINT_UNDEFINED => return Err(Fp3Error::UnknownType(FPI_PRINT_UNDEFINED)),
        other => return Err(Fp3Error::UnknownType(other)),
    };

    let finger = Finger::from_u8(finger_byte).ok_or(Fp3Error::BadFinger(finger_byte))?;

    Ok(Print {
        template,
        finger: Some(finger),
        username,
        description,
        driver: non_empty(driver).map(DriverId),
        device_id: non_empty(device_id).map(DeviceId),
        device_stored,
        enroll_date: (enroll_date != date::G_MININT32)
            .then(|| date::from_julian(enroll_date))
            .flatten(),
    })
}

/// An empty string means "unset" for the driver/device-id slots (GVariant `s` has no
/// nullability); everything else is a real value.
fn non_empty(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

/// Serialize one enrolled sample as the GVariant `(aiaiai)` triple of parallel columns.
fn sample_to_wire(sample: &[Minutia]) -> Val {
    let mut x = Vec::with_capacity(sample.len());
    let mut y = Vec::with_capacity(sample.len());
    let mut theta = Vec::with_capacity(sample.len());
    for m in sample {
        x.push(m.x);
        y.push(m.y);
        theta.push(m.theta);
    }
    gvariant::tuple(
        vec![
            gvariant::int32_array(&x),
            gvariant::int32_array(&y),
            gvariant::int32_array(&theta),
        ],
        4,
    )
}

/// Parse one `(aiaiai)` sample slice back into minutiae, enforcing the equal-length invariant.
fn sample_from_wire(slice: &[u8]) -> Result<Vec<Minutia>> {
    let cols = gvariant::walk_tuple(slice, &[Spec::var(4), Spec::var(4), Spec::var(4)])?;
    let x = gvariant::read_i32_array(cols[0])?;
    let y = gvariant::read_i32_array(cols[1])?;
    let theta = gvariant::read_i32_array(cols[2])?;
    if x.len() != y.len() || y.len() != theta.len() {
        return Err(Fp3Error::UnevenSampleArrays);
    }
    Ok(x.into_iter()
        .zip(y)
        .zip(theta)
        .map(|((x, y), theta)| Minutia { x, y, theta })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fp_core::EnrollDate;

    // ---- fixtures ---------------------------------------------------------------------

    fn minutia(x: i32, y: i32, theta: i32) -> Minutia {
        Minutia { x, y, theta }
    }

    /// A representative NBIS print with every metadata field set and several samples of
    /// differing (non-zero) minutia counts, so a round-trip exercises the whole tuple.
    fn nbis_print() -> Print {
        Print {
            template: Template::Nbis(vec![
                vec![minutia(1, 2, 3), minutia(4, 5, 6)],
                vec![minutia(7, 8, 9)],
                vec![minutia(10, 11, 12), minutia(13, 14, 15), minutia(-1, -2, -3)],
            ]),
            finger: Some(Finger::RightIndex),
            username: Some("alice".into()),
            description: Some("work laptop".into()),
            driver: Some(DriverId("goodix".into())),
            device_id: Some(DeviceId("0000".into())),
            device_stored: false,
            enroll_date: Some(EnrollDate { year: 2026, month: 7, day: 15 }),
        }
    }

    // ---- golden byte-strings ----------------------------------------------------------
    //
    // Frozen GLib-normal-form bytes, captured from the previous zvariant-based *encoder*
    // (whose output was validated against libfprint's framing) for shapes without a
    // zero-minutia sample. They are the permanent oracle: the hand-rolled encoder must
    // reproduce each byte-for-byte. (The zero-minutia shapes are deliberately absent — the
    // old encoder mis-framed them; those are covered by round-trip below.)

    /// (a) full NBIS: all metadata, samples of counts 2/1/3, a real enroll date.
    const GOLDEN_NBIS_FULL: &[u8] = &[
        0x46, 0x50, 0x33, 0x02, 0x00, 0x00, 0x00, 0x67, 0x6f, 0x6f, 0x64, 0x69, 0x78, 0x00, 0x30,
        0x30, 0x30, 0x30, 0x00, 0x00, 0x07, 0x61, 0x6c, 0x69, 0x63, 0x65, 0x00, 0x00, 0x77, 0x6f,
        0x72, 0x6b, 0x20, 0x6c, 0x61, 0x70, 0x74, 0x6f, 0x70, 0x00, 0x00, 0x00, 0x00, 0xe4, 0x49,
        0x0b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x02,
        0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00,
        0x10, 0x08, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00,
        0x00, 0x08, 0x04, 0x00, 0x00, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x00, 0x00, 0x00, 0xff, 0xff,
        0xff, 0xff, 0x0b, 0x00, 0x00, 0x00, 0x0e, 0x00, 0x00, 0x00, 0xfe, 0xff, 0xff, 0xff, 0x0c,
        0x00, 0x00, 0x00, 0x0f, 0x00, 0x00, 0x00, 0xfd, 0xff, 0xff, 0xff, 0x18, 0x0c, 0x1a, 0x2a,
        0x52, 0x00, 0x28, 0x61, 0x28, 0x61, 0x69, 0x61, 0x69, 0x61, 0x69, 0x29, 0x29, 0x30, 0x26,
        0x19, 0x10, 0x0b,
    ];

    /// (b) NBIS with zero samples (`Nbis(vec![])`), finger = LeftThumb.
    const GOLDEN_NBIS_ZERO_SAMPLES: &[u8] = &[
        0x46, 0x50, 0x33, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x80,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x28, 0x61, 0x28, 0x61, 0x69, 0x61, 0x69, 0x61, 0x69, 0x29,
        0x29, 0x10, 0x08, 0x08, 0x06, 0x05,
    ];

    /// (d) RAW whose inner variant is a string, device_stored set, driver bound.
    const GOLDEN_RAW_STRING: &[u8] = &[
        0x46, 0x50, 0x33, 0x01, 0x00, 0x00, 0x00, 0x65, 0x6c, 0x61, 0x6e, 0x00, 0x00, 0x01, 0x03,
        0x00, 0x00, 0x00, 0x80, 0x6f, 0x6e, 0x2d, 0x63, 0x68, 0x69, 0x70, 0x2d, 0x68, 0x61, 0x6e,
        0x64, 0x6c, 0x65, 0x00, 0x00, 0x73, 0x10, 0x0c, 0x0c, 0x0a, 0x09,
    ];

    /// (e) RAW whose inner variant is `(su)`.
    const GOLDEN_RAW_SU: &[u8] = &[
        0x46, 0x50, 0x33, 0x01, 0x00, 0x00, 0x00, 0x65, 0x6c, 0x61, 0x6e, 0x00, 0x00, 0x01, 0x03,
        0x00, 0x00, 0x00, 0x80, 0x73, 0x6c, 0x6f, 0x74, 0x00, 0x00, 0x00, 0x00, 0x07, 0x00, 0x00,
        0x00, 0x05, 0x00, 0x28, 0x73, 0x75, 0x29, 0x10, 0x0c, 0x0c, 0x0a, 0x09,
    ];

    /// (f) maybe-strings: both present (`Some`/`Some`), single-minutia NBIS.
    const GOLDEN_MAYBE_BOTH: &[u8] = &[
        0x46, 0x50, 0x33, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x62, 0x6f, 0x62, 0x00,
        0x00, 0x64, 0x65, 0x73, 0x63, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x01, 0x00, 0x00,
        0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x08, 0x04, 0x0e, 0x00, 0x28, 0x61,
        0x28, 0x61, 0x69, 0x61, 0x69, 0x61, 0x69, 0x29, 0x29, 0x18, 0x13, 0x0d, 0x06, 0x05,
    ];

    /// (f) maybe-strings: both absent (`None`/`None`), single-minutia NBIS.
    const GOLDEN_MAYBE_NONE: &[u8] = &[
        0x46, 0x50, 0x33, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x80,
        0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
        0x00, 0x08, 0x04, 0x0e, 0x00, 0x28, 0x61, 0x28, 0x61, 0x69, 0x61, 0x69, 0x61, 0x69, 0x29,
        0x29, 0x10, 0x08, 0x08, 0x06, 0x05,
    ];

    /// (g) finger = Unknown(0), zero samples.
    const GOLDEN_FINGER_UNKNOWN: &[u8] = &[
        0x46, 0x50, 0x33, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x28, 0x61, 0x28, 0x61, 0x69, 0x61, 0x69, 0x61, 0x69, 0x29,
        0x29, 0x10, 0x08, 0x08, 0x06, 0x05,
    ];

    /// The inner standalone `v` bytes for a RAW string handle (child + `0x00` + `"s"`).
    const RAW_INNER_STRING: &[u8] = &[
        0x6f, 0x6e, 0x2d, 0x63, 0x68, 0x69, 0x70, 0x2d, 0x68, 0x61, 0x6e, 0x64, 0x6c, 0x65, 0x00,
        0x00, 0x73,
    ];

    /// The inner standalone `v` bytes for a RAW `(su)` handle (`("slot", 7u32)` + `0x00` + `"(su)"`).
    const RAW_INNER_SU: &[u8] = &[
        0x73, 0x6c, 0x6f, 0x74, 0x00, 0x00, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00, 0x05, 0x00, 0x28,
        0x73, 0x75, 0x29,
    ];

    /// The inner standalone `v` bytes for a RAW `u64` handle (`42u64` + `0x00` + `"t"`).
    const RAW_INNER_U64: &[u8] = &[
        0x2a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x74,
    ];

    // ---- golden byte-exactness --------------------------------------------------------

    #[test]
    fn encoder_reproduces_goldens_byte_for_byte() {
        assert_eq!(to_bytes(&nbis_print()).unwrap(), GOLDEN_NBIS_FULL);

        let zero = Print {
            template: Template::Nbis(vec![]),
            finger: Some(Finger::LeftThumb),
            ..Default::default()
        };
        assert_eq!(to_bytes(&zero).unwrap(), GOLDEN_NBIS_ZERO_SAMPLES);

        let raw_string = Print {
            template: Template::Raw(RAW_INNER_STRING.to_vec()),
            finger: Some(Finger::LeftMiddle),
            driver: Some(DriverId("elan".into())),
            device_stored: true,
            ..Default::default()
        };
        assert_eq!(to_bytes(&raw_string).unwrap(), GOLDEN_RAW_STRING);

        let raw_su = Print {
            template: Template::Raw(RAW_INNER_SU.to_vec()),
            finger: Some(Finger::LeftMiddle),
            driver: Some(DriverId("elan".into())),
            device_stored: true,
            ..Default::default()
        };
        assert_eq!(to_bytes(&raw_su).unwrap(), GOLDEN_RAW_SU);

        let both = Print {
            template: Template::Nbis(vec![vec![minutia(1, 1, 1)]]),
            finger: Some(Finger::RightThumb),
            username: Some("bob".into()),
            description: Some("desc".into()),
            ..Default::default()
        };
        assert_eq!(to_bytes(&both).unwrap(), GOLDEN_MAYBE_BOTH);

        let none = Print {
            template: Template::Nbis(vec![vec![minutia(1, 1, 1)]]),
            finger: Some(Finger::RightThumb),
            ..Default::default()
        };
        assert_eq!(to_bytes(&none).unwrap(), GOLDEN_MAYBE_NONE);

        let unknown = Print {
            template: Template::Nbis(vec![]),
            finger: Some(Finger::Unknown),
            ..Default::default()
        };
        assert_eq!(to_bytes(&unknown).unwrap(), GOLDEN_FINGER_UNKNOWN);
    }

    /// Every frozen golden must also decode back to the print that produced it.
    #[test]
    fn goldens_decode_back() {
        assert_eq!(from_bytes(GOLDEN_NBIS_FULL).unwrap(), nbis_print());
    }

    // ---- container: magic & truncation ------------------------------------------------

    #[test]
    fn magic_prefix_is_written() {
        let bytes = to_bytes(&nbis_print()).unwrap();
        assert_eq!(&bytes[..3], MAGIC.as_slice());
        assert_eq!(MAGIC, b"FP3");
    }

    #[test]
    fn bad_magic_is_rejected() {
        let mut bytes = to_bytes(&nbis_print()).unwrap();
        bytes[0] = b'X';
        assert!(matches!(from_bytes(&bytes), Err(Fp3Error::BadMagic)));
    }

    #[test]
    fn truncated_is_rejected() {
        assert!(matches!(from_bytes(&[]), Err(Fp3Error::Truncated)));
        assert!(matches!(from_bytes(b"FP"), Err(Fp3Error::Truncated)));
    }

    // ---- type dispatch ----------------------------------------------------------------

    #[test]
    fn undefined_not_serializable() {
        let print = Print {
            template: Template::Undefined,
            finger: Some(Finger::Unknown),
            ..Default::default()
        };
        assert!(matches!(to_bytes(&print), Err(Fp3Error::UndefinedTemplate)));
    }

    #[test]
    fn undefined_discriminant_decodes_to_unknown_type() {
        let bytes = to_bytes(&nbis_print()).unwrap();
        // The `kind` i32 is the first tuple field, right after the 3-byte magic.
        let mut forged = bytes.clone();
        forged[3] = 0; // NBIS(2) -> UNDEFINED(0)
        assert!(matches!(from_bytes(&forged), Err(Fp3Error::UnknownType(0))));
        forged[3] = 7; // an out-of-range discriminant
        assert!(matches!(from_bytes(&forged), Err(Fp3Error::UnknownType(7))));
    }

    // ---- NBIS -------------------------------------------------------------------------

    #[test]
    fn nbis_roundtrip() {
        let print = nbis_print();
        assert_eq!(from_bytes(&to_bytes(&print).unwrap()).unwrap(), print);
    }

    #[test]
    fn nbis_roundtrip_zero_samples() {
        let print = Print {
            template: Template::Nbis(vec![]),
            finger: Some(Finger::LeftThumb),
            ..Default::default()
        };
        assert_eq!(from_bytes(&to_bytes(&print).unwrap()).unwrap(), print);
    }

    // A capture that yielded no minutiae serializes to the GVariant `([], [], [])` degenerate
    // sample — a zero-sized element of the `a(aiaiai)` array. This never occurs in real
    // enrollment (MINDTCT always emits minutiae), but the hand-rolled codec handles it
    // faithfully: the empty sample is framed as its two zero framing offsets, and both a
    // lone empty sample and empty samples mixed with real ones round-trip exactly. (The
    // previous zvariant-based encoder mis-framed this and dropped the empty element; this is
    // the fix.)
    #[test]
    fn nbis_zero_minutia_sample_roundtrips() {
        let lone = Print {
            template: Template::Nbis(vec![vec![]]),
            finger: Some(Finger::LeftIndex),
            ..Default::default()
        };
        assert_eq!(from_bytes(&to_bytes(&lone).unwrap()).unwrap(), lone);

        let mixed = Print {
            template: Template::Nbis(vec![
                vec![minutia(1, 2, 3)],
                vec![],
                vec![minutia(4, 5, 6), minutia(7, 8, 9)],
            ]),
            finger: Some(Finger::LeftMiddle),
            ..Default::default()
        };
        assert_eq!(from_bytes(&to_bytes(&mixed).unwrap()).unwrap(), mixed);
    }

    #[test]
    fn nbis_uneven_arrays_rejected() {
        // Hand-frame a `(aiaiai)` whose columns differ in length (x: 2, y: 1, theta: 2) and
        // confirm the sample reader rejects it.
        let sample = gvariant::tuple(
            vec![
                gvariant::int32_array(&[1, 2]),
                gvariant::int32_array(&[3]),
                gvariant::int32_array(&[4, 5]),
            ],
            4,
        )
        .into_bytes();
        assert!(matches!(
            sample_from_wire(&sample),
            Err(Fp3Error::UnevenSampleArrays)
        ));
    }

    // ---- RAW --------------------------------------------------------------------------

    #[test]
    fn raw_roundtrip() {
        for inner in [RAW_INNER_STRING, RAW_INNER_SU, RAW_INNER_U64] {
            let print = Print {
                template: Template::Raw(inner.to_vec()),
                finger: Some(Finger::LeftMiddle),
                driver: Some(DriverId("elan".into())),
                device_stored: true,
                ..Default::default()
            };
            // Print -> bytes -> Print
            let bytes = to_bytes(&print).unwrap();
            assert_eq!(from_bytes(&bytes).unwrap(), print);
            // bytes -> Print -> bytes (byte-identical)
            assert_eq!(to_bytes(&from_bytes(&bytes).unwrap()).unwrap(), bytes);
        }
    }

    // ---- maybe-strings ----------------------------------------------------------------

    #[test]
    fn maybe_strings_all_combinations() {
        for (u, d) in [
            (None, None),
            (Some("bob".to_string()), None),
            (None, Some("desc".to_string())),
            (Some("bob".to_string()), Some("desc".to_string())),
        ] {
            let print = Print {
                template: Template::Nbis(vec![vec![minutia(1, 1, 1)]]),
                finger: Some(Finger::RightThumb),
                username: u,
                description: d,
                ..Default::default()
            };
            assert_eq!(from_bytes(&to_bytes(&print).unwrap()).unwrap(), print);
        }
    }

    // ---- finger byte ------------------------------------------------------------------

    #[test]
    fn finger_zero_is_unknown() {
        let print = Print {
            template: Template::Nbis(vec![]),
            finger: Some(Finger::Unknown),
            ..Default::default()
        };
        let decoded = from_bytes(&to_bytes(&print).unwrap()).unwrap();
        assert_eq!(decoded.finger, Some(Finger::Unknown));
    }

    #[test]
    fn out_of_range_finger_byte_is_rejected() {
        // Locate the finger byte by diffing two encodings that differ only in it, then poke
        // an out-of-range value.
        let mk = |f| Print {
            template: Template::Nbis(vec![]),
            finger: Some(f),
            ..Default::default()
        };
        let mut a = to_bytes(&mk(Finger::LeftThumb)).unwrap();
        let b = to_bytes(&mk(Finger::LeftIndex)).unwrap();
        let idx = a.iter().zip(&b).position(|(x, y)| x != y).unwrap();
        a[idx] = 11;
        assert!(matches!(from_bytes(&a), Err(Fp3Error::BadFinger(11))));
    }

    // ---- determinism ------------------------------------------------------------------

    #[test]
    fn deterministic_encode() {
        let print = nbis_print();
        assert_eq!(to_bytes(&print).unwrap(), to_bytes(&print).unwrap());
        let bytes = to_bytes(&print).unwrap();
        assert_eq!(to_bytes(&from_bytes(&bytes).unwrap()).unwrap(), bytes);
    }

    // ---- the primary invariant across a spread of prints ------------------------------

    #[test]
    fn roundtrip_is_identity() {
        let prints = [
            nbis_print(),
            Print {
                template: Template::Nbis(vec![vec![minutia(5, 5, 5)], vec![minutia(9, 9, 9)]]),
                finger: Some(Finger::RightLittle),
                device_stored: true,
                enroll_date: Some(EnrollDate { year: 1, month: 1, day: 1 }),
                ..Default::default()
            },
            Print {
                template: Template::Raw(RAW_INNER_U64.to_vec()),
                finger: Some(Finger::LeftRing),
                device_stored: true,
                ..Default::default()
            },
        ];
        for print in prints {
            assert_eq!(from_bytes(&to_bytes(&print).unwrap()).unwrap(), print);
        }
    }
}
