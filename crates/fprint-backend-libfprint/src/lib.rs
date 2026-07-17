// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

// The whole crate is Linux-only: it dynamically links the C libfprint through `libfprint-sys`,
// which only exists on Linux. On every other target this `#![cfg]` empties the crate so the
// cross-platform workspace still builds and resolves.
#![cfg(target_os = "linux")]

//! # fprint-backend-libfprint
//!
//! A [`fprint_core::Backend`]/[`fprint_core::Device`] implementation that owns the **C libfprint**
//! FFI directly through `libfprint-sys`, so the pure-Rust daemon can drive every sensor libfprint
//! already supports. The LGPL library is linked *dynamically* — an interoperability boundary LGPL
//! explicitly permits — and this crate's own source stays MIT/Apache.
//!
//! ## The `!Send` bridge and the worker thread
//!
//! libfprint's objects are glib `GObject`s bound to the thread that created their `GMainContext`,
//! and its only entry points are the *blocking* `*_sync` calls. The shim wraps them as thin glib
//! objects and owns their lifetime itself (the `ffi` module). [`LibfprintBackend`] holds an
//! `FpContext` and is therefore **`!Send`**; discovery reads each device's getters on the caller
//! thread and hands the device off to a dedicated **worker thread** that owns the `FpDevice` and
//! runs every `*_sync` there. [`LibfprintDevice`] is thus a `Send` handle — a channel to its
//! worker — while the sensor it drives never leaves that one thread. `fprint-core` never requires
//! `Send` (principle 7), so either shape is expressible; cancellation runs through the worker
//! thread.
//!
//! ## Cancellation
//!
//! `fprint-core`'s contract is "**dropping the future cancels the operation**" (principle 4), and
//! the shim honours it fully. Each operation submits a job to the worker with a fresh
//! [`gio::Cancellable`] and awaits the reply; the worker parks inside the blocking `*_sync` on its
//! own thread, so the operation future yields `Pending` on the caller thread. A guard held across
//! that await fires the cancellable — which is `Send` — from the caller thread if the future is
//! dropped, waking the parked `*_sync` cross-thread (libfprint's own `g_cancellable_cancel` path).
//! The worker's call returns `Cancelled` and it moves on to the next job, releasing the sensor
//! without the caller ever waiting.

// The pure crates all set this; the shim is the one published crate that may not
// `#![forbid(unsafe_code)]` (it is the FFI quarantine), but it holds the same documentation bar.
#![deny(missing_docs)]

mod backend;
mod convert;
mod device;
mod ffi;
mod print;
mod progress;
mod worker;

pub use backend::LibfprintBackend;
pub use device::LibfprintDevice;

/// Round-trip `bytes` through the real C libfprint's own `fp_print_deserialize` →
/// `fp_print_serialize`, yielding libfprint's canonical FP3 encoding. The byte-identity tests use
/// it as the real-libfprint oracle for `fprint-fp3`'s output. Gated behind the test-only `virtual`
/// feature and hidden from docs: it is not part of the shim's API.
#[cfg(feature = "virtual")]
#[doc(hidden)]
pub fn libfprint_canonical_fp3(bytes: &[u8]) -> Result<Vec<u8>, glib::Error> {
    ffi::FpPrint::deserialize(bytes)?.serialize()
}
