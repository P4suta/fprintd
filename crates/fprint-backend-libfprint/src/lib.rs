// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

// The whole crate is Linux-only: it dynamically links the C libfprint through the
// `libfprint-rs`/`libfprint-sys` FFI stack, which only exists on Linux. On every other
// target this `#![cfg]` empties the crate so the cross-platform workspace still builds and
// resolves — the Windows dev box compiles an empty library here and moves on.
#![cfg(target_os = "linux")]

//! # fprint-backend-libfprint — the M1 shim
//!
//! A [`fprint_core::Backend`]/[`fprint_core::Device`] implementation that wraps the **C libfprint**
//! (via the crates.io `libfprint-rs` binding), so the pure-Rust daemon can drive every
//! sensor libfprint already supports while the native drivers are still being written.
//! `ARCHITECTURE.md` calls this "the honest path we take first": we *dynamically* link the
//! LGPL library — an interoperability boundary LGPL explicitly permits — and keep our own
//! source MIT/Apache.
//!
//! ## The `!Send` bridge
//!
//! libfprint's objects are glib `GObject`s bound to the thread that created their
//! `GMainContext`. [`LibfprintBackend`] and [`LibfprintDevice`] are therefore **`!Send`**
//! (they hold `FpContext`/`FpDevice`, which are `Rc`-flavoured, non-`Send` glib wrappers).
//! `fprint-core` never requires `Send`, exactly so this backend can be honest about its thread
//! affinity (principle 7); the daemon confines each device to a single actor thread.
//!
//! ## The honest cancellation limitation
//!
//! `fprint-core`'s contract is "**dropping the future cancels the operation**" (principle 4).
//! The binding, however, only exposes the *blocking* `*_sync` entry points: once
//! [`LibfprintDevice::enroll`]/`verify`/`identify` calls into libfprint it does not return
//! until libfprint does. We keep a live [`gio::Cancellable`] per in-flight operation and
//! hand it to libfprint, but we cannot fire it from a `Drop` that never runs while the
//! calling thread is parked inside the FFI call. The faithful, fully-drop-cancellable story
//! belongs to `fprint-backend-native`; here cancellation is best-effort and only meaningful when
//! the daemon runs the blocking call on a thread it can later signal. This is documented
//! rather than papered over.

mod backend;
mod convert;
mod device;
mod print;
mod progress;
mod storage;

pub use backend::LibfprintBackend;
pub use device::LibfprintDevice;
