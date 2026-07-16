// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The enroll-progress trampoline.
//!
//! The binding's `FpEnrollProgress` is a *non-capturing* `fn` pointer with a single generic
//! user-data slot; it cannot carry a Rust closure, while `fprint-core`'s [`Device::enroll`]
//! takes a generic `F: FnMut(EnrollProgress)`. The bridge parks a `&mut F` (plus the total
//! stage count) in a [`Trampoline<'_, F>`], passes a raw pointer to it through the user-data
//! slot, and rebuilds the `&mut` inside [`on_enroll_progress`] — a fn generic over `F` that
//! monomorphizes to one concrete `extern`-compatible fn pointer per closure type.
//!
//! [`Device::enroll`]: fprint_core::Device::enroll

use core::ffi::c_void;

use fprint_core::EnrollProgress;
use libfprint_rs::{FpDevice, FpPrint, GError};

use crate::convert;

/// The caller's progress callback and the device's total stage count, ferried across FFI.
///
/// Generic over the concrete closure type `F` so the callback stays a monomorphized fn pointer
/// (no `dyn`): [`on_enroll_progress`] is instantiated for the exact `F` that produced the
/// pointer, so the raw-pointer reconstruction below is type-correct by construction.
pub struct Trampoline<'a, F> {
    pub cb: &'a mut F,
    pub total: u32,
}

/// The `FpEnrollProgress<*mut c_void>` libfprint invokes once per capture attempt.
///
/// Generic over `F` but takes none of it in its signature, so `on_enroll_progress::<F>`
/// monomorphizes to a plain fn pointer of the exact type libfprint expects.
///
/// A failed capture arrives as a retry-domain `error` with the stage count unchanged; we relay
/// it as [`EnrollProgress::retry`] rather than aborting the enrollment. The device's live
/// finger-presence status is read from `dev` and attached to each report.
pub fn on_enroll_progress<F: FnMut(EnrollProgress)>(
    dev: &FpDevice,
    completed: i32,
    _print: Option<FpPrint>,
    error: Option<GError>,
    data: &Option<*mut c_void>,
) {
    // `*mut c_void` is `Copy`, so read the pointer out of the shared `&Option<_>` by value.
    let Some(ptr) = *data else { return };

    // SAFETY: `ptr` is the `&mut Trampoline<'_, F>` we handed to `fp_device_enroll_sync` as its
    // user-data, and `F` here is the very closure type it was created with (this fn is
    // monomorphized per `F`). libfprint runs this callback synchronously, on the very thread
    // parked inside `enroll_sync`, strictly within the lifetime of the borrow that produced the
    // pointer. No other alias to the `Trampoline` exists and the callback never outlives the
    // enroll call, so reconstituting the `&mut` here is sound.
    let tramp: &mut Trampoline<'_, F> = unsafe { &mut *ptr.cast::<Trampoline<'_, F>>() };

    let retry = error.as_ref().and_then(convert::gerror_retry);
    let mut progress = EnrollProgress::new(completed.max(0) as u32, tramp.total)
        .with_finger_status(convert::finger_status(dev));
    if let Some(reason) = retry {
        progress = progress.with_retry(reason);
    }
    (tramp.cb)(progress);
}
