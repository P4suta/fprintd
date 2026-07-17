// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Edge translation between libfprint's glib vocabulary and the pure `fprint-core` model.
//!
//! Everything wire/quirk-specific about the C library — the `GQuark` error domains, the
//! `FpDeviceError`/`FpDeviceRetry` code tables, the `FpFinger`/`FpScanType`/`FpDeviceFeature`
//! enum discriminants — is confined here so it never leaks into the domain model
//! (`ARCHITECTURE.md` principle 3). These are interoperability *facts* (enum values, quark
//! strings), not copyrightable expression, so documenting and matching them is clean. The raw
//! object access lives in [`crate::ffi`]; this module reads the primitive values it returns.

use fprint_core::{
    DeviceFeature, DeviceId, DeviceInfo, DriverId, Error, Finger, FingerStatus, RetryReason,
    ScanType, Temperature,
};
use fprint_fp3::Fp3Error;
use glib::error::ErrorDomain;
use glib::Quark;

use crate::ffi::FpDevice;

// --- libfprint error domains, expressed as glib `ErrorDomain`s ------------------------
//
// libfprint ships `FpDeviceError`/`FpDeviceRetry` as plain C enums under two `GQuark` domains,
// not as typed glib error domains, so these zero-cost markers stand in.
// `glib::Error::kind::<T>()` returns the code iff the error's `GQuark` domain equals
// `T::domain()`, giving safe, allocation-free classification with no raw-pointer access to the
// `GError`. The quark strings come from libfprint's `G_DEFINE_QUARK (fp - device - error - quark,
// …)` / `(fp - device - retry - quark, …)`.

#[derive(Clone, Copy)]
struct FpDeviceErrorCode(i32);

impl ErrorDomain for FpDeviceErrorCode {
    fn domain() -> Quark {
        Quark::from_str("fp-device-error-quark")
    }
    fn code(self) -> i32 {
        self.0
    }
    fn from(code: i32) -> Option<Self> {
        Some(FpDeviceErrorCode(code))
    }
}

#[derive(Clone, Copy)]
struct FpDeviceRetryCode(i32);

impl ErrorDomain for FpDeviceRetryCode {
    fn domain() -> Quark {
        Quark::from_str("fp-device-retry-quark")
    }
    fn code(self) -> i32 {
        self.0
    }
    fn from(code: i32) -> Option<Self> {
        Some(FpDeviceRetryCode(code))
    }
}

// `FpDeviceError` enum values (fp-device.h). Named locally so the mapping reads as a spec.
// (GENERAL = 0 and TOO_HOT = 0x101 are intentionally absent: they fall through to `Other`.)
const FP_ERR_NOT_SUPPORTED: i32 = 1;
const FP_ERR_NOT_OPEN: i32 = 2;
const FP_ERR_ALREADY_OPEN: i32 = 3;
const FP_ERR_BUSY: i32 = 4;
const FP_ERR_PROTO: i32 = 5;
const FP_ERR_DATA_INVALID: i32 = 6;
const FP_ERR_DATA_NOT_FOUND: i32 = 7;
const FP_ERR_DATA_FULL: i32 = 8;
const FP_ERR_DATA_DUPLICATE: i32 = 9;
const FP_ERR_REMOVED: i32 = 0x100;

// `FpDeviceRetry` enum values (fp-device.h). (GENERAL = 0 falls through to the general reason.)
const FP_RETRY_TOO_SHORT: i32 = 1;
const FP_RETRY_CENTER_FINGER: i32 = 2;
const FP_RETRY_REMOVE_FINGER: i32 = 3;
const FP_RETRY_TOO_FAST: i32 = 4;

/// Classify a `glib::Error` from any libfprint sync call into the core [`Error`] vocabulary.
pub(crate) fn from_gerror(err: glib::Error) -> Error {
    if let Some(retry) = err.kind::<FpDeviceRetryCode>() {
        return Error::RetryScan(map_retry(retry.0));
    }
    if let Some(dev_err) = err.kind::<FpDeviceErrorCode>() {
        return map_device_error(dev_err.0, &err);
    }
    if err.matches(gio::IOErrorEnum::Cancelled) {
        return Error::Cancelled;
    }
    Error::Other(err.message().to_owned())
}

fn map_device_error(code: i32, err: &glib::Error) -> Error {
    match code {
        FP_ERR_NOT_SUPPORTED => Error::NotSupported,
        FP_ERR_NOT_OPEN | FP_ERR_ALREADY_OPEN => Error::ProtoState,
        FP_ERR_BUSY => Error::Busy,
        FP_ERR_PROTO | FP_ERR_DATA_INVALID => Error::Protocol(err.message().to_owned()),
        FP_ERR_DATA_NOT_FOUND => Error::DataNotFound,
        FP_ERR_DATA_FULL => Error::DataFull,
        FP_ERR_DATA_DUPLICATE => Error::DataDuplicate,
        FP_ERR_REMOVED => Error::NotFound,
        // FP_ERR_GENERAL, FP_ERR_TOO_HOT and any future code carry the human-readable message.
        _ => Error::Other(err.message().to_owned()),
    }
}

fn map_retry(code: i32) -> RetryReason {
    match code {
        FP_RETRY_TOO_SHORT => RetryReason::TooShort,
        FP_RETRY_CENTER_FINGER => RetryReason::NotCentered,
        FP_RETRY_REMOVE_FINGER => RetryReason::RemoveAndRetry,
        FP_RETRY_TOO_FAST => RetryReason::TooFast,
        // FP_RETRY_GENERAL and anything unrecognised degrade to the general retry reason.
        _ => RetryReason::General,
    }
}

/// A retry reason for a failed enroll capture, or `None` if `err` is not a retry-class error.
pub(crate) fn gerror_retry(err: &glib::Error) -> Option<RetryReason> {
    err.kind::<FpDeviceRetryCode>().map(|c| map_retry(c.0))
}

/// Map a failure of the FP3 codec (round-tripping templates through the shim) to [`Error`].
pub(crate) fn from_fp3(err: Fp3Error) -> Error {
    Error::Protocol(err.to_string())
}

// --- Enum translation -----------------------------------------------------------------

// Note: the reverse (`FpFinger` → `Finger`) is deliberately absent. A decoded print's finger
// comes from the FP3 blob via `fprint-fp3` (the single source of truth, per the D1 decision in
// `print.rs`), so no separate live-getter translation is needed.

/// core [`Finger`] → libfprint's raw `FpFinger` value.
pub(crate) fn finger_to_fp(f: Finger) -> libfprint_sys::FpFinger {
    match f {
        Finger::Unknown => libfprint_sys::FpFinger_FP_FINGER_UNKNOWN,
        Finger::LeftThumb => libfprint_sys::FpFinger_FP_FINGER_LEFT_THUMB,
        Finger::LeftIndex => libfprint_sys::FpFinger_FP_FINGER_LEFT_INDEX,
        Finger::LeftMiddle => libfprint_sys::FpFinger_FP_FINGER_LEFT_MIDDLE,
        Finger::LeftRing => libfprint_sys::FpFinger_FP_FINGER_LEFT_RING,
        Finger::LeftLittle => libfprint_sys::FpFinger_FP_FINGER_LEFT_LITTLE,
        Finger::RightThumb => libfprint_sys::FpFinger_FP_FINGER_RIGHT_THUMB,
        Finger::RightIndex => libfprint_sys::FpFinger_FP_FINGER_RIGHT_INDEX,
        Finger::RightMiddle => libfprint_sys::FpFinger_FP_FINGER_RIGHT_MIDDLE,
        Finger::RightRing => libfprint_sys::FpFinger_FP_FINGER_RIGHT_RING,
        Finger::RightLittle => libfprint_sys::FpFinger_FP_FINGER_RIGHT_LITTLE,
    }
}

/// Read the device's scan type. `FpScanType`'s discriminants *are* libfprint's raw
/// `FP_SCAN_TYPE_*` values.
pub(crate) fn scan_type(dev: &FpDevice) -> ScanType {
    if dev.scan_type() == libfprint_sys::FpScanType_FP_SCAN_TYPE_PRESS {
        ScanType::Press
    } else {
        ScanType::Swipe
    }
}

/// Fold the device's capability set into a core [`DeviceFeature`] bitmask.
///
/// libfprint's `FpDeviceFeature` bit positions are mirrored exactly by [`DeviceFeature`], so the
/// raw bitmask reconstructs the same mask.
pub(crate) fn features(dev: &FpDevice) -> DeviceFeature {
    DeviceFeature::from_bits_truncate(dev.features())
}

/// Read the device's sensor temperature (`FpTemperature`) as a core [`Temperature`], or `None`
/// for an unrecognised value. The worker publishes this after each job to the device handle's
/// shared cell, which the live [`Device::temperature`](fprint_core::Device::temperature) getter
/// reads without a round-trip to the `!Send` device.
pub(crate) fn temperature(dev: &FpDevice) -> Option<Temperature> {
    match dev.temperature() {
        libfprint_sys::FpTemperature_FP_TEMPERATURE_COLD => Some(Temperature::Cold),
        libfprint_sys::FpTemperature_FP_TEMPERATURE_WARM => Some(Temperature::Warm),
        libfprint_sys::FpTemperature_FP_TEMPERATURE_HOT => Some(Temperature::Hot),
        _ => None,
    }
}

/// Read the device's live finger-presence status (`FpFingerStatusFlags`) as a core
/// [`FingerStatus`] bitmask.
pub(crate) fn finger_status(dev: &FpDevice) -> FingerStatus {
    let bits = dev.finger_status();
    let mut status = FingerStatus::NONE;
    if bits & libfprint_sys::FpFingerStatusFlags_FP_FINGER_STATUS_NEEDED != 0 {
        status |= FingerStatus::NEEDED;
    }
    if bits & libfprint_sys::FpFingerStatusFlags_FP_FINGER_STATUS_PRESENT != 0 {
        status |= FingerStatus::PRESENT;
    }
    status
}

/// Build the static [`DeviceInfo`] from a device's getters.
///
/// The virtual (and some real) devices report an empty `device_id`; we fall back to the
/// driver id so the identifier is still stable and non-empty for [`crate::LibfprintBackend`]'s
/// open-by-id lookup. Thermal state is *not* part of this static shape — it is a live reading via
/// [`Device::temperature`](fprint_core::Device::temperature), published from the worker by
/// [`temperature`].
pub(crate) fn device_info(dev: &FpDevice) -> DeviceInfo {
    let driver = dev.driver();
    let device_id = dev.device_id();
    let id = if device_id.is_empty() {
        driver.clone()
    } else {
        device_id
    };
    DeviceInfo::new(
        DeviceId::new(id),
        DriverId::new(driver.clone()),
        dev.name().unwrap_or(driver),
        scan_type(dev),
        features(dev),
        dev.nr_enroll_stages().max(0) as u32,
    )
}
