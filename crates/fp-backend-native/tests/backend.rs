// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The backend: enumerate the roster, open by id (known and unknown).

mod common;
use common::block_on;

use fp_backend_native::{VirtualBackend, VirtualDeviceBuilder};
use fp_core::{Backend, Device, DeviceId, Error};

fn backend() -> VirtualBackend {
    VirtualBackend::new(vec![
        VirtualDeviceBuilder::host_image_sensor().id(DeviceId("host-0".to_string())),
        VirtualDeviceBuilder::chip_storage_sensor().id(DeviceId("moc-0".to_string())),
    ])
}

#[test]
fn enumerate_returns_both() {
    let devices = block_on(backend().enumerate()).unwrap();
    assert_eq!(devices.len(), 2);
    assert_eq!(devices[0].info().id, DeviceId("host-0".to_string()));
    assert_eq!(devices[1].info().id, DeviceId("moc-0".to_string()));
}

#[test]
fn open_known_id() {
    let dev = block_on(backend().open(&DeviceId("moc-0".to_string()))).unwrap();
    assert_eq!(dev.info().name, "Virtual MOC Sensor");
}

#[test]
fn open_unknown_id_is_not_found() {
    let result = block_on(backend().open(&DeviceId("nope".to_string())));
    assert!(matches!(result, Err(Error::NotFound)));
}
