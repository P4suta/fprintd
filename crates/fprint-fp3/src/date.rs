// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Enrollment-date <-> GLib Julian-day conversion.
//!
//! libfprint stores the enrollment date as `g_date_get_julian()`: the proleptic-Gregorian
//! day number where `0001-01-01` is Julian day **1**. An unset/invalid date is written as
//! the `G_MININT32` sentinel. This module is the only place that arithmetic lives — the
//! domain model ([`EnrollDate`]) stays a plain `(year, month, day)`.
//!
//! The calendar math is Howard Hinnant's branch-free `days_from_civil` /`civil_from_days`
//! (a public-domain algorithm), anchored so that day 0 is `1970-01-01`. Adding
//! [`EPOCH_OFFSET`] then shifts onto GLib's `0001-01-01 = 1` origin.

use fprint_core::EnrollDate;

/// GLib's "date unset" sentinel (`G_MININT32`), written when [`EnrollDate`] is absent.
pub(crate) const G_MININT32: i32 = i32::MIN;

/// Julian day of the Unix epoch (`1970-01-01`), i.e. the offset between Hinnant's
/// epoch-relative day count and GLib's `0001-01-01 = 1` Julian day.
const EPOCH_OFFSET: i32 = 719_163;

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
pub(crate) fn to_julian(date: EnrollDate) -> i32 {
    days_from_civil(date.year, date.month, date.day) as i32 + EPOCH_OFFSET
}

/// Decode a GLib Julian day back into an [`EnrollDate`]. `G_MININT32` (the "unset"
/// sentinel) yields `None`.
pub(crate) fn from_julian(julian: i32) -> Option<EnrollDate> {
    if julian == G_MININT32 {
        return None;
    }
    let (year, month, day) = civil_from_days(julian as i64 - EPOCH_OFFSET as i64);
    Some(EnrollDate { year, month, day })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_match_glib() {
        // GLib's g_date: 0001-01-01 is Julian day 1; the Unix epoch is 719163.
        assert_eq!(
            to_julian(EnrollDate {
                year: 1,
                month: 1,
                day: 1
            }),
            1
        );
        assert_eq!(
            to_julian(EnrollDate {
                year: 1970,
                month: 1,
                day: 1
            }),
            719_163
        );
        // One modern date, cross-checked against a known g_date value.
        assert_eq!(
            to_julian(EnrollDate {
                year: 2026,
                month: 7,
                day: 15
            }),
            739_812
        );
    }

    #[test]
    fn date_conversion_roundtrips() {
        let dates = [
            EnrollDate {
                year: 1,
                month: 1,
                day: 1,
            },
            EnrollDate {
                year: 1970,
                month: 1,
                day: 1,
            },
            EnrollDate {
                year: 2000,
                month: 2,
                day: 29,
            }, // leap day
            EnrollDate {
                year: 2026,
                month: 7,
                day: 15,
            },
            EnrollDate {
                year: 2999,
                month: 12,
                day: 31,
            },
        ];
        for d in dates {
            assert_eq!(from_julian(to_julian(d)), Some(d));
        }
    }

    #[test]
    fn sentinel_is_none_both_ways() {
        assert_eq!(from_julian(G_MININT32), None);
        assert_eq!(from_julian(i32::MIN), None);
    }
}
