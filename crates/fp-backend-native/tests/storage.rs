// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! On-device storage: list, delete, clear on a MOC device; unsupported on a host sensor.

mod common;
use common::block_on;

use fp_backend_native::{EnrollScript, FingerId, Scenario, VirtualDeviceBuilder};
use fp_core::{Device, Error, Finger, Print};

fn moc_with_enrollment(id: u64) -> (fp_backend_native::VirtualDevice, Print) {
    let mut dev = VirtualDeviceBuilder::chip_storage_sensor()
        .scenario(Scenario::new().enroll(EnrollScript::default().produces(FingerId(id))))
        .build();
    block_on(dev.open()).unwrap();
    let print =
        block_on(dev.enroll(Print::new_for_enroll(Finger::RightRing), &mut |_p| {})).unwrap();
    (dev, print)
}

#[test]
fn list_delete_clear() {
    let (mut dev, print) = moc_with_enrollment(5);

    // List sees the enrolled print.
    let listed = block_on(dev.list_prints()).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].template, print.template);

    // Delete removes it; deleting again is DataNotFound.
    block_on(dev.delete_print(&print)).unwrap();
    assert!(block_on(dev.list_prints()).unwrap().is_empty());
    assert!(matches!(
        block_on(dev.delete_print(&print)),
        Err(Error::DataNotFound)
    ));

    // Re-enroll two, then clear wipes storage.
    block_on(dev.enroll(Print::new_for_enroll(Finger::RightRing), &mut |_p| {})).unwrap();
    dev.present_finger(FingerId(6));
    dev.set_enroll_script(EnrollScript::default().produces(FingerId(6)));
    block_on(dev.enroll(Print::new_for_enroll(Finger::RightLittle), &mut |_p| {})).unwrap();
    assert_eq!(block_on(dev.list_prints()).unwrap().len(), 2);

    block_on(dev.clear_storage()).unwrap();
    assert!(block_on(dev.list_prints()).unwrap().is_empty());
}

#[test]
fn storage_unsupported_on_host() {
    let mut dev = VirtualDeviceBuilder::host_image_sensor().build();
    block_on(dev.open()).unwrap();

    assert!(matches!(
        block_on(dev.list_prints()),
        Err(Error::NotSupported)
    ));
    assert!(matches!(
        block_on(dev.delete_print(&Print::default())),
        Err(Error::NotSupported)
    ));
    assert!(matches!(
        block_on(dev.clear_storage()),
        Err(Error::NotSupported)
    ));
}
