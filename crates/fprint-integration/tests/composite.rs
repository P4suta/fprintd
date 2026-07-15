// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The composite seam, driven native-only so it is green on the Windows dev box.
//!
//! We build a [`CompositeBackend::with_native`] over a single host image sensor and prove the
//! delegation works end to end: enumeration surfaces exactly one `Native` device, `open`
//! routes by id (and reports `NotFound` for an unknown one), and a full enroll → verify runs
//! *through* the composite [`Device`] impl. On Linux the same backend could add a shim device
//! via [`CompositeBackend::new`]; the default test stays native-only so it passes anywhere.

mod common;
use common::block_on;

use fprint_backend_native::{
    EnrollScript, FingerId, Scenario, VirtualBackend, VirtualDeviceBuilder,
};
use fprint_core::{Backend, Device, DeviceId, Error, Finger, Print};
use fprint_integration::{CompositeBackend, CompositeDevice};

/// The default device id for the host image sensor is its driver name.
const IMAGE_ID: &str = "virtual_image";

/// A native-only composite serving one host image sensor that enrolls and matches finger 7.
fn backend() -> CompositeBackend {
    CompositeBackend::with_native(VirtualBackend::single(
        VirtualDeviceBuilder::host_image_sensor().scenario(
            Scenario::new()
                .present(FingerId(7))
                .enroll(EnrollScript::default().produces(FingerId(7))),
        ),
    ))
}

#[test]
fn enumerate_finds_one_native_device() {
    let backend = backend();
    let devices = block_on(backend.enumerate()).unwrap();

    assert_eq!(devices.len(), 1);
    assert!(matches!(devices[0], CompositeDevice::Native(_)));
    // `info()` is delegated through the enum.
    assert_eq!(devices[0].info().id, DeviceId(IMAGE_ID.into()));
}

#[test]
fn open_routes_by_id() {
    let backend = backend();

    let dev = block_on(backend.open(&DeviceId(IMAGE_ID.into()))).unwrap();
    assert!(matches!(dev, CompositeDevice::Native(_)));

    // With no shim to fall through to, an unknown id is NotFound.
    let missing = block_on(backend.open(&DeviceId("no-such-device".into())));
    assert!(matches!(missing, Err(Error::NotFound)));
}

#[test]
fn enroll_then_verify_through_composite() {
    let backend = backend();
    let mut dev = block_on(backend.open(&DeviceId(IMAGE_ID.into()))).unwrap();
    block_on(dev.open()).unwrap();

    // Enroll: drive the one multi-poll method through the composite, counting clean stages.
    let mut last_completed = 0u32;
    let print = block_on(dev.enroll(Print::new_for_enroll(Finger::LeftIndex), |p| {
        if p.retry.is_none() {
            last_completed = p.completed_stages;
        }
    }))
    .unwrap();

    assert_eq!(print.finger, Some(Finger::LeftIndex));
    assert_eq!(last_completed, 5); // host_image_sensor advertises five enroll stages

    // Verify: finger 7 is on the sensor and the enrolled print encodes finger 7, so it matches
    // and a host sensor surfaces the scan — all routed through the composite delegation.
    let outcome = block_on(dev.verify(&print)).unwrap();
    assert!(outcome.matched);
    assert!(outcome.scanned.is_some());

    block_on(dev.close()).unwrap();
}
