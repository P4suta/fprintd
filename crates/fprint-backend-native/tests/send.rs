// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The virtual device and backend are `Send`.
//!
//! fprint-core requires no `Send` (the libfprint shim is thread-affine and `!Send`), but the
//! native, in-memory device has no thread affinity, so the daemon may move it across
//! threads. This is a compile-time assertion.

use fprint_backend_native::{VirtualBackend, VirtualDevice};

fn assert_send<T: Send>() {}

#[test]
fn types_are_send() {
    assert_send::<VirtualDevice>();
    assert_send::<VirtualBackend>();
}
