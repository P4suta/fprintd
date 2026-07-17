// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Finger identity — the FP3/`FpFinger` byte value, via the [`From`]/[`TryFrom`] conversions to and
//! from `u8` (or the equivalent [`Finger::as_u8`]/[`Finger::from_u8`] inherent methods).
//!
//! The `net.reactivated.Fprint` D-Bus finger-name vocabulary (`"left-index-finger"`, …) is
//! *not* here: that is interop wire vocabulary and lives at the daemon edge (`fprintd`),
//! per `ARCHITECTURE.md` principle 3.

/// A finger, matching libfprint's `FpFinger` enum ordering (see `fp-print.h`).
///
/// The discriminant is meaningful: it is the value stored in the FP3 template `finger`
/// byte and used to name per-finger files under `/var/lib/fprint/<user>/<driver>/`.
/// `Unknown = 0`; real fingers are `1..=10`.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Finger {
    /// `FP_FINGER_UNKNOWN`: no finger recorded. Not one of [`Finger::ALL`].
    Unknown = 0,
    /// `FP_FINGER_LEFT_THUMB`.
    LeftThumb = 1,
    /// `FP_FINGER_LEFT_INDEX`.
    LeftIndex = 2,
    /// `FP_FINGER_LEFT_MIDDLE`.
    LeftMiddle = 3,
    /// `FP_FINGER_LEFT_RING`.
    LeftRing = 4,
    /// `FP_FINGER_LEFT_LITTLE`.
    LeftLittle = 5,
    /// `FP_FINGER_RIGHT_THUMB`.
    RightThumb = 6,
    /// `FP_FINGER_RIGHT_INDEX`.
    RightIndex = 7,
    /// `FP_FINGER_RIGHT_MIDDLE`.
    RightMiddle = 8,
    /// `FP_FINGER_RIGHT_RING`.
    RightRing = 9,
    /// `FP_FINGER_RIGHT_LITTLE`.
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

/// The error [`Finger::try_from`] returns for a byte outside the valid `0..=10` range.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct InvalidFinger(
    /// The offending byte.
    pub u8,
);

impl core::fmt::Display for InvalidFinger {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{} is not a valid finger byte (expected 0..=10)", self.0)
    }
}

impl std::error::Error for InvalidFinger {}

impl From<Finger> for u8 {
    /// The raw FP3/`FpFinger` byte value — the [`From`] form of [`Finger::as_u8`].
    fn from(finger: Finger) -> u8 {
        finger as u8
    }
}

impl TryFrom<u8> for Finger {
    type Error = InvalidFinger;

    /// Reconstruct from the raw FP3/`FpFinger` byte — the [`TryFrom`] form of [`Finger::from_u8`],
    /// naming the offending byte on failure.
    fn try_from(v: u8) -> Result<Finger, InvalidFinger> {
        Finger::from_u8(v).ok_or(InvalidFinger(v))
    }
}

#[cfg(test)]
mod tests {
    use super::{Finger, InvalidFinger};

    #[test]
    fn byte_roundtrip_is_stable() {
        for f in Finger::ALL {
            assert_eq!(Finger::from_u8(f.as_u8()), Some(f));
        }
        assert_eq!(Finger::from_u8(0), Some(Finger::Unknown));
        assert_eq!(Finger::from_u8(11), None);
    }

    /// The std [`From`]/[`TryFrom`] conversions agree with the inherent `as_u8`/`from_u8` over the
    /// whole byte, and the error names the offending value.
    #[test]
    fn std_conversions_agree_with_inherent_methods() {
        for v in u8::MIN..=u8::MAX {
            assert_eq!(
                Finger::try_from(v).ok(),
                Finger::from_u8(v),
                "try_from({v})"
            );
            match Finger::try_from(v) {
                Ok(f) => assert_eq!(u8::from(f), v, "u8::from round-trip for {f:?}"),
                Err(e) => assert_eq!(e, InvalidFinger(v), "error carries the byte"),
            }
        }
        // Every real finger and Unknown convert to their discriminant.
        for f in Finger::ALL.iter().chain(&[Finger::Unknown]) {
            assert_eq!(u8::from(*f), f.as_u8());
        }
    }

    /// [`Finger::from_u8`] is total over its input, so the accepted set is checked over the whole
    /// byte rather than at the `10`/`11` boundary: **`Some` iff `0..=10`.**
    #[test]
    fn from_u8_accepts_exactly_zero_through_ten() {
        for v in u8::MIN..=u8::MAX {
            let decoded = Finger::from_u8(v);
            assert_eq!(decoded.is_some(), v <= 10, "from_u8({v}) = {decoded:?}");
            // Whatever it decodes to must encode back to the byte it came from.
            if let Some(f) = decoded {
                assert_eq!(f.as_u8(), v);
            }
        }
    }

    /// [`Finger::ALL`] is the ten real fingers — `Unknown` is not one of them — in byte order.
    #[test]
    fn all_lists_the_ten_real_fingers_in_byte_order() {
        let bytes: Vec<u8> = Finger::ALL.iter().map(|f| f.as_u8()).collect();
        assert_eq!(bytes, (1..=10).collect::<Vec<u8>>());
        assert!(!Finger::ALL.contains(&Finger::Unknown));
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
