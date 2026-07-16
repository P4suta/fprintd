// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The round-trip law, stated exactly: **`from_bytes(to_bytes(p))` reproduces
//! [`normalize`]`(p)`, not `p`.**
//!
//! The codec collapses two distinctions the domain model can express and the wire cannot. Both
//! are deliberate and documented on `from_bytes`; [`normalize`] is the whole of the difference,
//! written out here so the law is checkable rather than approximate:
//!
//! * **`finger: None` becomes `Some(Finger::Unknown)`.** The wire has a `y` byte, not a maybe —
//!   absence and `Unknown` are the same byte `0`, and `0` decodes to `Unknown`.
//! * **`driver`/`device_id` of `Some("")` become `None`.** GVariant `s` is not nullable, so the
//!   codec spends the empty string on "unset"; it cannot mean an empty id as well.
//!
//! Nothing else collapses. `username`/`description` are `ms` (a real maybe), so `Some("")` and
//! `None` stay distinct; embedded NULs and non-ASCII survive; `Raw` payloads are copied verbatim.
//!
//! ## Limits
//!
//! Prints are drawn from an [`Lcg`], so this covers the shapes the generator reaches, not all
//! `Print`s. Two are deliberately out of the generator's domain:
//!
//! * **Dates outside the representable Julian band.** `EnrollDate`'s `i32` year outranges the
//!   wire's `i32` day count, so `to_bytes` rejects the ends of the year range — [`DATE_BAND`] is
//!   the generator's statement of where the encoding exists, and
//!   [`dates_outside_the_band_are_rejected`] pins that outside it is an error. The exact
//!   boundary date is pinned by the unit tests in `src/date.rs`.
//! * **Unvalidated `month`/`day`.** `EnrollDate`'s fields are public and unchecked, so
//!   `2026-13-01` is constructible; it encodes through the calendar and decodes as `2027-01-01`.
//!   That is a third collapse, reachable only by building a date that is not a date. The
//!   generator emits valid Gregorian dates only, which is what makes `normalize` the whole law
//!   here.

use fprint_core::{DeviceId, DriverId, EnrollDate, Finger, Minutia, Print, Template};
use fprint_fp3::{from_bytes, to_bytes, Fp3Error};
use fprint_testkit::gen::xyt;
use fprint_testkit::{ByteSource, Lcg};

/// Iterations per sweep. Large enough to reach every field combination the generator draws,
/// small enough that the suite stays well under a second.
const ITERATIONS: usize = 5_000;

/// The years whose Julian day is comfortably inside `i32`. The true limit is about
/// `±5_879_610`; the band stops short of it so that every date the generator emits is
/// representable regardless of month and day, which is what lets this sweep assert `Ok`
/// unconditionally.
const DATE_BAND: (i32, i32) = (-5_879_000, 5_879_000);

/// The codec's documented collapse, in full: the exact `Print` a round-trip yields.
fn normalize(p: &Print) -> Print {
    let mut n = p.clone();
    // The wire's `y` byte cannot say "absent"; `0` is `Unknown` on the way back.
    n.finger = Some(p.finger.unwrap_or(Finger::Unknown));
    // The empty string is how `s` says "unset"; it cannot also mean an empty id.
    n.driver = p.driver.clone().filter(|d| !d.as_str().is_empty());
    n.device_id = p.device_id.clone().filter(|d| !d.as_str().is_empty());
    n
}

/// Strings chosen to exercise what the framing must survive: the empty string, an embedded NUL
/// (the `s` terminator is the *last* byte, not the first NUL), and multi-byte UTF-8.
const STRINGS: [&str; 6] = ["", "a", "goodix", "a\0b", "日本語", "0000"];

fn gen_string(src: &mut impl ByteSource) -> String {
    let i = src.in_range(0, STRINGS.len() as i32 - 1) as usize;
    STRINGS[i].to_owned()
}

/// `None`, or one of [`STRINGS`] — including `""`, which is the collapsing case for the ids.
fn gen_opt_string(src: &mut impl ByteSource) -> Option<String> {
    src.ratio(3, 4).then(|| gen_string(src))
}

fn gen_finger(src: &mut impl ByteSource) -> Option<Finger> {
    if src.ratio(1, 5) {
        return None;
    }
    let b = src.in_range(0, 10) as u8;
    Some(Finger::from_u8(b).expect("0..=10 is the whole FpFinger range"))
}

/// A valid Gregorian date inside [`DATE_BAND`], leap rules included — the generator *is* the
/// statement of `to_julian`'s domain.
fn gen_date(src: &mut impl ByteSource) -> EnrollDate {
    let year = src.in_range(DATE_BAND.0, DATE_BAND.1);
    let month = src.in_range(1, 12) as u8;
    let day = src.in_range(1, i32::from(days_in_month(year, month))) as u8;
    EnrollDate::new(year, month, day)
}

/// Days in `month` of `year`, proleptic Gregorian.
fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        m => unreachable!("month {m} outside 1..=12"),
    }
}

/// Minutiae from the testkit's `xyt` triples, plus occasional `i32` extremes — the codec stores
/// each column as a plain `ai`, so it must not care about the values.
fn gen_sample(src: &mut impl ByteSource) -> Vec<Minutia> {
    let n = src.in_range(0, 4) as usize;
    if src.ratio(1, 8) {
        return (0..n)
            .map(|_| Minutia {
                x: src.u32() as i32,
                y: i32::MIN,
                theta: i32::MAX,
            })
            .collect();
    }
    xyt(src, n, 500, 500)
        .into_iter()
        .map(|(x, y, theta)| Minutia { x, y, theta })
        .collect()
}

/// A `Print` this crate can serialize: never `Template::Undefined`, and never a date outside
/// [`DATE_BAND`] — those are the two inputs `to_bytes` legitimately refuses, and they are
/// asserted separately.
fn gen_print(src: &mut impl ByteSource) -> Print {
    let template = if src.ratio(3, 4) {
        let n = src.in_range(0, 3) as usize;
        Template::Nbis((0..n).map(|_| gen_sample(src)).collect())
    } else {
        // A driver's opaque `v`, copied verbatim by the codec; its bytes are never parsed.
        let n = src.in_range(0, 12) as usize;
        let mut bytes = vec![0u8; n];
        src.fill(&mut bytes);
        Template::Raw(bytes)
    };
    Print::builder()
        .template(template)
        .finger(gen_finger(src))
        .username(gen_opt_string(src))
        .description(gen_opt_string(src))
        .driver(gen_opt_string(src).map(DriverId::new))
        .device_id(gen_opt_string(src).map(DeviceId::new))
        .device_stored(src.ratio(1, 2))
        .enroll_date(src.ratio(3, 4).then(|| gen_date(src)))
        .build()
}

/// **The law**: decoding an encoded print yields the print, up to the codec's documented
/// collapse.
#[test]
fn roundtrip_reproduces_the_normalized_print() {
    let mut lcg = Lcg::new(0xF930_0001);
    for i in 0..ITERATIONS {
        let print = gen_print(&mut lcg);
        let bytes = to_bytes(&print).unwrap_or_else(|e| {
            panic!(
                "seed {} iter {i}: to_bytes refused {print:?}: {e}",
                lcg.seed()
            )
        });
        let back = from_bytes(&bytes)
            .unwrap_or_else(|e| panic!("seed {} iter {i}: from_bytes: {e}", lcg.seed()));
        assert_eq!(
            back,
            normalize(&print),
            "seed {} iter {i}: round-trip diverged from the documented collapse",
            lcg.seed()
        );
    }
}

/// Each collapse, named and pinned on its own — the sweep proves `normalize` is *sufficient*,
/// this proves each of its clauses is *necessary*.
#[test]
fn each_documented_collapse_happens() {
    let base = Print::builder()
        .template(Template::Nbis(vec![]))
        .finger(Some(Finger::LeftIndex))
        .build();
    let decode = |p: &Print| from_bytes(&to_bytes(p).unwrap()).unwrap();

    // `finger: None` has no wire form of its own; it shares byte 0 with `Unknown`.
    let mut absent_finger = base.clone();
    absent_finger.finger = None;
    assert_eq!(decode(&absent_finger).finger, Some(Finger::Unknown));

    // An empty driver/device id is how `s` spells "unset".
    let mut empty_ids = base.clone();
    empty_ids.driver = Some(DriverId::new(""));
    empty_ids.device_id = Some(DeviceId::new(""));
    let back = decode(&empty_ids);
    assert_eq!(back.driver, None);
    assert_eq!(back.device_id, None);

    // The contrast that shows the ids' collapse is about emptiness, not about `Some`.
    let mut real_ids = base.clone();
    real_ids.driver = Some(DriverId::new("goodix"));
    real_ids.device_id = Some(DeviceId::new("0000"));
    let back = decode(&real_ids);
    assert_eq!(back.driver, Some(DriverId::new("goodix")));
    assert_eq!(back.device_id, Some(DeviceId::new("0000")));

    // And the contrast that shows `ms` does *not* collapse: `Some("")` stays distinct from `None`.
    let mut empty_text = base;
    empty_text.username = Some(String::new());
    empty_text.description = None;
    let back = decode(&empty_text);
    assert_eq!(back.username, Some(String::new()));
    assert_eq!(back.description, None);
}

/// The collapse is idempotent and the encoding is stable under it: a decoded print is already
/// normalized, so re-encoding it is byte-identical. **This is what makes the collapse safe** —
/// a stored template does not drift each time it is loaded and saved.
#[test]
fn decoded_prints_are_fixed_points() {
    let mut lcg = Lcg::new(0x5EED_0F93);
    for i in 0..ITERATIONS {
        let print = gen_print(&mut lcg);
        let bytes = to_bytes(&print).expect("generated prints are serializable");
        let back = from_bytes(&bytes).expect("decode");
        assert_eq!(normalize(&back), back, "seed {} iter {i}", lcg.seed());
        assert_eq!(
            to_bytes(&back).expect("re-encode"),
            bytes,
            "seed {} iter {i}: re-encoding a decoded print changed its bytes",
            lcg.seed()
        );
    }
}

/// Every valid Gregorian date in [`DATE_BAND`] survives the wire exactly. The sweep covers the
/// leap-year rule, month lengths, and both signs of the year.
#[test]
fn valid_dates_in_the_representable_band_roundtrip() {
    let mut lcg = Lcg::new(0xDA7E_5EED);
    for i in 0..ITERATIONS {
        let date = gen_date(&mut lcg);
        let print = Print::builder()
            .template(Template::Nbis(vec![]))
            .finger(Some(Finger::Unknown))
            .enroll_date(Some(date))
            .build();
        let back = from_bytes(&to_bytes(&print).unwrap_or_else(|e| {
            panic!(
                "seed {} iter {i}: {date:?} must be encodable: {e}",
                lcg.seed()
            )
        }))
        .expect("decode");
        assert_eq!(back.enroll_date, Some(date), "seed {} iter {i}", lcg.seed());
    }
}

/// Outside the band there is no Julian day, and `to_bytes` says so rather than wrapping.
#[test]
fn dates_outside_the_band_are_rejected() {
    for year in [i32::MIN, i32::MIN + 1, -6_000_000, 6_000_000, i32::MAX] {
        let print = Print::builder()
            .template(Template::Nbis(vec![]))
            .finger(Some(Finger::Unknown))
            .enroll_date(Some(EnrollDate::new(year, 1, 1)))
            .build();
        assert!(
            matches!(to_bytes(&print), Err(Fp3Error::DateOutOfRange(_))),
            "year {year} has no Julian day and must be refused"
        );
    }
}

/// `Template::Undefined` has no on-disk form; the generator never emits one, so it is pinned
/// here instead.
#[test]
fn undefined_templates_have_no_encoding() {
    let print = Print::new_for_enroll(Finger::LeftIndex);
    assert!(matches!(to_bytes(&print), Err(Fp3Error::UndefinedTemplate)));
}
