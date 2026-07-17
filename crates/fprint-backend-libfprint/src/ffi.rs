// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Rust ownership over the C libfprint objects.
//!
//! The shim links libfprint through `libfprint-sys` (raw bindgen) and owns the GObject lifetime
//! itself: [`FpContext`], [`FpDevice`] and [`FpPrint`] are thin [`glib::wrapper!`] types whose
//! ref/unref — and, for the floating [`FpPrint`], ref-sink — is handled by glib's translate
//! traits. Every `unsafe` block below names the libfprint entry point it calls and the transfer
//! semantics it relies on; outside this module only the same-thread callback trampolines in
//! [`crate::progress`] touch a raw pointer.

use std::os::raw::c_void;
use std::ptr;

use gio::Cancellable;
use glib::translate::{
    from_glib_full, from_glib_none, FromGlibContainer, FromGlibPtrContainer, FromGlibPtrFull,
    FromGlibPtrNone, ToGlibContainerFromSlice, ToGlibPtr,
};

glib::wrapper! {
    /// libfprint's discovery root (`FpContext`).
    pub(crate) struct FpContext(Object<libfprint_sys::FpContext, libfprint_sys::FpContextClass>);

    match fn {
        type_ => || libfprint_sys::fp_context_get_type() as usize,
    }
}

glib::wrapper! {
    /// A fingerprint reader (`FpDevice`).
    pub(crate) struct FpDevice(Object<libfprint_sys::FpDevice, libfprint_sys::FpDeviceClass>);

    match fn {
        type_ => || libfprint_sys::fp_device_get_type() as usize,
    }
}

glib::wrapper! {
    /// A fingerprint template (`FpPrint`). It derives from `GInitiallyUnowned`, so a fresh one
    /// carries a floating reference that [`FpPrint::new`] sinks.
    pub(crate) struct FpPrint(Object<libfprint_sys::FpPrint, libfprint_sys::FpPrintClass>)
        @extends glib::object::InitiallyUnowned;

    match fn {
        type_ => || libfprint_sys::fp_print_get_type() as usize,
    }
}

/// `gio` and `libfprint-sys` each carry their own bindgen `GCancellable`, so the glib-native
/// pointer is cast across the two. Returns NULL for `None` (libfprint reads that as "no
/// cancellable"). The pointer borrows `cancel`; a `GObject` `to_glib_none` allocates no temporary,
/// so it stays valid for as long as `cancel` lives.
fn raw_cancel(cancel: Option<&Cancellable>) -> *mut libfprint_sys::GCancellable {
    match cancel {
        Some(c) => {
            let ptr: *mut gio::ffi::GCancellable = c.to_glib_none().0;
            ptr.cast()
        }
        None => ptr::null_mut(),
    }
}

/// Wrap a possibly-NULL, transfer-full `FpPrint*` out-parameter (the match/scan slots of
/// verify/identify) as an owned [`FpPrint`].
fn out_print(raw: *mut libfprint_sys::FpPrint) -> Option<FpPrint> {
    if raw.is_null() {
        None
    } else {
        // SAFETY: a non-NULL match/scan out-pointer written by `fp_device_verify_sync` /
        // `fp_device_identify_sync` is transfer-full; `from_glib_full` takes its reference.
        Some(unsafe { from_glib_full(raw) })
    }
}

impl FpContext {
    /// Create a fresh context.
    pub(crate) fn new() -> FpContext {
        // SAFETY: `fp_context_new` returns a transfer-full `FpContext`; `from_glib_full` takes
        // that single reference.
        unsafe { from_glib_full(libfprint_sys::fp_context_new()) }
    }

    /// The readers the context currently knows about.
    pub(crate) fn devices(&self) -> Vec<FpDevice> {
        // SAFETY: `fp_context_get_devices` returns a transfer-none `GPtrArray` of `FpDevice*`
        // owned by the context; `from_glib_none` copies the Vec and refs each borrowed element.
        unsafe {
            let array = libfprint_sys::fp_context_get_devices(self.to_glib_none().0);
            FromGlibPtrContainer::from_glib_none(array.cast::<glib::ffi::GPtrArray>())
        }
    }
}

/// libfprint's `(device, cancellable, error) -> gboolean` sync entry points, shared by the
/// operations with no callback or return value.
type BoolSyncFn = unsafe extern "C" fn(
    *mut libfprint_sys::FpDevice,
    *mut libfprint_sys::GCancellable,
    *mut *mut libfprint_sys::GError,
) -> libfprint_sys::gboolean;

impl FpDevice {
    /// The driver id (e.g. `virtual_device`).
    pub(crate) fn driver(&self) -> String {
        // SAFETY: `fp_device_get_driver` returns a device-owned (transfer-none) C string; copied.
        unsafe {
            let ptr = libfprint_sys::fp_device_get_driver(self.to_glib_none().0);
            glib::GString::from_glib_none(ptr).into()
        }
    }

    /// The device id, empty for the virtual debug devices.
    pub(crate) fn device_id(&self) -> String {
        // SAFETY: `fp_device_get_device_id` returns a device-owned (transfer-none) C string; copied.
        unsafe {
            let ptr = libfprint_sys::fp_device_get_device_id(self.to_glib_none().0);
            glib::GString::from_glib_none(ptr).into()
        }
    }

    /// The human-readable name, or `None` when the driver exposes none.
    pub(crate) fn name(&self) -> Option<String> {
        // SAFETY: `fp_device_get_name` is a pure getter returning a device-owned (transfer-none)
        // C string or NULL; `from_glib_none` copies it without taking ownership.
        unsafe {
            let ptr = libfprint_sys::fp_device_get_name(self.to_glib_none().0);
            (!ptr.is_null()).then(|| glib::GString::from_glib_none(ptr).into())
        }
    }

    /// Whether the sensor is currently open.
    pub(crate) fn is_open(&self) -> bool {
        // SAFETY: `fp_device_is_open` is a pure boolean getter on the live device.
        unsafe { libfprint_sys::fp_device_is_open(self.to_glib_none().0) != glib::ffi::GFALSE }
    }

    /// The raw `FpScanType`.
    pub(crate) fn scan_type(&self) -> u32 {
        // SAFETY: `fp_device_get_scan_type` is a pure getter returning an `FpScanType`.
        unsafe { libfprint_sys::fp_device_get_scan_type(self.to_glib_none().0) }
    }

    /// The raw `FpDeviceFeature` bitmask.
    pub(crate) fn features(&self) -> u32 {
        // SAFETY: `fp_device_get_features` is a pure getter returning the feature bitmask.
        unsafe { libfprint_sys::fp_device_get_features(self.to_glib_none().0) }
    }

    /// The raw `FpTemperature`.
    pub(crate) fn temperature(&self) -> u32 {
        // SAFETY: `fp_device_get_temperature` is a pure getter returning the thermal state.
        unsafe { libfprint_sys::fp_device_get_temperature(self.to_glib_none().0) }
    }

    /// The raw `FpFingerStatusFlags` bitmask.
    pub(crate) fn finger_status(&self) -> u32 {
        // SAFETY: `fp_device_get_finger_status` is a pure getter returning the status flags.
        unsafe { libfprint_sys::fp_device_get_finger_status(self.to_glib_none().0) }
    }

    /// The number of enroll stages (negative before the device settles it).
    pub(crate) fn nr_enroll_stages(&self) -> i32 {
        // SAFETY: `fp_device_get_nr_enroll_stages` is a pure getter.
        unsafe { libfprint_sys::fp_device_get_nr_enroll_stages(self.to_glib_none().0) }
    }

    /// Drive one of the `(device, cancellable, error) -> gboolean` sync ops.
    fn bool_sync(&self, cancel: Option<&Cancellable>, f: BoolSyncFn) -> Result<(), glib::Error> {
        let mut error = ptr::null_mut();
        // SAFETY: `f` is one of libfprint's `(device, cancellable, error) -> gboolean` sync entry
        // points; it runs on the live `self` with a live-or-null cancellable and, on FALSE, sets
        // `error` transfer-full.
        let ok = unsafe { f(self.to_glib_none().0, raw_cancel(cancel), &mut error) };
        if ok == glib::ffi::GFALSE {
            // SAFETY: `error` was set transfer-full by the failing call above.
            Err(unsafe { glib::Error::from_glib_full(error.cast()) })
        } else {
            Ok(())
        }
    }

    pub(crate) fn open_sync(&self, cancel: Option<&Cancellable>) -> Result<(), glib::Error> {
        self.bool_sync(cancel, libfprint_sys::fp_device_open_sync)
    }

    pub(crate) fn close_sync(&self, cancel: Option<&Cancellable>) -> Result<(), glib::Error> {
        self.bool_sync(cancel, libfprint_sys::fp_device_close_sync)
    }

    pub(crate) fn suspend_sync(&self, cancel: Option<&Cancellable>) -> Result<(), glib::Error> {
        self.bool_sync(cancel, libfprint_sys::fp_device_suspend_sync)
    }

    pub(crate) fn resume_sync(&self, cancel: Option<&Cancellable>) -> Result<(), glib::Error> {
        self.bool_sync(cancel, libfprint_sys::fp_device_resume_sync)
    }

    pub(crate) fn clear_storage_sync(
        &self,
        cancel: Option<&Cancellable>,
    ) -> Result<(), glib::Error> {
        self.bool_sync(cancel, libfprint_sys::fp_device_clear_storage_sync)
    }

    /// Run a full enrollment. `progress`/`progress_data` are libfprint's per-stage callback and
    /// its opaque user-data (a same-thread `&EnrollSink`), invoked synchronously during the call.
    pub(crate) fn enroll_sync(
        &self,
        template: FpPrint,
        cancel: Option<&Cancellable>,
        progress: libfprint_sys::FpEnrollProgress,
        progress_data: *mut c_void,
    ) -> Result<FpPrint, glib::Error> {
        let mut error = ptr::null_mut();
        // SAFETY: `fp_device_enroll_sync` drives enrollment on the live `self`. `template` is
        // `(transfer floating)`: libfprint ref-sinks it, so we pass a borrowed pointer and keep
        // our own reference (dropped with `template` below). It returns a transfer-full `FpPrint`
        // on success, or NULL with `error` set transfer-full.
        let print = unsafe {
            libfprint_sys::fp_device_enroll_sync(
                self.to_glib_none().0,
                template.to_glib_none().0,
                raw_cancel(cancel),
                progress,
                progress_data,
                &mut error,
            )
        };
        if print.is_null() {
            // SAFETY: `error` was set transfer-full by the failing call above.
            Err(unsafe { glib::Error::from_glib_full(error.cast()) })
        } else {
            // SAFETY: `print` is the transfer-full enrolled `FpPrint`.
            Ok(unsafe { from_glib_full(print) })
        }
    }

    /// Verify against `enrolled`. Returns `(matched, scanned)`, where `scanned` is the live scan
    /// when the driver surfaces one. `match_cb`/`match_data` are libfprint's same-thread callback.
    pub(crate) fn verify_sync(
        &self,
        enrolled: &FpPrint,
        cancel: Option<&Cancellable>,
        match_cb: libfprint_sys::FpMatchCb,
        match_data: *mut c_void,
    ) -> Result<(bool, Option<FpPrint>), glib::Error> {
        let mut error = ptr::null_mut();
        let mut matched = glib::ffi::GFALSE;
        let mut scanned: *mut libfprint_sys::FpPrint = ptr::null_mut();
        // SAFETY: `fp_device_verify_sync` verifies the live `self` against the borrowed
        // (transfer-none) `enrolled` print. `matched` receives the boolean result; `scanned`
        // receives a transfer-full `FpPrint` or stays NULL. On failure it returns FALSE with
        // `error` set transfer-full.
        let ok = unsafe {
            libfprint_sys::fp_device_verify_sync(
                self.to_glib_none().0,
                enrolled.to_glib_none().0,
                raw_cancel(cancel),
                match_cb,
                match_data,
                &mut matched,
                &mut scanned,
                &mut error,
            )
        };
        if ok == glib::ffi::GFALSE {
            // SAFETY: `error` was set transfer-full by the failing call above.
            return Err(unsafe { glib::Error::from_glib_full(error.cast()) });
        }
        Ok((matched != glib::ffi::GFALSE, out_print(scanned)))
    }

    /// Identify `self`'s scan against `gallery`. Returns `(matched, scanned)`, where `matched` is
    /// the gallery print libfprint hands back (not its index).
    pub(crate) fn identify_sync(
        &self,
        gallery: &[FpPrint],
        cancel: Option<&Cancellable>,
        match_cb: libfprint_sys::FpMatchCb,
        match_data: *mut c_void,
    ) -> Result<(Option<FpPrint>, Option<FpPrint>), glib::Error> {
        let mut error = ptr::null_mut();
        let mut matched: *mut libfprint_sys::FpPrint = ptr::null_mut();
        let mut scanned: *mut libfprint_sys::FpPrint = ptr::null_mut();
        let (array, _elements): (*mut glib::ffi::GPtrArray, _) =
            ToGlibContainerFromSlice::to_glib_container_from_slice(gallery);
        // SAFETY: `fp_device_identify_sync` matches the live `self` against the borrowed
        // (transfer-none) `array` of gallery prints. `matched`/`scanned` receive transfer-full
        // `FpPrint`s or stay NULL. On failure it returns FALSE with `error` set transfer-full.
        let ok = unsafe {
            libfprint_sys::fp_device_identify_sync(
                self.to_glib_none().0,
                array.cast(),
                raw_cancel(cancel),
                match_cb,
                match_data,
                &mut matched,
                &mut scanned,
                &mut error,
            )
        };
        // SAFETY: `array` came from `to_glib_container_from_slice` with no element free-func, so
        // freeing the segment (TRUE) releases the container without touching the borrowed elements.
        unsafe { glib::ffi::g_ptr_array_free(array, glib::ffi::GTRUE) };
        if ok == glib::ffi::GFALSE {
            // SAFETY: `error` was set transfer-full by the failing call above.
            return Err(unsafe { glib::Error::from_glib_full(error.cast()) });
        }
        Ok((out_print(matched), out_print(scanned)))
    }

    /// List the templates stored on the sensor.
    pub(crate) fn list_prints_sync(
        &self,
        cancel: Option<&Cancellable>,
    ) -> Result<Vec<FpPrint>, glib::Error> {
        let mut error = ptr::null_mut();
        // SAFETY: `fp_device_list_prints_sync` returns, on success, a `(transfer container)`
        // `GPtrArray` of `FpPrint*`: ours to free, its `g_object_unref` element free-func dropping
        // each element's ref when we do. On failure it returns NULL with `error` set transfer-full.
        let array = unsafe {
            libfprint_sys::fp_device_list_prints_sync(
                self.to_glib_none().0,
                raw_cancel(cancel),
                &mut error,
            )
        };
        if array.is_null() {
            // SAFETY: `error` was set transfer-full by the failing call above.
            Err(unsafe { glib::Error::from_glib_full(error.cast()) })
        } else {
            // SAFETY: `array` is a `(transfer container)` `GPtrArray`; `from_glib_full` copies each
            // element out with one ref of its own and frees the container — for these GObject
            // elements identical to `from_glib_container`, honouring the container-only transfer.
            Ok(unsafe {
                FromGlibPtrContainer::from_glib_full(array.cast::<glib::ffi::GPtrArray>())
            })
        }
    }

    /// Delete one stored template by its device-side handle.
    pub(crate) fn delete_print_sync(
        &self,
        print: &FpPrint,
        cancel: Option<&Cancellable>,
    ) -> Result<(), glib::Error> {
        let mut error = ptr::null_mut();
        // SAFETY: `fp_device_delete_print_sync` deletes the borrowed (transfer-none) `print` from
        // the live `self`, returning FALSE with `error` set transfer-full on failure.
        let ok = unsafe {
            libfprint_sys::fp_device_delete_print_sync(
                self.to_glib_none().0,
                print.to_glib_none().0,
                raw_cancel(cancel),
                &mut error,
            )
        };
        if ok == glib::ffi::GFALSE {
            // SAFETY: `error` was set transfer-full by the failing call above.
            Err(unsafe { glib::Error::from_glib_full(error.cast()) })
        } else {
            Ok(())
        }
    }
}

impl FpPrint {
    /// A fresh template bound to `dev`, ready to be filled in and enrolled.
    pub(crate) fn new(dev: &FpDevice) -> FpPrint {
        // SAFETY: `fp_print_new` returns a floating `FpPrint` for the live `dev`; `from_glib_none`
        // ref-sinks it (FpPrint `@extends InitiallyUnowned`), giving sole ownership with no leak.
        unsafe { from_glib_none(libfprint_sys::fp_print_new(dev.to_glib_none().0)) }
    }

    /// The driver the print was made for (transfer-none getter).
    pub(crate) fn driver(&self) -> String {
        // SAFETY: `fp_print_get_driver` returns a print-owned (transfer-none) C string; copied.
        unsafe {
            let ptr = libfprint_sys::fp_print_get_driver(self.to_glib_none().0);
            glib::GString::from_glib_none(ptr).into()
        }
    }

    /// The device id the print was made for (transfer-none getter).
    pub(crate) fn device_id(&self) -> String {
        // SAFETY: `fp_print_get_device_id` returns a print-owned (transfer-none) C string; copied.
        unsafe {
            let ptr = libfprint_sys::fp_print_get_device_id(self.to_glib_none().0);
            glib::GString::from_glib_none(ptr).into()
        }
    }

    /// Whether the print is a handle to a template stored on the device.
    pub(crate) fn device_stored(&self) -> bool {
        // SAFETY: `fp_print_get_device_stored` is a pure boolean getter on the live print.
        unsafe {
            libfprint_sys::fp_print_get_device_stored(self.to_glib_none().0) != glib::ffi::GFALSE
        }
    }

    pub(crate) fn set_finger(&self, finger: libfprint_sys::FpFinger) {
        // SAFETY: `fp_print_set_finger` sets a plain enum field on the live print.
        unsafe { libfprint_sys::fp_print_set_finger(self.to_glib_none().0, finger) };
    }

    pub(crate) fn set_username(&self, username: &str) {
        // SAFETY: `fp_print_set_username` copies the passed UTF-8 C string into the live print;
        // the `to_glib_none` temporary lives for the duration of the call.
        unsafe {
            libfprint_sys::fp_print_set_username(self.to_glib_none().0, username.to_glib_none().0);
        }
    }

    pub(crate) fn set_description(&self, description: &str) {
        // SAFETY: `fp_print_set_description` copies the passed UTF-8 C string into the live print;
        // the `to_glib_none` temporary lives for the duration of the call.
        unsafe {
            libfprint_sys::fp_print_set_description(
                self.to_glib_none().0,
                description.to_glib_none().0,
            );
        }
    }

    /// Serialize to the canonical FP3 blob (`"FP3"` + GVariant).
    pub(crate) fn serialize(&self) -> Result<Vec<u8>, glib::Error> {
        let mut data = ptr::null_mut();
        let mut len = 0;
        let mut error = ptr::null_mut();
        // SAFETY: `fp_print_serialize` writes a transfer-full `guchar*`/length pair on success, or
        // FALSE with `error` set transfer-full. `from_glib_full_num` copies the buffer into a Vec
        // and g_frees it; `from_glib_full` takes the error.
        unsafe {
            libfprint_sys::fp_print_serialize(
                self.to_glib_none().0,
                &mut data,
                &mut len,
                &mut error,
            );
            if error.is_null() {
                Ok(Vec::from_glib_full_num(data, len as usize))
            } else {
                Err(glib::Error::from_glib_full(error.cast()))
            }
        }
    }

    /// Deserialize a template from its FP3 blob.
    pub(crate) fn deserialize(bytes: &[u8]) -> Result<FpPrint, glib::Error> {
        let mut error = ptr::null_mut();
        // SAFETY: `fp_print_deserialize` reads `bytes` and returns a transfer-full `FpPrint`, or
        // NULL with `error` set transfer-full.
        let print = unsafe {
            libfprint_sys::fp_print_deserialize(
                bytes.as_ptr(),
                bytes.len() as libfprint_sys::gsize,
                &mut error,
            )
        };
        if print.is_null() {
            // SAFETY: `error` was set transfer-full by the failing call above.
            Err(unsafe { glib::Error::from_glib_full(error.cast()) })
        } else {
            // SAFETY: `print` is the transfer-full deserialized `FpPrint`.
            Ok(unsafe { from_glib_full(print) })
        }
    }
}
