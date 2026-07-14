// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
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
/// Hand-rolled (no `bitflags` dependency) to keep `fp-core` allocation- and dependency-free.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct DeviceFeature(u16);

impl DeviceFeature {
    pub const NONE: DeviceFeature = DeviceFeature(0);
    pub const CAPTURE: DeviceFeature = DeviceFeature(1 << 0);
    pub const IDENTIFY: DeviceFeature = DeviceFeature(1 << 1);
    pub const VERIFY: DeviceFeature = DeviceFeature(1 << 2);
    pub const STORAGE: DeviceFeature = DeviceFeature(1 << 3);
    pub const STORAGE_LIST: DeviceFeature = DeviceFeature(1 << 4);
    pub const STORAGE_DELETE: DeviceFeature = DeviceFeature(1 << 5);
    pub const STORAGE_CLEAR: DeviceFeature = DeviceFeature(1 << 6);
    pub const DUPLICATES_CHECK: DeviceFeature = DeviceFeature(1 << 7);
    pub const ALWAYS_ON: DeviceFeature = DeviceFeature(1 << 8);
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
    pub const fn contains(self, other: DeviceFeature) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether this looks like a match-on-chip device (persistent on-sensor storage).
    #[must_use]
    pub const fn is_match_on_chip(self) -> bool {
        self.contains(DeviceFeature::STORAGE)
    }
}

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

impl core::fmt::Debug for DeviceFeature {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        const NAMES: [(DeviceFeature, &str); 10] = [
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
        if self.0 == 0 {
            return f.write_str("NONE");
        }
        let mut first = true;
        for (flag, name) in NAMES {
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
    pub const NONE: FingerStatus = FingerStatus(0);
    pub const NEEDED: FingerStatus = FingerStatus(1 << 0);
    pub const PRESENT: FingerStatus = FingerStatus(1 << 1);

    pub const fn bits(self) -> u8 {
        self.0
    }
    pub const fn contains(self, other: FingerStatus) -> bool {
        self.0 & other.0 == other.0
    }
}

impl core::ops::BitOr for FingerStatus {
    type Output = FingerStatus;
    fn bitor(self, rhs: FingerStatus) -> FingerStatus {
        FingerStatus(self.0 | rhs.0)
    }
}

/// Sensor thermal state (`FpTemperature`); some sensors throttle when warm/hot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Temperature {
    Cold,
    Warm,
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
}
