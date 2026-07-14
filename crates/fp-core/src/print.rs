// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Fingerprint prints/templates — the domain model behind libfprint's `FpPrint` and the
//! on-disk FP3 format.
//!
//! A [`Print`] is metadata plus a [`Template`] payload. Serialization to/from the FP3
//! byte format (`"FP3"` magic + GVariant `(issbymsmsia{sv}v)`) lives in a downstream
//! crate (it needs a GVariant encoder, e.g. `zvariant`); this module is the in-memory
//! model those (de)serializers map to.

use crate::{DeviceId, DriverId, Finger};

/// A single detected minutia (MINDTCT output / BOZORTH3 input).
///
/// Maps to one column across libfprint's `xyt_struct` parallel arrays: `x`, `y`, and
/// `theta` (orientation, degrees). In the FP3 payload these are stored as three int32
/// arrays per enrolled sample (GVariant `(aiaiai)`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Minutia {
    pub x: i32,
    pub y: i32,
    /// Ridge orientation in degrees.
    pub theta: i32,
}

/// Enrollment date (libfprint serializes this as a Julian-day int32; `None` ⇒ the
/// `G_MININT32` "unset" sentinel).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EnrollDate {
    pub year: i32,
    pub month: u8,
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
    /// Invariant (spoken by the FP3 edge, not enforced here): the bytes are a *self-describing,
    /// standalone GVariant variant* (`v`) — the driver's `print->data` — which the codec in
    /// `fp-fp3` writes and reads verbatim so a match-on-chip print round-trips byte-for-byte.
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
    pub template: Template,
    pub finger: Option<Finger>,
    pub username: Option<String>,
    pub description: Option<String>,
    /// Driver id this template is bound to (templates are device/driver-specific).
    pub driver: Option<DriverId>,
    pub device_id: Option<DeviceId>,
    /// True when the real template lives on the sensor and this `Print` is only a handle
    /// (`fpi_print_set_device_stored`). Always true for MOC prints.
    pub device_stored: bool,
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
    /// A first-cut of libfprint's `fp_print_compatible` (which also checks device_id for
    /// some drivers); the transport-specific rules will be refined per backend.
    #[must_use]
    pub fn is_compatible_with_driver(&self, driver: &DriverId) -> bool {
        match &self.driver {
            Some(d) => d == driver,
            None => true, // not yet bound
        }
    }
}
