// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! On-device storage — the second `unsafe` island.
//!
//! The `libfprint-rs` 0.3.1 wrappers for `list_prints`/`delete_print`/`clear_storage` are
//! `unimplemented!()`, so we call the C entry points (`fp_device_*_sync`, `fp-device.h`)
//! directly through `libfprint-sys` and translate GLib ownership at the boundary. Each verb
//! is only reachable once [`crate::LibfprintDevice`] has checked the matching
//! [`DeviceFeature`](fprint_core::DeviceFeature); the extra `NotSupported`-shaped guarding lives
//! there, keeping these functions a thin, honest FFI seam.

// UPSTREAM(libfprint-rs 0.3.1): list_prints/delete_print/clear_storage wrappers are unimplemented!(), so we call the raw fp_device_*_sync — remove when fixed; see docs/known-issues.md
use fprint_core::Result;
use gio::Cancellable;
use glib::translate::{FromGlibPtrContainer, FromGlibPtrFull, ToGlibPtr};
use libfprint_rs::{FpDevice, FpPrint};

use crate::convert;

/// The `gio` and `libfprint-sys` crates each carry their own bindgen `GCancellable` type, so
/// the glib-native pointer must be cast across the two. Naming the `gio::ffi` pointer type
/// explicitly also disambiguates `ToGlibPtr` (which is implemented for several pointer forms).
fn raw_cancel(cancel: Option<&Cancellable>) -> *mut libfprint_sys::GCancellable {
    match cancel {
        Some(c) => {
            let p: *mut gio::ffi::GCancellable = c.to_glib_none().0;
            p.cast()
        }
        None => std::ptr::null_mut(),
    }
}

/// List the templates stored on the sensor (`STORAGE_LIST` devices).
pub fn list(dev: &FpDevice, cancel: Option<&Cancellable>) -> Result<Vec<FpPrint>> {
    let raw_cancel = raw_cancel(cancel);
    let mut error: *mut libfprint_sys::GError = std::ptr::null_mut();

    // SAFETY: `fp_device_list_prints_sync` takes a live device, an optional cancellable and an
    // out-error slot (fp-device.h). It returns a transfer-full `GPtrArray` of `FpPrint*` on
    // success, or NULL with `error` set on failure.
    let array = unsafe {
        libfprint_sys::fp_device_list_prints_sync(
            dev.to_glib_none().0,
            raw_cancel,
            std::ptr::addr_of_mut!(error),
        )
    };

    if array.is_null() {
        return Err(convert::from_gerror(unsafe {
            glib::Error::from_glib_full(error.cast())
        }));
    }

    // SAFETY: `array` is a transfer-full `GPtrArray` whose elements are `FpPrint*` with a
    // `g_object_unref` free-func; `from_glib_full` assumes ownership of container and elements.
    let prints: Vec<FpPrint> =
        unsafe { FromGlibPtrContainer::from_glib_full(array.cast::<glib::ffi::GPtrArray>()) };
    Ok(prints)
}

/// Delete one stored template by its device-side handle (`STORAGE_DELETE` devices).
pub fn delete(dev: &FpDevice, print: &FpPrint, cancel: Option<&Cancellable>) -> Result<()> {
    let raw_cancel = raw_cancel(cancel);
    let mut error: *mut libfprint_sys::GError = std::ptr::null_mut();

    // SAFETY: `fp_device_delete_print_sync` takes live device + print pointers, an optional
    // cancellable and an out-error slot; it returns FALSE with `error` set on failure.
    let ok = unsafe {
        libfprint_sys::fp_device_delete_print_sync(
            dev.to_glib_none().0,
            print.to_glib_none().0,
            raw_cancel,
            std::ptr::addr_of_mut!(error),
        )
    };

    if ok == glib::ffi::GFALSE {
        return Err(convert::from_gerror(unsafe {
            glib::Error::from_glib_full(error.cast())
        }));
    }
    Ok(())
}

/// Erase all templates from the sensor (`STORAGE_CLEAR` devices).
pub fn clear(dev: &FpDevice, cancel: Option<&Cancellable>) -> Result<()> {
    let raw_cancel = raw_cancel(cancel);
    let mut error: *mut libfprint_sys::GError = std::ptr::null_mut();

    // SAFETY: `fp_device_clear_storage_sync` takes a live device, an optional cancellable and
    // an out-error slot; it returns FALSE with `error` set on failure.
    let ok = unsafe {
        libfprint_sys::fp_device_clear_storage_sync(
            dev.to_glib_none().0,
            raw_cancel,
            std::ptr::addr_of_mut!(error),
        )
    };

    if ok == glib::ffi::GFALSE {
        return Err(convert::from_gerror(unsafe {
            glib::Error::from_glib_full(error.cast())
        }));
    }
    Ok(())
}
