// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Edge translation between libfprint's glib vocabulary and the pure `fprint-core` model.
//!
//! Everything wire/quirk-specific about the C library — the `GQuark` error domains, the
//! `FpDeviceError`/`FpDeviceRetry` code tables, the `FpFinger`/`FpScanType`/`FpDeviceFeature`
//! enum discriminants — is confined here so it never leaks into the domain model
//! (`ARCHITECTURE.md` principle 3). These are interoperability *facts* (enum values, quark
//! strings), not copyrightable expression, so documenting and matching them is clean.

use fprint_core::{
    DeviceFeature, DeviceId, DeviceInfo, DriverId, Error, Finger, RetryReason, ScanType,
};
use fprint_fp3::Fp3Error;
use glib::error::ErrorDomain;
use glib::Quark;
use libfprint_rs::{FpDevice, FpFinger, GError};

// --- libfprint error domains, expressed as glib `ErrorDomain`s ------------------------
//
// The binding does not wrap `FpDeviceError`/`FpDeviceRetry` as typed glib error domains, so
// these zero-cost markers stand in. `glib::Error::kind::<T>()` returns the code iff the error's
// `GQuark` domain equals `T::domain()`, giving safe, allocation-free classification with no
// raw-pointer access to the `GError`. The quark strings come from libfprint's
// `G_DEFINE_QUARK (fp - device - error - quark, …)` / `(fp - device - retry - quark, …)`.

// UPSTREAM(libfprint-rs 0.3.1): FpDeviceError/FpDeviceRetry are not exported as typed glib error domains — remove when fixed; see docs/known-issues.md
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

/// Classify a `GError` from any libfprint sync call into the core [`Error`] vocabulary.
pub fn from_gerror(err: GError) -> Error {
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

fn map_device_error(code: i32, err: &GError) -> Error {
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
pub fn gerror_retry(err: &GError) -> Option<RetryReason> {
    err.kind::<FpDeviceRetryCode>().map(|c| map_retry(c.0))
}

/// Map a failure of the FP3 codec (round-tripping templates through the binding) to [`Error`].
pub fn from_fp3(err: Fp3Error) -> Error {
    Error::Protocol(err.to_string())
}

// --- Enum translation -----------------------------------------------------------------

// Note: the reverse (`FpFinger` → `Finger`) is deliberately absent. A decoded print's finger
// comes from the FP3 blob via `fprint-fp3` (the single source of truth, per the D1 decision in
// `print.rs`), so no separate live-getter translation is needed.

/// core [`Finger`] → libfprint `FpFinger`.
pub fn finger_to_fp(f: Finger) -> FpFinger {
    match f {
        Finger::Unknown => FpFinger::Unknown,
        Finger::LeftThumb => FpFinger::LeftThumb,
        Finger::LeftIndex => FpFinger::LeftIndex,
        Finger::LeftMiddle => FpFinger::LeftMiddle,
        Finger::LeftRing => FpFinger::LeftRing,
        Finger::LeftLittle => FpFinger::LeftLittle,
        Finger::RightThumb => FpFinger::RightThumb,
        Finger::RightIndex => FpFinger::RightIndex,
        Finger::RightMiddle => FpFinger::RightMiddle,
        Finger::RightRing => FpFinger::RightRing,
        Finger::RightLittle => FpFinger::RightLittle,
    }
}

/// Read the device's scan type. `FpScanType`'s discriminants *are* libfprint's raw
/// `FP_SCAN_TYPE_*` values, so an `as`-cast avoids naming the binding's non-exported enum.
pub fn scan_type(dev: &FpDevice) -> ScanType {
    let raw = dev.scan_type() as u32;
    if raw == libfprint_sys::FpScanType_FP_SCAN_TYPE_PRESS {
        ScanType::Press
    } else {
        ScanType::Swipe
    }
}

/// Fold the device's capability set into a core [`DeviceFeature`] bitmask.
///
/// libfprint's `FpDeviceFeature` bit positions are mirrored exactly by [`DeviceFeature`], so
/// OR-ing each variant's discriminant reconstructs the same mask.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub fn features(dev: &FpDevice) -> DeviceFeature {
    // `FpDeviceFeature` is not re-exported by the binding, but each value is castable to its
    // FP_DEVICE_FEATURE_* bit without naming the type.
    let mut bits: u32 = 0;
    for f in dev.features() {
        bits |= f as u32;
    }
    DeviceFeature::from_bits_truncate(bits)
}

/// Non-x86 fallback: the binding gates `features()` to x86, so read the raw bitmask directly.
// UPSTREAM(libfprint-rs 0.3.1): FpDevice::features() is cfg-gated to x86, so non-x86 must read the raw getter — remove when fixed; see docs/known-issues.md
#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
pub fn features(dev: &FpDevice) -> DeviceFeature {
    use glib::translate::ToGlibPtr;
    // SAFETY: `dev` is a live `FpDevice`; `fp_device_get_features` is a pure getter that only
    // reads the object and returns a bitmask, borrowing nothing.
    let bits = unsafe { libfprint_sys::fp_device_get_features(dev.to_glib_none().0) };
    DeviceFeature::from_bits_truncate(bits)
}

/// Read the device's human-readable name.
///
/// This bypasses `libfprint-rs` 0.3.1's [`FpDevice::name`], which wraps the *transfer-none*
/// `fp_device_get_name` with `from_glib_full` and so tries to free a string it does not own —
/// panicking/corrupting on some devices (the virtual driver among them). It is read here with
/// the correct transfer-none semantics. The binding's `driver()`/`device_id()` are fine.
// UPSTREAM(libfprint-rs 0.3.1): FpDevice::name wraps transfer-none fp_device_get_name with from_glib_full (double-free) — remove when fixed; see docs/known-issues.md
fn device_name(dev: &FpDevice) -> String {
    use glib::translate::{FromGlibPtrNone, ToGlibPtr};
    // SAFETY: `dev` is a live `FpDevice`; `fp_device_get_name` is a pure getter returning a
    // device-owned (transfer-none) C string, which `from_glib_none` copies without freeing.
    unsafe {
        let ptr = libfprint_sys::fp_device_get_name(dev.to_glib_none().0);
        if ptr.is_null() {
            dev.driver()
        } else {
            glib::GString::from_glib_none(ptr.cast::<std::os::raw::c_char>()).into()
        }
    }
}

/// Build the static [`DeviceInfo`] from a device's getters.
///
/// The virtual (and some real) devices report an empty `device_id`; we fall back to the
/// driver id so the identifier is still stable and non-empty for [`crate::LibfprintBackend`]'s
/// open-by-id lookup.
pub fn device_info(dev: &FpDevice) -> DeviceInfo {
    let driver = dev.driver();
    let device_id = dev.device_id();
    let id = if device_id.is_empty() {
        driver.clone()
    } else {
        device_id
    };
    DeviceInfo {
        id: DeviceId(id),
        driver: DriverId(driver),
        name: device_name(dev),
        scan_type: scan_type(dev),
        features: features(dev),
        enroll_stages: dev.nr_enroll_stage().max(0) as u32,
    }
}
