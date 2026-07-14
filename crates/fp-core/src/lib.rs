// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! # fp-core
//!
//! The GObject-free, idiomatic-Rust core of an fprintd-compatible fingerprint stack.
//!
//! This crate is the "spiritual modern libfprint": the domain model (fingers, device
//! capabilities, prints/templates) plus the [`Backend`]/[`Device`] traits that a
//! concrete backend implements. It contains **no** device drivers, **no** USB code, and
//! **no** matching algorithms — those live in downstream crates so that:
//!
//! * `fp-backend-libfprint` can implement [`Backend`] by wrapping the C libfprint (the
//!   M1 shim), and
//! * `fp-backend-native` can implement the same trait with pure-Rust drivers + transport,
//!
//! and the fprintd-compatible daemon (`fprintd-rs`) depends only on these traits, so the
//! backend can be swapped without touching the daemon.
//!
//! Enum *values* that cross a wire boundary (the FP3 template format, fprintd's per-finger
//! file names) mirror libfprint's C enums exactly — see [`Finger`] and [`feature`] — so the
//! stack stays interoperable with existing `/var/lib/fprint` stores. The wire *vocabularies*
//! themselves (the `net.reactivated.Fprint` finger-name and status strings) are not modeled
//! here; they live at the daemon edge (`ARCHITECTURE.md` principle 3).

#![forbid(unsafe_code)]

mod error;
mod finger;
mod feature;
mod print;
mod device;

pub use error::{Error, RetryReason, Result};
pub use finger::Finger;
pub use feature::{DeviceFeature, FingerStatus, ScanType, Temperature};
pub use print::{EnrollDate, Minutia, Print, Template};
pub use device::{
    Backend, Device, DeviceId, DeviceInfo, DriverId, EnrollProgress, IdentifyOutcome,
    VerifyOutcome,
};
