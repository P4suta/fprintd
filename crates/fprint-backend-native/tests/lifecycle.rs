// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Open/close state machine and suspend/resume.

mod common;
use common::block_on;

use fprint_backend_native::{DeviceShape, VirtualDeviceBuilder};
use fprint_core::{Device, DeviceFeature, Error, Print, ScanType};

#[test]
fn operation_before_open_is_proto_state() {
    let mut dev = VirtualDeviceBuilder::host_image_sensor().build();
    // Not opened: an operation must fail with ProtoState (not NotSupported).
    assert!(matches!(
        block_on(dev.verify(&Print::default())),
        Err(Error::ProtoState)
    ));
}

#[test]
fn double_open_is_proto_state() {
    let mut dev = VirtualDeviceBuilder::host_image_sensor().build();
    block_on(dev.open()).unwrap();
    assert!(matches!(block_on(dev.open()), Err(Error::ProtoState)));
    assert!(dev.is_open());
}

/// A device's shape is only known once it is open — see [`DeviceShape`]. Modelled on a UPEK
/// TouchStrip (swipe, 3 stages) reported by enumeration as the class defaults.
#[test]
fn shape_settles_on_open() {
    let mut dev = VirtualDeviceBuilder::host_image_sensor()
        .name("UPEK TouchStrip")
        .scan_type(ScanType::Swipe)
        .enroll_stages(3)
        .features(DeviceFeature::CAPTURE | DeviceFeature::VERIFY | DeviceFeature::IDENTIFY)
        .probe_reports(DeviceShape {
            scan_type: ScanType::Press,
            features: DeviceFeature::CAPTURE,
            enroll_stages: 5,
        })
        .build();

    // Enumeration sees the class defaults.
    assert_eq!(dev.info().scan_type, ScanType::Press);
    assert_eq!(dev.info().enroll_stages, 5);
    assert!(!dev.has_feature(DeviceFeature::IDENTIFY));

    block_on(dev.open()).unwrap();

    assert_eq!(dev.info().scan_type, ScanType::Swipe);
    assert_eq!(dev.info().enroll_stages, 3);
    assert!(dev.has_feature(DeviceFeature::IDENTIFY));

    // Identity does not move.
    assert_eq!(dev.info().name, "UPEK TouchStrip");
    assert_eq!(dev.info().driver.0, "virtual_image");

    // Closing does not un-settle the shape.
    block_on(dev.close()).unwrap();
    assert_eq!(dev.info().enroll_stages, 3);
}

/// Without a probe/open split, the case every other test uses, `open` changes nothing.
#[test]
fn shape_is_stable_when_no_split_is_modelled() {
    let mut dev = VirtualDeviceBuilder::host_image_sensor().build();
    let before = dev.info().clone();
    block_on(dev.open()).unwrap();
    assert_eq!(&before, dev.info());
}

#[test]
fn suspend_resume_ok() {
    let mut dev = VirtualDeviceBuilder::host_image_sensor().build();
    block_on(dev.open()).unwrap();

    block_on(dev.suspend()).unwrap();
    assert!(dev.is_suspended());
    block_on(dev.resume()).unwrap();
    assert!(!dev.is_suspended());

    // Close clears the open flag.
    block_on(dev.close()).unwrap();
    assert!(!dev.is_open());
}
