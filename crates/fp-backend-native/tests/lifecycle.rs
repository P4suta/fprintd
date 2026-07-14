// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Open/close state machine and suspend/resume.

mod common;
use common::block_on;

use fp_backend_native::VirtualDeviceBuilder;
use fp_core::{Device, Error, Print};

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
