// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Enrollment-date <-> GLib Julian-day conversion.
//!
//! libfprint stores the enrollment date as `g_date_get_julian()`: the proleptic-Gregorian
//! day number where `0001-01-01` is Julian day **1**. An unset/invalid date is written as
//! the `G_MININT32` sentinel. The arithmetic lives only here; the domain model
//! ([`EnrollDate`]) stays a plain `(year, month, day)`.
//!
//! The calendar math is Howard Hinnant's branch-free `days_from_civil` /`civil_from_days`
//! (a public-domain algorithm), anchored so that day 0 is `1970-01-01`. Adding
//! [`EPOCH_OFFSET`] then shifts onto GLib's `0001-01-01 = 1` origin.
//!
//! ## The representable range
//!
//! [`EnrollDate`] holds an `i32` year, so it spans about 4.3 billion years; an FP3 Julian day
//! is an `i32` count of *days*, so it spans about 11.7 million. **The domain model is therefore
//! wider than the wire**, and [`to_julian`] is partial: a date outside
//! `-5879610-06-23 ..= 5879611-07-11` has no FP3 encoding and yields
//! [`Fp3Error::DateOutOfRange`]. Those two dates are the exact ends — they map to `i32::MIN + 1`
//! and `i32::MAX`. `G_MININT32` is excluded rather than used: the wire spends it on "unset", so
//! the one date that would land there is not representable either.
//!
//! The arithmetic is `i64` end to end, and only the final Julian day is narrowed to `i32`. The
//! intermediate day count legitimately leaves `i32` for dates whose Julian day is still in range
//! (the day count and the Julian day differ by [`EPOCH_OFFSET`]), so narrowing before the shift
//! would reject — or silently wrap — dates that FP3 can hold.

use fprint_core::EnrollDate;

use crate::error::{Fp3Error, Result};

/// GLib's "date unset" sentinel (`G_MININT32`), written when [`EnrollDate`] is absent.
pub(crate) const G_MININT32: i32 = i32::MIN;

/// Julian day of the Unix epoch (`1970-01-01`), i.e. the offset between Hinnant's
/// epoch-relative day count and GLib's `0001-01-01 = 1` Julian day.
const EPOCH_OFFSET: i64 = 719_163;

/// Days from `1970-01-01` to `year-month-day` in the proleptic Gregorian calendar.
///
/// Howard Hinnant's `days_from_civil`: exact for the whole `i32` year range, valid for any
/// `month` in `1..=12` and `day` in `1..=31`. Intermediate math is done in `i64` so no
/// step can overflow before the (in-range) result is returned.
fn days_from_civil(year: i32, month: u8, day: u8) -> i64 {
    let y = year as i64 - if month <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let m = month as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

/// Inverse of [`days_from_civil`]: an epoch-relative day count back to `(year, month, day)`.
fn civil_from_days(z: i64) -> (i32, u8, u8) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8; // [1, 31]
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u8; // [1, 12]
    let year = (y + if month <= 2 { 1 } else { 0 }) as i32;
    (year, month, day)
}

/// Encode an [`EnrollDate`] as a GLib Julian day.
///
/// Partial, because [`EnrollDate`]'s `i32` year outranges the wire's `i32` day count: a date
/// whose Julian day leaves `i32`, or lands exactly on the `G_MININT32` "unset" sentinel, has no
/// FP3 encoding and gives [`Fp3Error::DateOutOfRange`]. Total over every `EnrollDate` value —
/// including an unvalidated `month`/`day` — because the calendar math is `i64` and only the
/// result is narrowed.
pub(crate) fn to_julian(date: EnrollDate) -> Result<i32> {
    let julian = days_from_civil(date.year, date.month, date.day) + EPOCH_OFFSET;
    match i32::try_from(julian) {
        Ok(j) if j != G_MININT32 => Ok(j),
        _ => Err(Fp3Error::DateOutOfRange(date)),
    }
}

/// Decode a GLib Julian day back into an [`EnrollDate`]. `G_MININT32` (the "unset"
/// sentinel) yields `None`.
pub(crate) fn from_julian(julian: i32) -> Option<EnrollDate> {
    if julian == G_MININT32 {
        return None;
    }
    let (year, month, day) = civil_from_days(i64::from(julian) - EPOCH_OFFSET);
    Some(EnrollDate::new(year, month, day))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_match_glib() {
        // GLib's g_date: 0001-01-01 is Julian day 1; the Unix epoch is 719163.
        assert_eq!(to_julian(EnrollDate::new(1, 1, 1)).unwrap(), 1);
        assert_eq!(to_julian(EnrollDate::new(1970, 1, 1)).unwrap(), 719_163);
        // One modern date, cross-checked against a known g_date value.
        assert_eq!(to_julian(EnrollDate::new(2026, 7, 15)).unwrap(), 739_812);
    }

    #[test]
    fn date_conversion_roundtrips() {
        let dates = [
            EnrollDate::new(1, 1, 1),
            EnrollDate::new(1970, 1, 1),
            EnrollDate::new(2000, 2, 29), // leap day
            EnrollDate::new(2026, 7, 15),
            EnrollDate::new(2999, 12, 31),
        ];
        for d in dates {
            assert_eq!(from_julian(to_julian(d).unwrap()), Some(d));
        }
    }

    #[test]
    fn sentinel_is_none_both_ways() {
        assert_eq!(from_julian(G_MININT32), None);
        assert_eq!(from_julian(i32::MIN), None);
    }

    /// The extreme Julian day a non-sentinel blob can carry, and the date it decodes to. Its
    /// epoch-relative day count is `-2148202810`, which does **not** fit `i32` even though the
    /// Julian day itself is exactly `i32::MIN + 1`, which does.
    const EDGE_JULIAN: i32 = i32::MIN + 1;
    fn edge_date() -> EnrollDate {
        EnrollDate::new(-5_879_610, 6, 23)
    }

    /// **A date is encodable whenever its Julian day fits `i32`, not whenever its intermediate
    /// day count does.** The two differ by [`EPOCH_OFFSET`], so narrowing the day count first
    /// wraps this date's `-2148202810` to `2146764486` and overflows the shift.
    #[test]
    fn julian_at_the_i32_edge_is_not_truncated() {
        let edge = edge_date();
        assert_eq!(from_julian(EDGE_JULIAN), Some(edge));
        assert_eq!(to_julian(edge).unwrap(), EDGE_JULIAN);
        assert!(days_from_civil(edge.year, edge.month, edge.day) < i64::from(i32::MIN));
    }

    /// The domain model's `i32` year outranges the wire's `i32` day count by ~365x, so the ends
    /// of the year range have no encoding. `EnrollDate.year` is public and unvalidated, so
    /// `to_bytes` reaches this with no hostile bytes involved.
    #[test]
    fn dates_outside_the_julian_i32_range_are_rejected() {
        for year in [i32::MIN, i32::MAX, -5_879_611, 5_879_612] {
            let date = EnrollDate::new(year, 1, 1);
            assert!(
                matches!(to_julian(date), Err(Fp3Error::DateOutOfRange(d)) if d == date),
                "year {year} must be rejected, not wrapped"
            );
        }
    }

    /// The wire spends `G_MININT32` on "unset", so the one date whose Julian day would land
    /// there is unrepresentable — encoding it would decode back as `None`.
    #[test]
    fn the_date_colliding_with_the_unset_sentinel_is_rejected() {
        // One day before the edge date is exactly `G_MININT32`.
        let collides = EnrollDate::new(-5_879_610, 6, 22);
        assert_eq!(
            days_from_civil(collides.year, collides.month, collides.day) + EPOCH_OFFSET,
            i64::from(G_MININT32)
        );
        assert!(matches!(
            to_julian(collides),
            Err(Fp3Error::DateOutOfRange(_))
        ));
    }

    /// The representable range, to the day: `EDGE_DATE` maps to `i32::MIN + 1` and
    /// `5879611-07-11` to `i32::MAX`, and the date on either side of those has no Julian day.
    /// **The bound is exact, not approximate** — the docs quote these dates.
    #[test]
    fn the_representable_range_ends_exactly_here() {
        let date = |year, month, day| EnrollDate::new(year, month, day);

        assert_eq!(to_julian(edge_date()).unwrap(), i32::MIN + 1);
        assert_eq!(to_julian(date(5_879_611, 7, 11)).unwrap(), i32::MAX);

        // One day below is the sentinel; one day above overflows `i32`.
        assert!(matches!(
            to_julian(date(-5_879_610, 6, 22)),
            Err(Fp3Error::DateOutOfRange(_))
        ));
        assert!(matches!(
            to_julian(date(5_879_611, 7, 12)),
            Err(Fp3Error::DateOutOfRange(_))
        ));
    }

    /// `to_julian` never panics, for any `EnrollDate` value — including the unvalidated
    /// `month`/`day` the public struct permits.
    #[test]
    fn to_julian_is_total_over_unvalidated_fields() {
        for year in [i32::MIN, -1, 0, 1970, i32::MAX] {
            for month in [0u8, 1, 12, 13, u8::MAX] {
                for day in [0u8, 1, 31, 32, u8::MAX] {
                    let _ = to_julian(EnrollDate::new(year, month, day));
                }
            }
        }
    }
}
