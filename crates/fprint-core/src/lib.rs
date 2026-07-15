// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! # fprint-core
//!
//! The GObject-free core of an fprintd-compatible fingerprint stack: the domain model
//! (fingers, device capabilities, prints/templates) plus the [`Backend`]/[`Device`] traits
//! that a concrete backend implements.
//!
//! It contains **no** device drivers, **no** USB code, and **no** matching algorithms —
//! those live in downstream crates:
//!
//! * `fprint-backend-libfprint` implements [`Backend`] by wrapping the C libfprint, and
//! * `fprint-backend-native` implements the same trait with pure-Rust drivers + transport.
//!
//! The fprintd-compatible daemon (`fprintd`) depends only on these traits, so the backend
//! can be swapped without touching the daemon.
//!
//! Enum *values* that cross a wire boundary (the FP3 template format, fprintd's per-finger
//! file names) mirror libfprint's C enums exactly — see [`Finger`] — so the stack stays
//! interoperable with existing `/var/lib/fprint` stores. The device-capability enums
//! ([`DeviceFeature`], [`ScanType`]) mirror their libfprint counterparts too. The wire
//! *vocabularies* themselves (the `net.reactivated.Fprint` finger-name and status strings)
//! live at the daemon edge, not here (`ARCHITECTURE.md` principle 3).

#![forbid(unsafe_code)]

mod device;
mod error;
mod feature;
mod finger;
mod print;

pub use device::{
    Backend, Device, DeviceId, DeviceInfo, DriverId, EnrollProgress, IdentifyOutcome, VerifyOutcome,
};
pub use error::{Error, Result, RetryReason};
pub use feature::{DeviceFeature, FingerStatus, ScanType, Temperature};
pub use finger::Finger;
pub use print::{EnrollDate, Minutia, Print, Template};
