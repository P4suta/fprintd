// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Cancellation is dropping the future (ARCHITECTURE.md principle 4).
//!
//! A partially-driven `enroll` that is dropped must leave the device untouched: nothing in
//! storage, still open, ready for a fresh enrollment.

mod common;
use common::{block_on, poll_n};

use fprint_backend_native::{EnrollScript, FingerId, Scenario, VirtualDeviceBuilder};
use fprint_core::{Device, Finger, Print};

#[test]
fn dropping_enroll_cancels_without_committing() {
    // A MOC device with three stages, so two polls cannot finish it.
    let mut dev = VirtualDeviceBuilder::chip_storage_sensor()
        .enroll_stages(3)
        .scenario(Scenario::new().enroll(EnrollScript::default().produces(FingerId(9))))
        .build();
    block_on(dev.open()).unwrap();

    {
        let mut on_progress = |_p| {};
        // `Box::pin` makes the (non-Unpin) enroll future `Unpin` so `poll_n` can drive it.
        let mut fut =
            Box::pin(dev.enroll(Print::new_for_enroll(Finger::LeftThumb), &mut on_progress));
        let outcome = poll_n(&mut fut, 2);
        assert!(outcome.is_none()); // still mid-enrollment after two polls
        drop(fut); // <- cancellation
    }

    // Nothing was committed, and the device is still open and usable.
    assert!(dev.stored_prints().is_empty());
    assert!(dev.is_open());

    // A fresh enrollment now completes and stores.
    let print =
        block_on(dev.enroll(Print::new_for_enroll(Finger::LeftThumb), &mut |_p| {})).unwrap();
    assert!(print.device_stored);
    assert_eq!(dev.stored_prints().len(), 1);
}
