// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Finger identity — the FP3/`FpFinger` byte value.
//!
//! The `net.reactivated.Fprint` D-Bus finger-name vocabulary (`"left-index-finger"`, …) is
//! *not* here: that is interop wire vocabulary and lives at the daemon edge
//! (`fprintd`), per `ARCHITECTURE.md` principle 3. This module keeps only the domain
//! identity — the FP3 template byte via [`Finger::as_u8`]/[`Finger::from_u8`].

/// A finger, matching libfprint's `FpFinger` enum ordering (see `fp-print.h`).
///
/// The discriminant is meaningful: it is the value stored in the FP3 template `finger`
/// byte and used to name per-finger files under `/var/lib/fprint/<user>/<driver>/`.
/// `Unknown = 0`; real fingers are `1..=10`.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Finger {
    Unknown = 0,
    LeftThumb = 1,
    LeftIndex = 2,
    LeftMiddle = 3,
    LeftRing = 4,
    LeftLittle = 5,
    RightThumb = 6,
    RightIndex = 7,
    RightMiddle = 8,
    RightRing = 9,
    RightLittle = 10,
}

impl Finger {
    /// First real finger (`FP_FINGER_FIRST`).
    pub const FIRST: Finger = Finger::LeftThumb;
    /// Last real finger (`FP_FINGER_LAST`).
    pub const LAST: Finger = Finger::RightLittle;

    /// All ten real fingers in canonical order.
    pub const ALL: [Finger; 10] = [
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

    /// Reconstruct from the raw FP3/`FpFinger` byte.
    #[must_use]
    pub fn from_u8(v: u8) -> Option<Finger> {
        Some(match v {
            0 => Finger::Unknown,
            1 => Finger::LeftThumb,
            2 => Finger::LeftIndex,
            3 => Finger::LeftMiddle,
            4 => Finger::LeftRing,
            5 => Finger::LeftLittle,
            6 => Finger::RightThumb,
            7 => Finger::RightIndex,
            8 => Finger::RightMiddle,
            9 => Finger::RightRing,
            10 => Finger::RightLittle,
            _ => return None,
        })
    }

    /// The raw FP3/`FpFinger` byte value.
    #[must_use]
    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

#[cfg(test)]
mod tests {
    use super::Finger;

    #[test]
    fn byte_roundtrip_is_stable() {
        for f in Finger::ALL {
            assert_eq!(Finger::from_u8(f.as_u8()), Some(f));
        }
        assert_eq!(Finger::from_u8(0), Some(Finger::Unknown));
        assert_eq!(Finger::from_u8(11), None);
    }

    #[test]
    fn discriminants_match_libfprint() {
        // Guards against accidental reordering that would corrupt FP3 template bytes.
        assert_eq!(Finger::LeftThumb.as_u8(), 1);
        assert_eq!(Finger::RightLittle.as_u8(), 10);
        assert_eq!(Finger::FIRST, Finger::LeftThumb);
        assert_eq!(Finger::LAST, Finger::RightLittle);
    }
}
