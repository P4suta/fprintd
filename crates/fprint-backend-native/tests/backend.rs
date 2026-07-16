// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The backend: enumerate the roster, open by id (known and unknown).

use fprint_testkit::block_on;

use fprint_backend_native::{VirtualBackend, VirtualDeviceBuilder};
use fprint_core::{Backend, Device, DeviceId, Error};

fn backend() -> VirtualBackend {
    VirtualBackend::new(vec![
        VirtualDeviceBuilder::host_image_sensor().id(DeviceId::new("host-0")),
        VirtualDeviceBuilder::chip_storage_sensor().id(DeviceId::new("moc-0")),
    ])
}

#[test]
fn enumerate_returns_both() {
    let devices = block_on(backend().enumerate()).unwrap();
    assert_eq!(devices.len(), 2);
    assert_eq!(devices[0].info().id, DeviceId::new("host-0"));
    assert_eq!(devices[1].info().id, DeviceId::new("moc-0"));
}

#[test]
fn open_known_id() {
    let dev = block_on(backend().open(&DeviceId::new("moc-0"))).unwrap();
    assert_eq!(dev.info().name, "Virtual MOC Sensor");
}

#[test]
fn open_unknown_id_is_not_found() {
    let result = block_on(backend().open(&DeviceId::new("nope")));
    assert!(matches!(result, Err(Error::NotFound)));
}
