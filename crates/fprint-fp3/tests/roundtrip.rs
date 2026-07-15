// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! End-to-end cross-crate proof of the whole offline slice: enroll on the virtual device,
//! serialize the resulting [`Print`](fprint_core::Print) to the FP3 on-disk blob, read it back,
//! and confirm the deserialized print still verifies against the sensor.
//!
//! This is the one test that exercises all three crates at once — `fprint-backend-native`
//! (a [`fprint_core::Device`]), `fprint-fp3` (the wire codec), and `fprint-core` (the domain model they
//! meet in). It sits deliberately in `fprint-fp3`, which is above the domain model and so may
//! know a backend for testing; the backend itself stays unaware of any wire format
//! (`ARCHITECTURE.md` principle 3).
//!
//! The async surface is driven by a tiny, `unsafe`-free `block_on` built on
//! [`std::task::Wake`] — no runtime crate, matching how the backend's own integration tests
//! poll the virtual device.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use fprint_backend_native::{EnrollScript, FingerId, Scenario, VirtualDeviceBuilder};
use fprint_core::{Device, Finger, Print, Template};

/// A waker that resumes a parked thread. No `RawWaker`, no `unsafe`.
struct ThreadWaker(Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

/// Drive a future to completion by park/unpark polling on the current thread.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => thread::park(),
        }
    }
}

/// Host image sensor: enroll → NBIS print → FP3 round-trip → the read-back print verifies.
#[test]
fn host_enroll_fp3_roundtrip_then_verify() {
    let mut dev = VirtualDeviceBuilder::host_image_sensor()
        .scenario(Scenario::new().enroll(EnrollScript::default().produces(FingerId(7))))
        .build();
    block_on(dev.open()).unwrap();

    // Enroll the left index finger; the virtual sensor completes all five stages cleanly.
    let print = block_on(dev.enroll(Print::new_for_enroll(Finger::LeftIndex), |_p| {})).unwrap();
    assert_eq!(print.finger, Some(Finger::LeftIndex));
    assert!(matches!(print.template, Template::Nbis(_)));

    // Serialize to the FP3 blob and back. The magic leads, and the round-trip is exact.
    let bytes = fprint_fp3::to_bytes(&print).unwrap();
    assert!(bytes.starts_with(fprint_fp3::MAGIC));
    let back = fprint_fp3::from_bytes(&bytes).unwrap();
    assert_eq!(print, back);

    // The print that came off disk still verifies against the live sensor.
    dev.present_finger(FingerId(7));
    let outcome = block_on(dev.verify(&back)).unwrap();
    assert!(outcome.matched);
}

/// Match-on-chip sensor: enroll → Raw template → FP3 round-trip is exact.
///
/// A MOC print is a device-stored handle carrying an opaque `Raw` blob; the codec must
/// preserve it (and the `device_stored` flag) byte-for-byte across the wire format.
#[test]
fn moc_enroll_raw_fp3_roundtrip() {
    let mut dev = VirtualDeviceBuilder::chip_storage_sensor()
        .scenario(Scenario::new().enroll(EnrollScript::default().produces(FingerId(7))))
        .build();
    block_on(dev.open()).unwrap();

    let print = block_on(dev.enroll(Print::new_for_enroll(Finger::LeftIndex), |_p| {})).unwrap();
    assert!(matches!(print.template, Template::Raw(_)));
    assert!(print.device_stored);

    let bytes = fprint_fp3::to_bytes(&print).unwrap();
    assert!(bytes.starts_with(fprint_fp3::MAGIC));
    let back = fprint_fp3::from_bytes(&bytes).unwrap();
    assert_eq!(print, back);
}
