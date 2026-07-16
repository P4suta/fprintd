// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Fingerprint prints/templates — the domain model behind libfprint's `FpPrint` and the
//! on-disk FP3 format.
//!
//! A [`Print`] is metadata plus a [`Template`] payload. Serialization to/from the FP3
//! byte format (`"FP3"` magic + GVariant `(issbymsmsia{sv}v)`) lives in the downstream
//! `fprint-fp3` crate; this module is the in-memory model it maps to.

use crate::{DeviceId, DriverId, Finger};

/// A single detected minutia (MINDTCT output / BOZORTH3 input).
///
/// Maps to one column across libfprint's `xyt_struct` parallel arrays: `x`, `y`, and
/// `theta` (orientation, degrees). In the FP3 payload these are stored as three int32
/// arrays per enrolled sample (GVariant `(aiaiai)`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Minutia {
    /// Column in the source image, in pixels.
    pub x: i32,
    /// Row in the source image, in pixels.
    pub y: i32,
    /// Ridge orientation in degrees.
    pub theta: i32,
}

/// Enrollment date (libfprint serializes this as a Julian-day int32; `None` ⇒ the
/// `G_MININT32` "unset" sentinel).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EnrollDate {
    /// Gregorian year.
    pub year: i32,
    /// Month, `1..=12`.
    pub month: u8,
    /// Day of month, `1..=31`.
    pub day: u8,
}

/// The biometric payload of a print, matching libfprint's `FpiPrintType`.
///
/// `#[non_exhaustive]`: `FpiPrintType` is an external vocabulary that could grow, so adding a
/// payload kind must not be a breaking change for downstream matchers.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub enum Template {
    /// `FPI_PRINT_UNDEFINED` — a fresh print handed to `enroll` before it is filled in.
    #[default]
    Undefined,
    /// `FPI_PRINT_NBIS` — host-side minutiae comparison. One `Vec<Minutia>` per enrolled
    /// capture (image-capture sensors typically enroll several samples).
    Nbis(Vec<Vec<Minutia>>),
    /// `FPI_PRINT_RAW` — data compared directly. For match-on-chip devices this is the
    /// driver's opaque blob (often just a handle to a template stored on the sensor).
    ///
    /// Invariant relied on by `fprint-fp3` but not enforced here: the bytes are a
    /// *self-describing, standalone GVariant variant* (`v`) — the driver's `print->data` —
    /// which the codec writes and reads verbatim so a match-on-chip print round-trips
    /// byte-for-byte.
    Raw(Vec<u8>),
}

impl Template {
    /// True for match-on-chip / device-stored payloads that libfprint never runs through
    /// MINDTCT/BOZORTH3.
    #[must_use]
    pub fn is_raw(&self) -> bool {
        matches!(self, Template::Raw(_))
    }
}

/// A fingerprint print: biometric [`Template`] plus the metadata libfprint serializes
/// alongside it.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Print {
    /// The biometric payload. [`Template::Undefined`] until an enrollment fills it in.
    pub template: Template,
    /// Which finger this is, when known.
    pub finger: Option<Finger>,
    /// Owning user, as fprintd stores it under `/var/lib/fprint/<user>/`.
    pub username: Option<String>,
    /// Free-form label libfprint carries alongside the template.
    pub description: Option<String>,
    /// Driver id this template is bound to (templates are device/driver-specific).
    pub driver: Option<DriverId>,
    /// The specific reader this template came from, when the driver binds that tightly.
    pub device_id: Option<DeviceId>,
    /// True when the real template lives on the sensor and this `Print` is only a handle
    /// (`fpi_print_set_device_stored`). Always true for MOC prints.
    pub device_stored: bool,
    /// When the print was enrolled, when the backend records it.
    pub enroll_date: Option<EnrollDate>,
}

impl Print {
    /// A blank print to hand to [`crate::Device::enroll`], tagged with the target finger.
    #[must_use]
    pub fn new_for_enroll(finger: Finger) -> Print {
        Print {
            finger: Some(finger),
            ..Print::default()
        }
    }

    /// Whether this print's template is compatible with a device advertising `driver`.
    ///
    /// Corresponds to libfprint's `fp_print_compatible`, which additionally checks
    /// `device_id` for some drivers; those transport-specific rules are left to the backend.
    #[must_use]
    pub fn is_compatible_with_driver(&self, driver: &DriverId) -> bool {
        match &self.driver {
            Some(d) => d == driver,
            None => true, // not yet bound
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Minutia, Print, Template};
    use crate::{DriverId, Finger};

    /// **A print built by [`Print::new_for_enroll`] carries its finger and nothing else.**
    ///
    /// The `Undefined` template is a cross-crate contract stated from this side: it is the one
    /// `Print` that `fprint_fp3::to_bytes` must reject, because there is no biometric payload to
    /// serialize. Pinned here as a shape, without depending on the codec.
    #[test]
    fn new_for_enroll_sets_the_finger_and_leaves_the_template_undefined() {
        for finger in Finger::ALL.iter().chain(&[Finger::Unknown]) {
            let print = Print::new_for_enroll(*finger);
            assert_eq!(print.finger, Some(*finger));
            assert_eq!(print.template, Template::Undefined);
            assert!(!print.template.is_raw());
            // Everything a backend fills in later is still absent.
            assert_eq!(print.driver, None);
            assert_eq!(print.device_id, None);
            assert_eq!(print.username, None);
            assert_eq!(print.description, None);
            assert_eq!(print.enroll_date, None);
            assert!(!print.device_stored);
        }
    }

    /// An unbound print is compatible with every driver — a documented decision, so it is tested
    /// rather than left to be rediscovered by whoever changes the `None` arm.
    #[test]
    fn an_unbound_print_is_compatible_with_any_driver() {
        let print = Print::new_for_enroll(Finger::LeftIndex);
        assert_eq!(print.driver, None);
        for driver in ["goodixmoc", "synaptics", ""] {
            assert!(print.is_compatible_with_driver(&DriverId(driver.to_string())));
        }
    }

    /// A bound print is compatible with its own driver and no other.
    #[test]
    fn a_bound_print_is_compatible_only_with_its_own_driver() {
        let print = Print {
            driver: Some(DriverId("goodixmoc".to_string())),
            ..Print::new_for_enroll(Finger::LeftIndex)
        };
        assert!(print.is_compatible_with_driver(&DriverId("goodixmoc".to_string())));
        for other in ["synaptics", "GOODIXMOC", "goodixmoc2", ""] {
            assert!(
                !print.is_compatible_with_driver(&DriverId(other.to_string())),
                "{other:?} must not match goodixmoc"
            );
        }
    }

    /// `is_raw` selects the match-on-chip payload and only it.
    #[test]
    fn is_raw_holds_only_for_the_raw_template() {
        assert!(Template::Raw(Vec::new()).is_raw());
        assert!(Template::Raw(b"handle".to_vec()).is_raw());
        assert!(!Template::Undefined.is_raw());
        assert!(!Template::default().is_raw());
        assert!(!Template::Nbis(Vec::new()).is_raw());
        assert!(!Template::Nbis(vec![vec![Minutia {
            x: 1,
            y: 2,
            theta: 3
        }]])
        .is_raw());
    }

    /// `Template::default()` is `Undefined`: [`Print::default`] must not invent a payload.
    #[test]
    fn the_default_template_is_undefined() {
        assert_eq!(Template::default(), Template::Undefined);
        assert_eq!(Print::default().template, Template::Undefined);
        assert_eq!(Print::default().finger, None);
    }
}
