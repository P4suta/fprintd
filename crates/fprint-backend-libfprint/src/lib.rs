// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

// The whole crate is Linux-only: it dynamically links the C libfprint through the
// `libfprint-rs`/`libfprint-sys` FFI stack, which only exists on Linux. On every other
// target this `#![cfg]` empties the crate so the cross-platform workspace still builds and
// resolves.
#![cfg(target_os = "linux")]

//! # fprint-backend-libfprint
//!
//! A [`fprint_core::Backend`]/[`fprint_core::Device`] implementation that wraps the **C libfprint**
//! (via the crates.io `libfprint-rs` binding), so the pure-Rust daemon can drive every sensor
//! libfprint already supports. The LGPL library is linked *dynamically* — an interoperability
//! boundary LGPL explicitly permits — and this crate's own source stays MIT/Apache.
//!
//! ## The `!Send` bridge
//!
//! libfprint's objects are glib `GObject`s bound to the thread that created their
//! `GMainContext`. [`LibfprintBackend`] and [`LibfprintDevice`] are therefore **`!Send`**
//! (they hold `FpContext`/`FpDevice`, which are `Rc`-flavoured, non-`Send` glib wrappers).
//! `fprint-core` never requires `Send` (principle 7), so this thread affinity is expressible;
//! the daemon confines each device to a single actor thread.
//!
//! ## Cancellation limitation
//!
//! `fprint-core`'s contract is "**dropping the future cancels the operation**" (principle 4).
//! The binding, however, only exposes the *blocking* `*_sync` entry points: once
//! [`LibfprintDevice`]'s `enroll`/`verify`/`identify` calls into libfprint it does not return
//! until libfprint does. A live [`gio::Cancellable`] is kept per in-flight operation and
//! handed to libfprint, but it cannot be fired from a `Drop` that never runs while the
//! calling thread is parked inside the FFI call. Cancellation here is therefore best-effort,
//! and only meaningful when the daemon runs the blocking call on a thread it can later
//! signal. `fprint-backend-native` is fully drop-cancellable.

mod backend;
mod convert;
mod device;
mod print;
mod progress;
mod storage;

pub use backend::LibfprintBackend;
pub use device::LibfprintDevice;
