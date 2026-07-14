// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The `net.reactivated.Fprint` finger-name and scan-type wire vocabularies.
//!
//! fprintd names fingers with strings like `"left-index-finger"` and describes a sensor's
//! [`ScanType`] as `"swipe"`/`"press"`. These are interoperability facts — the wire
//! vocabulary of the `net.reactivated.Fprint` contract — not domain identity, so per
//! `ARCHITECTURE.md` principle 3 they live here at the daemon edge (next to [`crate::status`])
//! rather than in `fp-core`. The mappings are transcribed verbatim from fprintd's
//! `finger_name_to_num` / `finger_num_to_name` (`src/device.c`); `"any"` denotes "no specific
//! finger" ([`Finger::Unknown`]).

use fp_core::{Finger, ScanType};

/// The `net.reactivated.Fprint` finger-name string for `f` (`Unknown` ⇒ `"any"`).
pub fn finger_dbus_name(f: Finger) -> &'static str {
    match f {
        Finger::Unknown => "any",
        Finger::LeftThumb => "left-thumb",
        Finger::LeftIndex => "left-index-finger",
        Finger::LeftMiddle => "left-middle-finger",
        Finger::LeftRing => "left-ring-finger",
        Finger::LeftLittle => "left-little-finger",
        Finger::RightThumb => "right-thumb",
        Finger::RightIndex => "right-index-finger",
        Finger::RightMiddle => "right-middle-finger",
        Finger::RightRing => "right-ring-finger",
        Finger::RightLittle => "right-little-finger",
    }
}

/// Parse a `net.reactivated.Fprint` finger-name string (`"any"` ⇒ [`Finger::Unknown`]);
/// `None` for anything outside the vocabulary.
pub fn finger_from_dbus_name(s: &str) -> Option<Finger> {
    Some(match s {
        "any" => Finger::Unknown,
        "left-thumb" => Finger::LeftThumb,
        "left-index-finger" => Finger::LeftIndex,
        "left-middle-finger" => Finger::LeftMiddle,
        "left-ring-finger" => Finger::LeftRing,
        "left-little-finger" => Finger::LeftLittle,
        "right-thumb" => Finger::RightThumb,
        "right-index-finger" => Finger::RightIndex,
        "right-middle-finger" => Finger::RightMiddle,
        "right-ring-finger" => Finger::RightRing,
        "right-little-finger" => Finger::RightLittle,
        _ => return None,
    })
}

/// The `net.reactivated.Fprint` `scan-type` property string for `s`.
pub fn scan_type_dbus_str(s: ScanType) -> &'static str {
    match s {
        ScanType::Swipe => "swipe",
        ScanType::Press => "press",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finger_names_round_trip() {
        let all = [
            Finger::Unknown,
            Finger::LeftThumb,
            Finger::LeftIndex,
            Finger::LeftMiddle,
            Finger::LeftRing,
            Finger::LeftLittle,
            Finger::RightThumb,
            Finger::RightIndex,
            Finger::RightMiddle,
            Finger::RightRing,
            Finger::RightLittle,
        ];
        for f in all {
            assert_eq!(finger_from_dbus_name(finger_dbus_name(f)), Some(f));
        }
        assert_eq!(finger_dbus_name(Finger::Unknown), "any");
        assert_eq!(finger_from_dbus_name("any"), Some(Finger::Unknown));
        assert_eq!(finger_from_dbus_name("not-a-finger"), None);
    }

    #[test]
    fn scan_type_strings() {
        assert_eq!(scan_type_dbus_str(ScanType::Swipe), "swipe");
        assert_eq!(scan_type_dbus_str(ScanType::Press), "press");
    }
}
