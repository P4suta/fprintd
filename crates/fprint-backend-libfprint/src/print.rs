// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Translation between libfprint's `FpPrint` and the core [`Print`].
//!
//! ## The D1 decision: templates are unified through `fprint-fp3`
//!
//! libfprint's `fp_print_serialize` and the `fprint-fp3` codec target the *same* on-disk FP3
//! byte format (`"FP3"` + GVariant `(issbymsmsia{sv}v)`). Every template therefore crosses this
//! boundary as an FP3 blob, with `fprint-fp3` as the single decoder: an `FpPrint` becomes a
//! [`Print`] by `fprint_fp3::from_bytes(fp.serialize())`, and back by
//! `FpPrint::deserialize(fprint_fp3::to_bytes(print))`. Match-on-chip handles (libfprint's
//! `FPI_PRINT_RAW`, the FP3 field-9 opaque variant) and host-side NBIS minutiae then land in the
//! same [`Template`](fprint_core::Template) shape the native backend produces, so the daemon
//! stores and compares prints uniformly regardless of which backend made them.

use fprint_core::{DeviceId, DriverId, Print, Result};

use crate::convert;
use crate::ffi::{FpDevice, FpPrint};

/// Decode an `FpPrint` (via its FP3 serialization) into a core [`Print`].
///
/// The blob is libfprint's own serialization, so it is authoritative for the template payload,
/// username/description and enroll date. Only the device-identity fields libfprint exposes as
/// live object getters are overlaid: the live object outranks (and, in practice, equals) the
/// decoded copy.
pub(crate) fn fp_to_core(fp: &FpPrint) -> Result<Print> {
    let blob = fp.serialize().map_err(convert::from_gerror)?;
    let mut print = fprint_fp3::from_bytes(&blob).map_err(convert::from_fp3)?;

    let driver = fp.driver();
    if !driver.is_empty() {
        print.driver = Some(DriverId::new(driver));
    }
    let device_id = fp.device_id();
    if !device_id.is_empty() {
        print.device_id = Some(DeviceId::new(device_id));
    }
    print.device_stored = fp.device_stored();

    Ok(print)
}

/// Build a fresh enrollment template on `dev` from a core [`Print`].
///
/// Only the metadata a caller can meaningfully supply before enrolling is copied. The enroll
/// date is deliberately left unset: libfprint stamps it during enrollment and the completed
/// print carries the authoritative value in its blob, so mapping core's `y/m/d` into a
/// `glib::Date` here would be lossy work for a value that is immediately discarded.
pub(crate) fn core_to_fp(print: &Print, dev: &FpDevice) -> FpPrint {
    let fp = FpPrint::new(dev);
    if let Some(finger) = print.finger {
        fp.set_finger(convert::finger_to_fp(finger));
    }
    if let Some(username) = &print.username {
        fp.set_username(username);
    }
    if let Some(description) = &print.description {
        fp.set_description(description);
    }
    fp
}

/// Reconstruct a stored/enrolled `FpPrint` from a core [`Print`] for use as a match candidate
/// (the enrolled side of `verify`, or a gallery entry of `identify`).
pub(crate) fn core_to_fp_for_match(print: &Print) -> Result<FpPrint> {
    let blob = fprint_fp3::to_bytes(print).map_err(convert::from_fp3)?;
    FpPrint::deserialize(&blob).map_err(convert::from_gerror)
}
