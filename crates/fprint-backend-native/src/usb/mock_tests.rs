// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [`ScriptedTransport`] driving [`UsbFrameSource`] end-to-end with no hardware and no `usb` feature.
//!
//! The transport records every bulk-out/control write and replays a canned queue of bulk-in
//! responses, so `ImageDevice<UsbFrameSource<ScriptedTransport>>` runs the real
//! protocol → transport → frame-assembly → detect → match path over deterministic bytes. The bytes
//! come from [`crate::SyntheticFrameSource`] (a genuine minutiae-rich reference finger), lifted into
//! the transport's inbox exactly as they would arrive off the wire; a self-capture therefore
//! verifies through real MINDTCT + BOZORTH3, proving the assembly is correct — all on the default
//! (feature-free) build. The same capture also round-trips through a portable [`Session`], proving
//! the recording drives the driver identically to a directly scripted transport.

use fprint_core::{
    Device, DeviceFeature, DeviceId, DeviceInfo, DriverId, Error, Finger, Print, ScanType, Template,
};

use crate::frame::Frame;
use crate::frame_source::{Capture, FrameSource};
use crate::usb::proto;
use crate::usb::scripted::ScriptedTransport;
use crate::usb::source::UsbFrameSource;
use crate::usb::vfs5011;
use crate::usb::wire::{Session, UsbId, UsbTransfer};
use crate::{ImageDevice, SyntheticFrameSource};
use fprint_testkit::block_on;

/// Same threshold / stage count as `tests/image_device.rs`: identical bytes, identical pipeline.
const THRESHOLD: u32 = 40;
const STAGES: u32 = 3;

fn info() -> DeviceInfo {
    DeviceInfo::new(
        DeviceId::new("usb_vfs5011"),
        DriverId::new("vfs5011"),
        "VFS5011 (scripted transport)",
        ScanType::Press,
        DeviceFeature::CAPTURE | DeviceFeature::VERIFY | DeviceFeature::IDENTIFY,
        STAGES,
    )
}

/// One deterministic reference frame, lifted out of the synthetic source's `capture`.
fn reference_frame() -> Frame {
    let mut src = SyntheticFrameSource::reference();
    match block_on(src.capture()).unwrap() {
        Capture::Frame(f) => f,
        Capture::Retry(_) => unreachable!("the reference source never retries"),
    }
}

/// Open an [`ImageDevice`] over `transport` (running the vfs5011 init sequence through it).
fn device_from(transport: ScriptedTransport) -> ImageDevice<UsbFrameSource<ScriptedTransport>> {
    let mut dev = ImageDevice::new(info(), UsbFrameSource::new(transport), THRESHOLD);
    block_on(dev.open()).unwrap(); // runs the vfs5011 init sequence through the transport
    dev
}

/// A device serving `frames` scripted captures of the reference finger.
fn device_serving(frames: usize) -> ImageDevice<UsbFrameSource<ScriptedTransport>> {
    let frame = reference_frame();
    let mut transport = ScriptedTransport::new();
    for _ in 0..frames {
        transport.push_frame(&frame);
    }
    device_from(transport)
}

/// A portable recording of `frames` reference captures: the two device-to-host bulk-ins per capture
/// (header, then payload), tagged with the VFS5011 identity — a [`Session`] a [`ScriptedTransport`]
/// can replay.
fn session_serving(frames: usize) -> Session {
    let frame = reference_frame();
    let w = u16::try_from(frame.width).expect("reference frame width fits u16");
    let h = u16::try_from(frame.height).expect("reference frame height fits u16");
    let mut session = Session::for_device(UsbId {
        vid: vfs5011::VENDOR_ID,
        pid: vfs5011::PRODUCT_ID,
    });
    for _ in 0..frames {
        session
            .push(UsbTransfer::BulkIn {
                ep: vfs5011::EP_IN,
                data: proto::encode_frame_header(w, h),
            })
            .push(UsbTransfer::BulkIn {
                ep: vfs5011::EP_IN,
                data: frame.data.clone(),
            });
    }
    session
}

#[test]
fn capture_assembles_the_scripted_frame_exactly() {
    let frame = reference_frame();
    let mut transport = ScriptedTransport::new();
    transport.push_frame(&frame);
    let mut source = UsbFrameSource::new(transport);

    match block_on(source.capture()).unwrap() {
        Capture::Frame(assembled) => {
            assert_eq!(assembled.width, frame.width);
            assert_eq!(assembled.height, frame.height);
            assert_eq!(
                assembled.data, frame.data,
                "assembly must reproduce the wire bytes"
            );
        }
        Capture::Retry(_) => unreachable!("the transport always yields a frame"),
    }
}

#[test]
fn capture_sends_the_capture_command() {
    let frame = reference_frame();
    let mut transport = ScriptedTransport::new();
    transport.push_frame(&frame);
    let mut source = UsbFrameSource::new(transport);
    let _ = block_on(source.capture()).unwrap();

    // The recorded writes must include the encoded capture command.
    assert!(
        source
            .transport_ref()
            .sent()
            .contains(&proto::encode_capture_cmd()),
        "capture must issue the encoded capture command on bulk-out"
    );
}

#[test]
fn enroll_then_self_verify_through_the_transport() {
    // Enroll needs one frame per stage; the init sequence consumes no inbox entries.
    let mut enroll_dev = device_serving(STAGES as usize);
    let enrolled =
        block_on(enroll_dev.enroll(Print::new_for_enroll(Finger::LeftIndex), &mut |_p| {}))
            .unwrap();
    assert!(matches!(enrolled.template, Template::Nbis(_)));

    // A fresh device over the same bytes must self-verify: proto → transport → assembly → match.
    let mut verify_dev = device_serving(1);
    let outcome = block_on(verify_dev.verify(&enrolled)).unwrap();
    assert!(
        outcome.matched,
        "a self-capture must verify through the scripted transport"
    );
    assert!(outcome.scanned.is_some());
}

#[test]
fn session_recording_drives_a_self_verify() {
    // Build the enroll device from a portable Session rather than a directly scripted transport:
    // the recording alone must carry enough to enroll.
    let mut enroll_dev = device_from(ScriptedTransport::from_session(&session_serving(
        STAGES as usize,
    )));
    let enrolled =
        block_on(enroll_dev.enroll(Print::new_for_enroll(Finger::LeftIndex), &mut |_p| {}))
            .unwrap();
    assert!(matches!(enrolled.template, Template::Nbis(_)));

    // A one-capture Session must then self-verify through the same replay path.
    let mut verify_dev = device_from(ScriptedTransport::from_session(&session_serving(1)));
    let outcome = block_on(verify_dev.verify(&enrolled)).unwrap();
    assert!(
        outcome.matched,
        "a Session-scripted self-capture must verify through the replay transport"
    );
    assert!(outcome.scanned.is_some());
}

#[test]
fn exhausted_inbox_surfaces_a_transport_error() {
    // No frames scripted: the first header read fails as a transport error, not a panic.
    let mut dev = device_serving(0);
    let err = block_on(dev.verify(&Print::new_for_enroll(Finger::LeftIndex)));
    assert!(matches!(err, Err(Error::Transport(_))));
}
