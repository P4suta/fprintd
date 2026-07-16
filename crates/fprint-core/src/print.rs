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
///
/// The `fprint-mindtct` and `fprint-bozorth3` kernels stay dependency-free and define their own
/// `Minutia` of the same shape; a backend maps between them at the boundary (via [`Self::from_xyt`] /
/// [`Self::as_xyt`]). The derives match those kernels' so the three types line up.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Minutia {
    /// Column in the source image, in pixels.
    pub x: i32,
    /// Row in the source image, in pixels.
    pub y: i32,
    /// Ridge orientation in degrees.
    pub theta: i32,
}

impl Minutia {
    /// Construct a minutia from the `xyt` triple that crosses every kernel boundary.
    #[must_use]
    pub const fn from_xyt(x: i32, y: i32, theta: i32) -> Self {
        Self { x, y, theta }
    }

    /// The `(x, y, theta)` triple, for handing this minutia to a matcher or detector that names
    /// the same interoperability fact.
    #[must_use]
    pub const fn as_xyt(&self) -> (i32, i32, i32) {
        (self.x, self.y, self.theta)
    }
}

impl From<(i32, i32, i32)> for Minutia {
    fn from((x, y, theta): (i32, i32, i32)) -> Self {
        Self { x, y, theta }
    }
}

/// Enrollment date (libfprint serializes this as a Julian-day int32; `None` ⇒ the
/// `G_MININT32` "unset" sentinel).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct EnrollDate {
    /// Gregorian year.
    pub year: i32,
    /// Month, `1..=12`.
    pub month: u8,
    /// Day of month, `1..=31`.
    pub day: u8,
}

impl EnrollDate {
    /// An enrollment date from its Gregorian `year`, `month`, and `day`.
    #[must_use]
    pub fn new(year: i32, month: u8, day: u8) -> Self {
        EnrollDate { year, month, day }
    }
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
#[non_exhaustive]
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

    /// A [`PrintBuilder`] with every field unset, the canonical way to construct a `Print`.
    #[must_use]
    pub fn builder() -> PrintBuilder {
        PrintBuilder::default()
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

/// Builder for [`Print`], one setter per field, terminated by [`build`](PrintBuilder::build).
///
/// `Print::default()` with field writes stays available; this is the fluent construction path
/// and the one that survives new fields on `Print`.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct PrintBuilder {
    template: Template,
    finger: Option<Finger>,
    username: Option<String>,
    description: Option<String>,
    driver: Option<DriverId>,
    device_id: Option<DeviceId>,
    device_stored: bool,
    enroll_date: Option<EnrollDate>,
}

impl PrintBuilder {
    /// Set the biometric payload.
    #[must_use]
    pub fn template(mut self, template: Template) -> Self {
        self.template = template;
        self
    }

    /// Set which finger this is.
    #[must_use]
    pub fn finger(mut self, finger: Option<Finger>) -> Self {
        self.finger = finger;
        self
    }

    /// Set the owning user.
    #[must_use]
    pub fn username(mut self, username: Option<String>) -> Self {
        self.username = username;
        self
    }

    /// Set the free-form label.
    #[must_use]
    pub fn description(mut self, description: Option<String>) -> Self {
        self.description = description;
        self
    }

    /// Set the driver this template is bound to.
    #[must_use]
    pub fn driver(mut self, driver: Option<DriverId>) -> Self {
        self.driver = driver;
        self
    }

    /// Set the specific reader this template came from.
    #[must_use]
    pub fn device_id(mut self, device_id: Option<DeviceId>) -> Self {
        self.device_id = device_id;
        self
    }

    /// Set whether the real template lives on the sensor.
    #[must_use]
    pub fn device_stored(mut self, device_stored: bool) -> Self {
        self.device_stored = device_stored;
        self
    }

    /// Set the enrollment date.
    #[must_use]
    pub fn enroll_date(mut self, enroll_date: Option<EnrollDate>) -> Self {
        self.enroll_date = enroll_date;
        self
    }

    /// Finish building the [`Print`].
    #[must_use]
    pub fn build(self) -> Print {
        Print {
            template: self.template,
            finger: self.finger,
            username: self.username,
            description: self.description,
            driver: self.driver,
            device_id: self.device_id,
            device_stored: self.device_stored,
            enroll_date: self.enroll_date,
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
            assert!(print.is_compatible_with_driver(&DriverId::new(driver)));
        }
    }

    /// A bound print is compatible with its own driver and no other.
    #[test]
    fn a_bound_print_is_compatible_only_with_its_own_driver() {
        let print = Print {
            driver: Some(DriverId::new("goodixmoc")),
            ..Print::new_for_enroll(Finger::LeftIndex)
        };
        assert!(print.is_compatible_with_driver(&DriverId::new("goodixmoc")));
        for other in ["synaptics", "GOODIXMOC", "goodixmoc2", ""] {
            assert!(
                !print.is_compatible_with_driver(&DriverId::new(other)),
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
