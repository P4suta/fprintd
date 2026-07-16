// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [`FileFrameSource`] driving the real detect → match pipeline over committed frame bytes.
//!
//! Hardware-free and platform-independent: a [`SyntheticFrameSource`] renders a deterministic
//! reference finger once; we lift its raw bytes into a [`FileFrameSource`] (exactly as a committed
//! golden corpus would be replayed) and confirm `ImageDevice<FileFrameSource>` enrolls and then
//! self-verifies through genuine MINDTCT + BOZORTH3. A second check round-trips those bytes through
//! the PGM writer/parser to prove the file format is loss-free.

use fprint_testkit::block_on;

use fprint_backend_native::{
    Capture, FileFrameSource, Frame, FrameSource, ImageDevice, SyntheticFrameSource,
};
use fprint_core::{
    Device, DeviceFeature, DeviceId, DeviceInfo, DriverId, Finger, Print, ScanType, Template,
};

/// Same threshold as `image_device.rs` — the two sources feed identical bytes to the same pipeline.
const THRESHOLD: u32 = 40;
const STAGES: u32 = 3;

fn info() -> DeviceInfo {
    DeviceInfo::new(
        DeviceId::new("file_image"),
        DriverId::new("file_image"),
        "File Image Sensor",
        ScanType::Press,
        DeviceFeature::CAPTURE | DeviceFeature::VERIFY | DeviceFeature::IDENTIFY,
        STAGES,
    )
}

/// Capture one frame out of a [`SyntheticFrameSource`] to obtain committed reference bytes — the
/// same trick the module docs describe: drive the synthetic source's `capture` once and keep the
/// `Frame`. This stands in for a raw frame checked into a golden corpus.
fn reference_frame() -> Frame {
    let mut src = SyntheticFrameSource::reference();
    match block_on(src.capture()).unwrap() {
        Capture::Frame(f) => f,
        Capture::Retry(_) => unreachable!("the reference source never retries"),
    }
}

fn open_file_device(source: FileFrameSource) -> ImageDevice<FileFrameSource> {
    let mut dev = ImageDevice::new(info(), source, THRESHOLD);
    block_on(dev.open()).unwrap();
    dev
}

#[test]
fn enroll_then_self_verify_over_file_bytes() {
    let frame = reference_frame();
    let (w, h, ppi) = (frame.width, frame.height, frame.ppi);

    // Enroll from a source built out of the raw bytes (validated against the geometry).
    let enroll_src = FileFrameSource::from_raw(frame.data.clone(), w, h, ppi).unwrap();
    let mut enroll_dev = open_file_device(enroll_src);
    let enrolled =
        block_on(enroll_dev.enroll(Print::new_for_enroll(Finger::LeftIndex), &mut |_p| {}))
            .unwrap();
    assert!(matches!(enrolled.template, Template::Nbis(_)));
    assert!(
        !enrolled.device_stored,
        "host-image sensors store nothing on-chip"
    );

    // A fresh device over the same committed bytes must self-verify.
    let verify_src = FileFrameSource::from_raw(frame.data.clone(), w, h, ppi).unwrap();
    let mut verify_dev = open_file_device(verify_src);
    let outcome = block_on(verify_dev.verify(&enrolled)).unwrap();
    assert!(
        outcome.matched,
        "the same committed frame must verify against itself"
    );
    assert!(outcome.scanned.is_some());
}

#[test]
fn pgm_round_trip_reconstructs_the_frame() {
    let frame = reference_frame();

    // Serialize to binary PGM the way a corpus would store it, then parse it back through the public
    // `FileFrameSource::from_pgm` and confirm the first captured frame is byte-identical.
    let mut pgm = format!("P5\n{} {}\n255\n", frame.width, frame.height).into_bytes();
    pgm.extend_from_slice(&frame.data);

    let mut src = FileFrameSource::from_pgm(&pgm, frame.ppi).unwrap();
    match block_on(src.capture()).unwrap() {
        Capture::Frame(parsed) => {
            assert_eq!(parsed.width, frame.width);
            assert_eq!(parsed.height, frame.height);
            assert_eq!(parsed.ppi, frame.ppi);
            assert_eq!(parsed.data, frame.data, "PGM round-trip must be loss-free");
        }
        Capture::Retry(_) => unreachable!("FileFrameSource never retries"),
    }
}

#[test]
fn from_pgm_rejects_malformed_header() {
    // Wrong magic: a text PGM ("P2") is not a binary frame source.
    let mut pgm = b"P2\n4 3\n255\n".to_vec();
    pgm.extend_from_slice(&[0u8; 12]);
    assert!(FileFrameSource::from_pgm(&pgm, 500).is_err());
}
