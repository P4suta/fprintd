// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Verify (1:1) and identify (1:N): matching, scan surfacing, and feature gating.

mod common;
use common::block_on;

use fprint_backend_native::{EnrollScript, FingerId, Scenario, VirtualDeviceBuilder};
use fprint_core::{Device, DeviceFeature, Error, Finger, Print};

fn enroll_print(dev: &mut fprint_backend_native::VirtualDevice) -> Print {
    block_on(dev.enroll(Print::new_for_enroll(Finger::LeftIndex), &mut |_p| {})).unwrap()
}

#[test]
fn verify_match_surfaces_scan() {
    let mut dev = VirtualDeviceBuilder::host_image_sensor()
        .scenario(
            Scenario::new()
                .present(FingerId(4))
                .enroll(EnrollScript::default().produces(FingerId(4))),
        )
        .build();
    block_on(dev.open()).unwrap();

    let print = enroll_print(&mut dev);
    let outcome = block_on(dev.verify(&print)).unwrap();

    assert!(outcome.matched);
    assert!(outcome.scanned.is_some()); // host sensors surface the scan
}

#[test]
fn verify_mismatch() {
    // Enrolled finger 4, but finger 5 is on the sensor.
    let mut dev = VirtualDeviceBuilder::host_image_sensor()
        .scenario(
            Scenario::new()
                .present(FingerId(5))
                .enroll(EnrollScript::default().produces(FingerId(4))),
        )
        .build();
    block_on(dev.open()).unwrap();

    let print = enroll_print(&mut dev);
    let outcome = block_on(dev.verify(&print)).unwrap();

    assert!(!outcome.matched);
}

#[test]
fn moc_hides_scan() {
    let mut dev = VirtualDeviceBuilder::chip_storage_sensor()
        .scenario(
            Scenario::new()
                .present(FingerId(2))
                .enroll(EnrollScript::default().produces(FingerId(2))),
        )
        .build();
    block_on(dev.open()).unwrap();

    let print = enroll_print(&mut dev);
    let outcome = block_on(dev.verify(&print)).unwrap();

    assert!(outcome.matched);
    assert!(outcome.scanned.is_none()); // match-on-chip does not surface the scan
}

#[test]
fn verify_unsupported_without_feature() {
    // A device that advertises only CAPTURE — no VERIFY.
    let mut dev = VirtualDeviceBuilder::host_image_sensor()
        .features(DeviceFeature::CAPTURE)
        .build();
    block_on(dev.open()).unwrap();

    let outcome = block_on(dev.verify(&Print::default()));
    assert!(matches!(outcome, Err(Error::NotSupported)));
}

#[test]
fn identify_finds_index() {
    let mut dev = VirtualDeviceBuilder::host_image_sensor()
        .scenario(
            Scenario::new()
                .present(FingerId(8))
                .enroll(EnrollScript::default().produces(FingerId(8))),
        )
        .build();
    block_on(dev.open()).unwrap();

    let target = enroll_print(&mut dev);
    // A gallery with a decoy first, then the matching print.
    let gallery = vec![Print::default(), target];

    let outcome = block_on(dev.identify(&gallery)).unwrap();
    assert_eq!(outcome.match_index, Some(1));

    // With no finger present there is no match.
    dev.clear_finger();
    let miss = block_on(dev.identify(&gallery)).unwrap();
    assert_eq!(miss.match_index, None);
}
