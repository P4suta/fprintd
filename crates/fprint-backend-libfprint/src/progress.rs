// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The enroll-progress and verify/identify match trampolines.
//!
//! libfprint's progress/match callbacks are C function pointers with a single `user_data` slot.
//! The worker passes a pointer to a stack-owned sink as that slot and one of these trampolines as
//! the callback; libfprint invokes the trampoline **synchronously, on the worker thread, from
//! inside the `*_sync` call**, so the trampoline can rebuild a shared `&Sink` and read the
//! device's live finger status there. It forwards a fully-formed domain report over the sink's
//! channel; the caller's future delivers the reports to the user closure.

use std::os::raw::{c_int, c_void};

use fprint_core::{EnrollProgress, FingerStatus};
use futures_channel::mpsc::UnboundedSender;
use glib::translate::{from_glib_borrow, FromGlibPtrNone};

use crate::convert;
use crate::ffi::FpDevice;

/// Ferries enroll progress from the worker to the caller's future: the progress sender plus the
/// device's total stage count (needed to shape each [`EnrollProgress`]).
pub(crate) struct EnrollSink {
    pub(crate) tx: UnboundedSender<EnrollProgress>,
    pub(crate) total: u32,
}

/// Ferries verify/identify finger-presence status from the worker to the caller's future.
pub(crate) struct StatusSink {
    pub(crate) tx: UnboundedSender<FingerStatus>,
}

/// libfprint's per-capture enroll callback (`FpEnrollProgress`). A failed capture arrives as a
/// retry-domain `error` with the stage count unchanged; it is relayed as
/// [`EnrollProgress::with_retry`] rather than aborting the enrollment. A closed channel (the
/// caller dropped the operation future) is ignored.
///
/// # Safety
///
/// Called only by libfprint, synchronously, from inside a `fp_device_enroll_sync` whose
/// `user_data` is the `&EnrollSink` the worker kept alive across the call.
pub(crate) unsafe extern "C" fn on_enroll_progress(
    device: *mut libfprint_sys::FpDevice,
    completed_stages: c_int,
    _print: *mut libfprint_sys::FpPrint,
    user_data: *mut c_void,
    error: *mut libfprint_sys::GError,
) {
    if user_data.is_null() {
        return;
    }
    // SAFETY: libfprint invokes this synchronously on the worker thread from inside the
    // `fp_device_enroll_sync` call whose `progress_data` is this `&EnrollSink` and whose live
    // device is `device` (borrowed, no ref taken). Both outlive the call, and the same-thread
    // invocation means the shared borrow cannot race.
    let sink = unsafe { &*(user_data as *const EnrollSink) };
    let dev = unsafe { from_glib_borrow::<_, FpDevice>(device) };

    let retry = if error.is_null() {
        None
    } else {
        // SAFETY: `error` is a transfer-none retry `GError`; copied here for classification.
        let err = unsafe { glib::Error::from_glib_none(error.cast()) };
        convert::gerror_retry(&err)
    };

    let mut progress = EnrollProgress::new(completed_stages.max(0) as u32, sink.total)
        .with_finger_status(convert::finger_status(&dev));
    if let Some(reason) = retry {
        progress = progress.with_retry(reason);
    }
    let _ = sink.tx.unbounded_send(progress);
}

/// libfprint's match callback (`FpMatchCb`), invoked when a print is matched (or a retry occurs)
/// during verify/identify. Relays the device's live finger-presence status so the daemon can drive
/// the `finger-present` / `finger-needed` prompts during a login as it does during enroll.
///
/// # Safety
///
/// Called only by libfprint, synchronously, from inside a `fp_device_verify_sync` /
/// `fp_device_identify_sync` whose `user_data` is the `&StatusSink` the worker kept alive.
pub(crate) unsafe extern "C" fn on_match_status(
    device: *mut libfprint_sys::FpDevice,
    _match: *mut libfprint_sys::FpPrint,
    _print: *mut libfprint_sys::FpPrint,
    user_data: *mut c_void,
    _error: *mut libfprint_sys::GError,
) {
    if user_data.is_null() {
        return;
    }
    // SAFETY: as in `on_enroll_progress` — invoked synchronously on the worker thread from inside
    // the match `*_sync` call; `user_data` is the live `&StatusSink`, `device` the live device
    // (borrowed, no ref taken).
    let sink = unsafe { &*(user_data as *const StatusSink) };
    let dev = unsafe { from_glib_borrow::<_, FpDevice>(device) };
    let _ = sink.tx.unbounded_send(convert::finger_status(&dev));
}
