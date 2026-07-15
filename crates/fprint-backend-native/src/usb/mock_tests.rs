// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A scripted [`UsbTransport`] that drives [`UsbFrameSource`] end-to-end with no hardware and no
//! `usb` feature.
//!
//! The mock records every bulk-out/control write and replays a canned queue of bulk-in responses, so
//! `ImageDevice<UsbFrameSource<MockTransport>>` runs the real
//! protocol → transport → frame-assembly → detect → match path over deterministic bytes. The bytes
//! come from [`crate::SyntheticFrameSource`] (a genuine minutiae-rich reference finger), lifted into
//! the transport's inbox exactly as they would arrive off the wire; a self-capture therefore
//! verifies through real MINDTCT + BOZORTH3, proving the assembly is correct — all on the default
//! (feature-free) build.

use std::collections::VecDeque;

use fprint_core::{
    Device, DeviceFeature, DeviceId, DeviceInfo, DriverId, Error, Finger, Print, Result, ScanType,
    Template,
};

use crate::frame::Frame;
use crate::frame_source::{Capture, FrameSource};
use crate::test_exec::block_on;
use crate::usb::proto;
use crate::usb::source::UsbFrameSource;
use crate::usb::transport::UsbTransport;
use crate::{ImageDevice, SyntheticFrameSource};

/// Same threshold / stage count as `tests/image_device.rs`: identical bytes, identical pipeline.
const THRESHOLD: u32 = 40;
const STAGES: u32 = 3;

/// A UsbTransport whose bulk-in responses are pre-loaded and whose writes are recorded.
struct MockTransport {
    /// Canned responses handed out by successive `bulk_in` calls, in order.
    inbox: VecDeque<Vec<u8>>,
    /// Every payload written via `bulk_out` / `control`, for assertions.
    sent: Vec<Vec<u8>>,
}

impl MockTransport {
    fn new() -> Self {
        MockTransport {
            inbox: VecDeque::new(),
            sent: Vec::new(),
        }
    }

    /// Script one on-the-wire frame: the self-describing header followed by the pixel payload, the
    /// exact pair `UsbFrameSource::capture` reads back (a header `bulk_in`, then a payload `bulk_in`).
    fn push_frame(&mut self, frame: &Frame) {
        let w = u16::try_from(frame.width).expect("test frame width fits u16");
        let h = u16::try_from(frame.height).expect("test frame height fits u16");
        self.inbox.push_back(proto::encode_frame_header(w, h));
        self.inbox.push_back(frame.data.clone());
    }
}

impl UsbTransport for MockTransport {
    async fn bulk_out(&mut self, _ep: u8, data: &[u8]) -> Result<()> {
        self.sent.push(data.to_vec());
        Ok(())
    }

    async fn bulk_in(&mut self, _ep: u8, _len: usize) -> Result<Vec<u8>> {
        self.inbox
            .pop_front()
            .ok_or_else(|| Error::Transport("mock inbox exhausted".to_string()))
    }

    async fn control(
        &mut self,
        _request_type: u8,
        _request: u8,
        _value: u16,
        _index: u16,
        data: &[u8],
    ) -> Result<Vec<u8>> {
        // Init/deinit control transfers need no response; record the payload and acknowledge.
        self.sent.push(data.to_vec());
        Ok(Vec::new())
    }
}

fn info() -> DeviceInfo {
    DeviceInfo {
        id: DeviceId("usb_vfs5011".to_string()),
        driver: DriverId("vfs5011".to_string()),
        name: "VFS5011 (mock transport)".to_string(),
        scan_type: ScanType::Press,
        features: DeviceFeature::CAPTURE | DeviceFeature::VERIFY | DeviceFeature::IDENTIFY,
        enroll_stages: STAGES,
    }
}

/// One deterministic reference frame, lifted out of the synthetic source's `capture`.
fn reference_frame() -> Frame {
    let mut src = SyntheticFrameSource::reference();
    match block_on(src.capture()).unwrap() {
        Capture::Frame(f) => f,
        Capture::Retry(_) => unreachable!("the reference source never retries"),
    }
}

fn device_serving(frames: usize) -> ImageDevice<UsbFrameSource<MockTransport>> {
    let frame = reference_frame();
    let mut mock = MockTransport::new();
    for _ in 0..frames {
        mock.push_frame(&frame);
    }
    let mut dev = ImageDevice::new(info(), UsbFrameSource::new(mock), THRESHOLD);
    block_on(dev.open()).unwrap(); // runs the vfs5011 init sequence through the mock
    dev
}

#[test]
fn capture_assembles_the_scripted_frame_exactly() {
    let frame = reference_frame();
    let mut mock = MockTransport::new();
    mock.push_frame(&frame);
    let mut source = UsbFrameSource::new(mock);

    match block_on(source.capture()).unwrap() {
        Capture::Frame(assembled) => {
            assert_eq!(assembled.width, frame.width);
            assert_eq!(assembled.height, frame.height);
            assert_eq!(
                assembled.data, frame.data,
                "assembly must reproduce the wire bytes"
            );
        }
        Capture::Retry(_) => unreachable!("the mock always yields a frame"),
    }
}

#[test]
fn capture_sends_the_capture_command() {
    let frame = reference_frame();
    let mut mock = MockTransport::new();
    mock.push_frame(&frame);
    let mut source = UsbFrameSource::new(mock);
    let _ = block_on(source.capture()).unwrap();

    // The recorded writes must include the encoded capture command.
    assert!(
        source_sent(&source).contains(&proto::encode_capture_cmd()),
        "capture must issue the encoded capture command on bulk-out"
    );
}

/// Small helper: reach the mock's recorded writes back out of the driver for assertions.
fn source_sent(source: &UsbFrameSource<MockTransport>) -> Vec<Vec<u8>> {
    source.transport_ref().sent.clone()
}

#[test]
fn enroll_then_self_verify_through_the_mock() {
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
        "a self-capture must verify through the mock transport"
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
