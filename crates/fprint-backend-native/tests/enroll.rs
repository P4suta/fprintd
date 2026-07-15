// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Enrollment: multi-stage progress, retries, MOC storage, duplicates, full storage.

mod common;
use common::block_on;

use fprint_backend_native::{EnrollScript, FingerId, Scenario, VirtualDeviceBuilder};
use fprint_core::{Device, DriverId, EnrollProgress, Error, Finger, Print, RetryReason, Template};

/// Enroll a finger, collecting the progress reports along the way.
fn enroll(
    dev: &mut fprint_backend_native::VirtualDevice,
    finger: Finger,
) -> (fprint_core::Result<Print>, Vec<EnrollProgress>) {
    let mut log = Vec::new();
    let result = {
        let mut on_progress = |p: EnrollProgress| log.push(p);
        block_on(dev.enroll(Print::new_for_enroll(finger), &mut on_progress))
    };
    (result, log)
}

#[test]
fn host_completes_five_stages() {
    let mut dev = VirtualDeviceBuilder::host_image_sensor()
        .scenario(Scenario::new().enroll(EnrollScript::default().produces(FingerId(7))))
        .build();
    block_on(dev.open()).unwrap();

    let (result, log) = enroll(&mut dev, Finger::LeftIndex);
    let print = result.unwrap();

    // One progress report per stage, counting 1..=5, none of them retries.
    assert_eq!(log.len(), 5);
    let counts: Vec<u32> = log.iter().map(|p| p.completed_stages).collect();
    assert_eq!(counts, vec![1, 2, 3, 4, 5]);
    assert!(log.iter().all(|p| p.total_stages == 5 && p.retry.is_none()));

    // The finished print carries the finger, driver, and an NBIS template; host storage is empty.
    assert_eq!(print.finger, Some(Finger::LeftIndex));
    assert_eq!(print.driver, Some(DriverId("virtual_image".into())));
    assert!(matches!(print.template, Template::Nbis(_)));
    assert!(!print.device_stored);
    assert!(dev.stored_prints().is_empty());
}

#[test]
fn retry_then_complete() {
    // A retry, then the sensor completes the five stages normally.
    let mut dev = VirtualDeviceBuilder::host_image_sensor()
        .scenario(
            Scenario::new().enroll(
                EnrollScript::default()
                    .produces(FingerId(3))
                    .retry(RetryReason::NotCentered),
            ),
        )
        .build();
    block_on(dev.open()).unwrap();

    let (result, log) = enroll(&mut dev, Finger::RightThumb);
    assert!(result.is_ok());

    // Six reports: the retry (stage still 0) then five advances.
    assert_eq!(log.len(), 6);
    assert_eq!(log[0].completed_stages, 0); // the retry did not advance the stage
    assert_eq!(log[0].retry, Some(RetryReason::NotCentered)); // the reason is forwarded
    let counts: Vec<u32> = log.iter().map(|p| p.completed_stages).collect();
    assert_eq!(counts, vec![0, 1, 2, 3, 4, 5]);
}

#[test]
fn moc_single_stage_stores() {
    let mut dev = VirtualDeviceBuilder::chip_storage_sensor()
        .scenario(Scenario::new().enroll(EnrollScript::default().produces(FingerId(9))))
        .build();
    block_on(dev.open()).unwrap();

    let (result, log) = enroll(&mut dev, Finger::LeftMiddle);
    let print = result.unwrap();

    assert_eq!(log.len(), 1); // one enroll stage
    assert!(matches!(print.template, Template::Raw(_)));
    assert!(print.device_stored);
    // The template now lives in on-device storage.
    assert_eq!(dev.stored_prints().len(), 1);
    assert_eq!(dev.stored_prints()[0].template, print.template);
}

#[test]
fn duplicate_rejected() {
    let mut dev = VirtualDeviceBuilder::chip_storage_sensor()
        .scenario(Scenario::new().enroll(EnrollScript::default().produces(FingerId(9))))
        .build();
    block_on(dev.open()).unwrap();

    enroll(&mut dev, Finger::LeftMiddle).0.unwrap(); // first enrollment stores the template
    let (again, _) = enroll(&mut dev, Finger::LeftMiddle); // same id => same template

    assert!(matches!(again, Err(Error::DataDuplicate)));
    assert_eq!(dev.stored_prints().len(), 1); // nothing added
}

#[test]
fn data_full_rejects_enroll() {
    let mut dev = VirtualDeviceBuilder::host_image_sensor()
        .scenario(
            Scenario::new()
                .storage_full()
                .enroll(EnrollScript::default().produces(FingerId(1))),
        )
        .build();
    block_on(dev.open()).unwrap();

    let (result, _) = enroll(&mut dev, Finger::RightIndex);
    assert!(matches!(result, Err(Error::DataFull)));
}
