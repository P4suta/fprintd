// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [`ImageDevice`] driving the real detect → match pipeline over a synthetic capture source.
//!
//! This exercises the host-image seam through the whole [`fprint_core::Device`] trait, hardware-free:
//! a [`SyntheticFrameSource`] renders a reproducible fingerprint, `enroll` detects it into an NBIS
//! template stage by stage, and `verify` / `identify` score a fresh scan with the real BOZORTH3
//! matcher. A self-capture matches; a stranger does not; identify picks the right gallery slot;
//! scripted retries advance no stage; and a dropped enroll future commits nothing.

mod common;
use common::{block_on, poll_n};

use fprint_backend_native::{ImageDevice, SyntheticFrameSource};
use fprint_core::{
    Device, DeviceFeature, DeviceId, DeviceInfo, DriverId, EnrollProgress, Finger, Print,
    RetryReason, ScanType, Template,
};

/// Match threshold, following the `end_to_end.rs` / `real_matching.rs` convention.
const THRESHOLD: u32 = 40;

/// Number of enrollment stages (kept small; each stage runs a full MINDTCT detection).
const STAGES: u32 = 3;

fn info() -> DeviceInfo {
    DeviceInfo {
        id: DeviceId("virtual_image".to_string()),
        driver: DriverId("virtual_image".to_string()),
        name: "Synthetic Image Sensor".to_string(),
        scan_type: ScanType::Press,
        features: DeviceFeature::CAPTURE | DeviceFeature::VERIFY | DeviceFeature::IDENTIFY,
        enroll_stages: STAGES,
    }
}

fn device(source: SyntheticFrameSource) -> ImageDevice<SyntheticFrameSource> {
    ImageDevice::new(info(), source, THRESHOLD)
}

fn open_device(source: SyntheticFrameSource) -> ImageDevice<SyntheticFrameSource> {
    let mut dev = device(source);
    block_on(dev.open()).unwrap();
    dev
}

/// Enroll on a fresh reference device and return the completed print.
fn enroll_reference() -> Print {
    let mut dev = open_device(SyntheticFrameSource::reference());
    block_on(dev.enroll(Print::new_for_enroll(Finger::LeftIndex), &mut |_p| {})).unwrap()
}

#[test]
fn enroll_advances_through_every_stage() {
    let mut dev = open_device(SyntheticFrameSource::reference());

    let mut seen: Vec<EnrollProgress> = Vec::new();
    let print = block_on(dev.enroll(Print::new_for_enroll(Finger::RightThumb), |p| {
        seen.push(p);
    }))
    .unwrap();

    // One clean progress report per stage, counting up to the total.
    assert_eq!(seen.len(), STAGES as usize);
    for (i, p) in seen.iter().enumerate() {
        assert_eq!(p.completed_stages, i as u32 + 1);
        assert_eq!(p.total_stages, STAGES);
        assert!(p.retry.is_none());
    }

    assert_eq!(print.finger, Some(Finger::RightThumb));
    assert!(matches!(print.template, Template::Nbis(_)));
    assert!(
        !print.device_stored,
        "host-image sensors store nothing on-chip"
    );
}

#[test]
fn scripted_retry_reports_without_advancing() {
    // The first capture is a weak one; the stage count must not move for it.
    let source =
        SyntheticFrameSource::reference().with_retries(vec![(0, RetryReason::NotCentered)]);
    let mut dev = open_device(source);

    let mut seen: Vec<EnrollProgress> = Vec::new();
    block_on(dev.enroll(Print::new_for_enroll(Finger::LeftIndex), |p| seen.push(p))).unwrap();

    // The very first report is the retry, and it did not advance the stage.
    let first = &seen[0];
    assert_eq!(first.retry, Some(RetryReason::NotCentered));
    assert_eq!(first.completed_stages, 0);

    // Exactly one retry report, and the stages still complete cleanly.
    assert_eq!(seen.iter().filter(|p| p.retry.is_some()).count(), 1);
    let advances: Vec<&EnrollProgress> = seen.iter().filter(|p| p.retry.is_none()).collect();
    assert_eq!(advances.len(), STAGES as usize);
    assert_eq!(advances.last().unwrap().completed_stages, STAGES);
}

#[test]
fn self_capture_verifies() {
    let enrolled = enroll_reference();
    let mut dev = open_device(SyntheticFrameSource::reference());

    let outcome = block_on(dev.verify(&enrolled)).unwrap();
    assert!(outcome.matched, "the same synthetic finger must verify");
    assert!(outcome.scanned.is_some(), "a host sensor surfaces the scan");
}

#[test]
fn stranger_does_not_verify() {
    let enrolled = enroll_reference();
    let mut dev = open_device(SyntheticFrameSource::stranger());

    let outcome = block_on(dev.verify(&enrolled)).unwrap();
    assert!(
        !outcome.matched,
        "an unrelated synthetic finger must not verify"
    );
}

#[test]
fn identify_returns_the_matching_gallery_index() {
    // A stranger at index 0, the true reference finger at index 1.
    let stranger = {
        let mut dev = open_device(SyntheticFrameSource::stranger());
        block_on(dev.enroll(Print::new_for_enroll(Finger::LeftIndex), &mut |_p| {})).unwrap()
    };
    let reference = enroll_reference();
    let gallery = vec![stranger, reference];

    let mut dev = open_device(SyntheticFrameSource::reference());
    let outcome = block_on(dev.identify(&gallery)).unwrap();
    assert_eq!(outcome.match_index, Some(1));
    assert!(outcome.scanned.is_some(), "identify surfaces the scan too");
}

#[test]
fn dropping_enroll_commits_nothing() {
    let mut dev = open_device(SyntheticFrameSource::reference());

    {
        let mut on_progress = |_p| {};
        // `Box::pin` makes the (non-Unpin) enroll future `Unpin` so `poll_n` can drive it.
        let mut fut =
            Box::pin(dev.enroll(Print::new_for_enroll(Finger::LeftIndex), &mut on_progress));
        // Two polls cannot finish a three-stage enroll (one poll boundary per capture stage).
        let outcome = poll_n(&mut fut, 2);
        assert!(outcome.is_none(), "no Print is returned mid-enrollment");
        drop(fut); // <- cancellation
    }

    // The device is still open, and a fresh enrollment completes cleanly.
    assert!(dev.is_open());
    let print =
        block_on(dev.enroll(Print::new_for_enroll(Finger::LeftIndex), &mut |_p| {})).unwrap();
    assert!(matches!(print.template, Template::Nbis(_)));
}
