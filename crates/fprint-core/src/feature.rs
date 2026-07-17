// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Device capabilities and status — mirrors libfprint's `FpDeviceFeature`, `FpScanType`,
//! `FpFingerStatusFlags`, and `FpTemperature`.

/// Capability bit-flags, matching libfprint's `FpDeviceFeature` (`fp-device.h`).
///
/// The bit positions are load-bearing: `IDENTIFY = 1<<1` and `VERIFY = 1<<2` (note the
/// order is *not* alphabetical in libfprint). `STORAGE` distinguishes match-on-chip (MOC)
/// devices — which keep templates on the sensor — from host-side-storage devices.
///
/// Hand-rolled (no `bitflags` dependency) to keep `fprint-core` allocation- and dependency-free.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct DeviceFeature(u16);

impl DeviceFeature {
    /// The empty set (`FP_DEVICE_FEATURE_NONE`). Every set contains it.
    pub const NONE: DeviceFeature = DeviceFeature(0);
    /// `FP_DEVICE_FEATURE_CAPTURE`: the device can return a raw image (`fp_device_capture`).
    /// Image sensors have it; match-on-chip sensors normally do not.
    pub const CAPTURE: DeviceFeature = DeviceFeature(1 << 0);
    /// `FP_DEVICE_FEATURE_IDENTIFY`: the device can match one scan against a gallery
    /// ([`Device::identify`](crate::Device::identify)).
    pub const IDENTIFY: DeviceFeature = DeviceFeature(1 << 1);
    /// `FP_DEVICE_FEATURE_VERIFY`: the device can match one scan against one print
    /// ([`Device::verify`](crate::Device::verify)).
    pub const VERIFY: DeviceFeature = DeviceFeature(1 << 2);
    /// `FP_DEVICE_FEATURE_STORAGE`: the device keeps templates on the sensor. This is what makes
    /// a device match-on-chip — see [`DeviceFeature::is_match_on_chip`].
    pub const STORAGE: DeviceFeature = DeviceFeature(1 << 3);
    /// `FP_DEVICE_FEATURE_STORAGE_LIST`: on-sensor storage can be enumerated
    /// ([`Device::list_prints`](crate::Device::list_prints)).
    pub const STORAGE_LIST: DeviceFeature = DeviceFeature(1 << 4);
    /// `FP_DEVICE_FEATURE_STORAGE_DELETE`: a single template can be removed from on-sensor
    /// storage ([`Device::delete_print`](crate::Device::delete_print)).
    pub const STORAGE_DELETE: DeviceFeature = DeviceFeature(1 << 5);
    /// `FP_DEVICE_FEATURE_STORAGE_CLEAR`: on-sensor storage can be erased wholesale
    /// ([`Device::clear_storage`](crate::Device::clear_storage)).
    pub const STORAGE_CLEAR: DeviceFeature = DeviceFeature(1 << 6);
    /// `FP_DEVICE_FEATURE_DUPLICATES_CHECK`: the device rejects enrolling a finger it already
    /// holds, reporting [`Error::DataDuplicate`](crate::Error::DataDuplicate).
    pub const DUPLICATES_CHECK: DeviceFeature = DeviceFeature(1 << 7);
    /// `FP_DEVICE_FEATURE_ALWAYS_ON`: the sensor runs continuously rather than being powered up
    /// per operation.
    pub const ALWAYS_ON: DeviceFeature = DeviceFeature(1 << 8);
    /// `FP_DEVICE_FEATURE_UPDATE_PRINT`: enrolling can extend an existing template with new scans
    /// instead of replacing it.
    pub const UPDATE_PRINT: DeviceFeature = DeviceFeature(1 << 9);

    /// Raw bitmask (as returned by `fp_device_get_features`).
    #[must_use]
    pub const fn bits(self) -> u16 {
        self.0
    }

    /// Build from a raw bitmask, keeping only defined bits.
    ///
    /// Takes a `u32` (libfprint's `FpDeviceFeature` is a 32-bit flags type) and masks to the
    /// defined `0..=9` bits here, so callers never narrow before masking.
    #[must_use]
    pub const fn from_bits_truncate(bits: u32) -> DeviceFeature {
        // 0..=9 are defined; mask off unknown/high bits.
        DeviceFeature((bits & 0x03FF) as u16)
    }

    /// `fp_device_has_feature`: true iff every bit in `other` is set in `self`.
    #[must_use]
    pub const fn contains(self, other: DeviceFeature) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether no flag is set (equal to [`DeviceFeature::NONE`]).
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether `self` and `other` share any flag.
    #[must_use]
    pub const fn intersects(self, other: DeviceFeature) -> bool {
        self.0 & other.0 != 0
    }

    /// Whether this looks like a match-on-chip device (persistent on-sensor storage).
    #[must_use]
    pub const fn is_match_on_chip(self) -> bool {
        self.contains(DeviceFeature::STORAGE)
    }
}

/// Every defined flag with its libfprint name — the one registry the [`Debug`] rendering and the
/// [`DeviceFeature::from_bits_truncate`] mask are both derived from, and the one list the tests
/// across this crate quantify over.
pub(crate) const FLAGS: [(DeviceFeature, &str); 10] = [
    (DeviceFeature::CAPTURE, "CAPTURE"),
    (DeviceFeature::IDENTIFY, "IDENTIFY"),
    (DeviceFeature::VERIFY, "VERIFY"),
    (DeviceFeature::STORAGE, "STORAGE"),
    (DeviceFeature::STORAGE_LIST, "STORAGE_LIST"),
    (DeviceFeature::STORAGE_DELETE, "STORAGE_DELETE"),
    (DeviceFeature::STORAGE_CLEAR, "STORAGE_CLEAR"),
    (DeviceFeature::DUPLICATES_CHECK, "DUPLICATES_CHECK"),
    (DeviceFeature::ALWAYS_ON, "ALWAYS_ON"),
    (DeviceFeature::UPDATE_PRINT, "UPDATE_PRINT"),
];

/// The union of every flag in [`FLAGS`].
const fn defined_bits() -> u16 {
    let mut acc = 0u16;
    let mut i = 0;
    while i < FLAGS.len() {
        acc |= FLAGS[i].0.bits();
        i += 1;
    }
    acc
}

/// **The truncation mask is exactly the defined flags.** An eleventh flag registered in [`FLAGS`]
/// without widening the `0x03FF` literal in [`DeviceFeature::from_bits_truncate`] fails to compile
/// here, rather than being silently masked off at runtime.
const _: () = assert!(DeviceFeature::from_bits_truncate(u32::MAX).bits() == defined_bits());

impl core::ops::BitOr for DeviceFeature {
    type Output = DeviceFeature;
    fn bitor(self, rhs: DeviceFeature) -> DeviceFeature {
        DeviceFeature(self.0 | rhs.0)
    }
}

impl core::ops::BitOrAssign for DeviceFeature {
    fn bitor_assign(&mut self, rhs: DeviceFeature) {
        self.0 |= rhs.0;
    }
}

impl core::ops::BitAnd for DeviceFeature {
    type Output = DeviceFeature;
    /// Set intersection: the flags in both sets.
    fn bitand(self, rhs: DeviceFeature) -> DeviceFeature {
        DeviceFeature(self.0 & rhs.0)
    }
}

impl core::ops::BitAndAssign for DeviceFeature {
    fn bitand_assign(&mut self, rhs: DeviceFeature) {
        self.0 &= rhs.0;
    }
}

impl core::ops::Sub for DeviceFeature {
    type Output = DeviceFeature;
    /// Set difference: the flags in `self` that are not in `rhs` (`self & !rhs`).
    fn sub(self, rhs: DeviceFeature) -> DeviceFeature {
        DeviceFeature(self.0 & !rhs.0)
    }
}

impl core::ops::SubAssign for DeviceFeature {
    fn sub_assign(&mut self, rhs: DeviceFeature) {
        self.0 &= !rhs.0;
    }
}

impl core::ops::Not for DeviceFeature {
    type Output = DeviceFeature;
    /// Set complement **within the defined flags** — unknown/high bits never appear, so
    /// `!!x == x` for every representable set.
    fn not(self) -> DeviceFeature {
        DeviceFeature(defined_bits() & !self.0)
    }
}

impl core::fmt::Debug for DeviceFeature {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.0 == 0 {
            return f.write_str("NONE");
        }
        let mut first = true;
        for (flag, name) in FLAGS {
            if self.contains(flag) {
                if !first {
                    f.write_str(" | ")?;
                }
                first = false;
                f.write_str(name)?;
            }
        }
        Ok(())
    }
}

/// How a finger is presented to the sensor (`FpScanType`).
///
/// The `net.reactivated.Fprint` `scan-type` property string (`"swipe"`/`"press"`) is interop
/// wire vocabulary and lives at the daemon edge, not here (`ARCHITECTURE.md` principle 3).
///
/// Deliberately exhaustive, unlike the other external-vocabulary mirrors in this module: the
/// `scan-type` wire contract defines exactly these two values, the shim collapses any other
/// `FpScanType` to `Swipe`, and the daemon maps the enum totally onto the wire string — so a
/// `#[non_exhaustive]` here would only force a lossy fallback arm at that edge with nothing to carry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScanType {
    /// Finger is swiped across a line sensor.
    Swipe,
    /// Finger is pressed onto an area sensor.
    Press,
}

/// Live finger-presence status (`FpFingerStatusFlags`), used to drive UI prompts.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct FingerStatus(u8);

impl FingerStatus {
    /// `FP_FINGER_STATUS_NONE`: no status information available. Every set contains it.
    pub const NONE: FingerStatus = FingerStatus(0);
    /// `FP_FINGER_STATUS_NEEDED`: the device is waiting for a finger on the sensor — the cue to
    /// prompt the user.
    pub const NEEDED: FingerStatus = FingerStatus(1 << 0);
    /// `FP_FINGER_STATUS_PRESENT`: the device reports a finger on the sensor.
    pub const PRESENT: FingerStatus = FingerStatus(1 << 1);

    /// Raw bitmask (as carried by libfprint's `finger-status` property).
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// The defined flags, `NEEDED | PRESENT` — the mask the complement stays within.
    const DEFINED_BITS: u8 = FingerStatus::NEEDED.0 | FingerStatus::PRESENT.0;

    /// Build from a raw bitmask, keeping only the defined `NEEDED`/`PRESENT` bits.
    #[must_use]
    pub const fn from_bits_truncate(bits: u8) -> FingerStatus {
        FingerStatus(bits & FingerStatus::DEFINED_BITS)
    }

    /// True iff every bit in `other` is set in `self`.
    #[must_use]
    pub const fn contains(self, other: FingerStatus) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether no flag is set (equal to [`FingerStatus::NONE`]).
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether `self` and `other` share any flag.
    #[must_use]
    pub const fn intersects(self, other: FingerStatus) -> bool {
        self.0 & other.0 != 0
    }
}

impl core::ops::BitOr for FingerStatus {
    type Output = FingerStatus;
    fn bitor(self, rhs: FingerStatus) -> FingerStatus {
        FingerStatus(self.0 | rhs.0)
    }
}

impl core::ops::BitOrAssign for FingerStatus {
    fn bitor_assign(&mut self, rhs: FingerStatus) {
        self.0 |= rhs.0;
    }
}

impl core::ops::BitAnd for FingerStatus {
    type Output = FingerStatus;
    /// Set intersection: the flags in both sets.
    fn bitand(self, rhs: FingerStatus) -> FingerStatus {
        FingerStatus(self.0 & rhs.0)
    }
}

impl core::ops::BitAndAssign for FingerStatus {
    fn bitand_assign(&mut self, rhs: FingerStatus) {
        self.0 &= rhs.0;
    }
}

impl core::ops::Sub for FingerStatus {
    type Output = FingerStatus;
    /// Set difference: the flags in `self` that are not in `rhs` (`self & !rhs`).
    fn sub(self, rhs: FingerStatus) -> FingerStatus {
        FingerStatus(self.0 & !rhs.0)
    }
}

impl core::ops::SubAssign for FingerStatus {
    fn sub_assign(&mut self, rhs: FingerStatus) {
        self.0 &= !rhs.0;
    }
}

impl core::ops::Not for FingerStatus {
    type Output = FingerStatus;
    /// Set complement **within the defined flags** (`NEEDED`/`PRESENT`), so `!!x == x`.
    fn not(self) -> FingerStatus {
        FingerStatus(FingerStatus::DEFINED_BITS & !self.0)
    }
}

/// Sensor thermal state (`FpTemperature`); some sensors throttle when warm/hot.
///
/// `#[non_exhaustive]`: `FpTemperature` is an external vocabulary that could grow.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Temperature {
    /// `FP_TEMPERATURE_COLD`: the device can run indefinitely.
    Cold,
    /// `FP_TEMPERATURE_WARM`: the device can run for a limited time before it must cool.
    Warm,
    /// `FP_TEMPERATURE_HOT`: the device must cool down; operations are throttled or refused.
    Hot,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_bits_match_libfprint() {
        assert_eq!(DeviceFeature::CAPTURE.bits(), 1 << 0);
        assert_eq!(DeviceFeature::IDENTIFY.bits(), 1 << 1);
        assert_eq!(DeviceFeature::VERIFY.bits(), 1 << 2);
        assert_eq!(DeviceFeature::UPDATE_PRINT.bits(), 1 << 9);
    }

    #[test]
    fn contains_and_moc_detection() {
        let moc = DeviceFeature::VERIFY
            | DeviceFeature::IDENTIFY
            | DeviceFeature::STORAGE
            | DeviceFeature::STORAGE_LIST;
        assert!(moc.contains(DeviceFeature::STORAGE));
        assert!(moc.is_match_on_chip());
        assert!(!DeviceFeature::CAPTURE.is_match_on_chip());
    }

    /// The defined flags, taken from the [`FLAGS`] registry rather than relisted: an eleventh flag
    /// is quantified over by the properties below the moment it is registered.
    const DEFINED: [DeviceFeature; FLAGS.len()] = {
        let mut out = [DeviceFeature::NONE; FLAGS.len()];
        let mut i = 0;
        while i < FLAGS.len() {
            out[i] = FLAGS[i].0;
            i += 1;
        }
        out
    };

    /// **The mask keeps the defined bits and drops every other one**, checked over every `u16`
    /// input plus the high halfword the `u32` parameter admits. Expected values come from
    /// [`defined_bits`], not from a relisted `0x03FF`, so this pins truncation to the registry
    /// rather than to the literal it is implemented with.
    #[test]
    fn from_bits_truncate_keeps_exactly_the_defined_bits() {
        for b in 0..=u32::from(u16::MAX) {
            let kept = DeviceFeature::from_bits_truncate(b).bits();
            assert_eq!(
                kept,
                b as u16 & defined_bits(),
                "from_bits_truncate({b:#06x})"
            );
        }
        // Bits above the u16 the type stores must not wrap into it.
        assert_eq!(DeviceFeature::from_bits_truncate(0x1_0000).bits(), 0);
        assert_eq!(
            DeviceFeature::from_bits_truncate(u32::MAX).bits(),
            defined_bits()
        );
    }

    /// `BitOr` is a set union: associative, commutative, with [`DeviceFeature::NONE`] as identity.
    /// Checked over all 100 ordered pairs of the ten flags.
    #[test]
    fn bitor_is_associative_commutative_with_none_identity() {
        for a in DEFINED {
            assert_eq!(a | DeviceFeature::NONE, a, "identity for {a:?}");
            for b in DEFINED {
                assert_eq!(a | b, b | a, "commutativity for {a:?}, {b:?}");
                for c in DEFINED {
                    assert_eq!(
                        (a | b) | c,
                        a | (b | c),
                        "associativity for {a:?}, {b:?}, {c:?}"
                    );
                }
            }
        }
    }

    /// Over every one of the 1024 representable sets: **`NONE` is contained in all of them**, a set
    /// contains each flag it was built from, and `is_match_on_chip` is exactly `contains(STORAGE)`.
    #[test]
    fn contains_none_always_and_moc_is_exactly_storage() {
        for bits in 0..=u32::from(defined_bits()) {
            let set = DeviceFeature::from_bits_truncate(bits);
            assert!(
                set.contains(DeviceFeature::NONE),
                "{set:?} must contain NONE"
            );
            assert!(set.contains(set), "{set:?} must contain itself");
            assert_eq!(
                set.is_match_on_chip(),
                set.contains(DeviceFeature::STORAGE),
                "{set:?}"
            );
            for flag in DEFINED {
                assert_eq!(
                    set.contains(flag),
                    bits & u32::from(flag.bits()) != 0,
                    "{set:?}.contains({flag:?})"
                );
                assert!(
                    (set | flag).contains(flag),
                    "{set:?} | {flag:?} must contain {flag:?}"
                );
            }
        }
    }

    /// The intersection / difference / complement operators obey the set laws, over every one of
    /// the 1024 representable `DeviceFeature` sets (and, for the binary ops, all pairs).
    #[test]
    fn bitand_sub_not_obey_the_set_laws() {
        let all = DeviceFeature::from_bits_truncate(u32::from(defined_bits()));
        for a_bits in 0..=defined_bits() {
            let a = DeviceFeature::from_bits_truncate(u32::from(a_bits));

            // Complement stays within the defined flags and is an involution.
            assert!((a & !a).is_empty(), "a & !a must be empty for {a:?}");
            assert_eq!(a | !a, all, "a | !a must be every defined flag for {a:?}");
            assert_eq!(!!a, a, "double complement is identity for {a:?}");
            assert_eq!(a.is_empty(), a == DeviceFeature::NONE, "is_empty for {a:?}");

            for b_bits in 0..=defined_bits() {
                let b = DeviceFeature::from_bits_truncate(u32::from(b_bits));

                assert_eq!(a & b, b & a, "intersection commutes for {a:?}, {b:?}");
                assert_eq!(
                    a.intersects(b),
                    !(a & b).is_empty(),
                    "intersects agrees with & for {a:?}, {b:?}"
                );
                // Difference clears exactly b's bits from a, and with the intersection partitions a.
                let diff = a - b;
                assert!(!diff.intersects(b), "a - b keeps no b bit for {a:?}, {b:?}");
                assert_eq!(diff, a & !b, "a - b == a & !b for {a:?}, {b:?}");
                assert_eq!(diff | (a & b), a, "difference and intersection partition a");

                // The assigning forms agree with the operators.
                let mut and_acc = a;
                and_acc &= b;
                assert_eq!(and_acc, a & b, "&= agrees for {a:?}, {b:?}");
                let mut sub_acc = a;
                sub_acc -= b;
                assert_eq!(sub_acc, a - b, "-= agrees for {a:?}, {b:?}");
            }
        }
    }

    /// `BitOrAssign` agrees with `BitOr` over all 100 pairs — the two must not drift apart.
    #[test]
    fn bitor_assign_agrees_with_bitor() {
        for a in DEFINED {
            for b in DEFINED {
                let mut acc = a;
                acc |= b;
                assert_eq!(acc, a | b, "{a:?} |= {b:?}");
            }
        }
    }

    /// [`FingerStatus`] gets the same treatment over its three values.
    #[test]
    fn finger_status_bitor_is_associative_commutative_with_none_identity() {
        const THREE: [FingerStatus; 3] = [
            FingerStatus::NONE,
            FingerStatus::NEEDED,
            FingerStatus::PRESENT,
        ];
        for a in THREE {
            assert_eq!(a | FingerStatus::NONE, a, "identity for {a:?}");
            assert!(a.contains(FingerStatus::NONE), "{a:?} must contain NONE");
            for b in THREE {
                assert_eq!(a | b, b | a, "commutativity for {a:?}, {b:?}");
                assert!(
                    (a | b).contains(a) && (a | b).contains(b),
                    "union for {a:?}, {b:?}"
                );
                for c in THREE {
                    assert_eq!(
                        (a | b) | c,
                        a | (b | c),
                        "associativity for {a:?}, {b:?}, {c:?}"
                    );
                }
            }
        }
        // The two flags are distinct bits, so a finger can be both needed and present.
        let both = FingerStatus::NEEDED | FingerStatus::PRESENT;
        assert_eq!(both.bits(), 0b11);
        assert!(!FingerStatus::NEEDED.contains(FingerStatus::PRESENT));
    }

    /// [`FingerStatus`]'s intersection / difference / complement obey the same set laws.
    #[test]
    fn finger_status_bitand_sub_not_obey_the_set_laws() {
        const THREE: [FingerStatus; 4] = [
            FingerStatus::NONE,
            FingerStatus::NEEDED,
            FingerStatus::PRESENT,
            FingerStatus(0b11),
        ];
        let all = FingerStatus::NEEDED | FingerStatus::PRESENT;
        // Truncation keeps the defined bits and drops the rest, and round-trips every defined set.
        assert_eq!(FingerStatus::from_bits_truncate(0xFF), all);
        assert_eq!(FingerStatus::from_bits_truncate(0b100), FingerStatus::NONE);
        for a in THREE {
            assert_eq!(
                FingerStatus::from_bits_truncate(a.bits()),
                a,
                "round-trip {a:?}"
            );
            assert!((a & !a).is_empty(), "a & !a must be empty for {a:?}");
            assert_eq!(a | !a, all, "a | !a must be every defined flag for {a:?}");
            assert_eq!(!!a, a, "double complement is identity for {a:?}");
            assert_eq!(a.is_empty(), a == FingerStatus::NONE, "is_empty for {a:?}");
            for b in THREE {
                assert_eq!(a & b, b & a, "intersection commutes for {a:?}, {b:?}");
                assert_eq!(
                    a.intersects(b),
                    !(a & b).is_empty(),
                    "intersects for {a:?}, {b:?}"
                );
                assert_eq!(a - b, a & !b, "a - b == a & !b for {a:?}, {b:?}");
                let mut acc = a;
                acc &= b;
                assert_eq!(acc, a & b, "&= agrees for {a:?}, {b:?}");
                acc = a;
                acc -= b;
                assert_eq!(acc, a - b, "-= agrees for {a:?}, {b:?}");
            }
        }
    }
}
