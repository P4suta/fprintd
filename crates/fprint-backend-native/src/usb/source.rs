// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [`UsbFrameSource`]: a [`crate::FrameSource`] driven over any [`UsbTransport`].
//!
//! `ImageDevice<UsbFrameSource<NusbTransport>>` runs the host-image pipeline over a Validity sensor
//! with no change to `ImageDevice`. Being generic over the transport, the same driver runs against
//! a scripted mock offline, so the protocol → transport → frame-assembly path is exercised on any
//! platform without hardware.
//!
//! `capture` awaits `crate::yield_now` once before touching the transport, so it keeps exactly one
//! poll boundary per capture — the drop-cancel point [`crate::ImageDevice::enroll`] relies on —
//! whether the transport is genuinely async (nusb) or resolves immediately (a mock).

use fprint_core::{Error, Result};

use crate::frame_source::{Capture, FrameSource};
use crate::usb::proto;
use crate::usb::transport::UsbTransport;
use crate::usb::vfs5011;
use crate::usb::wire::UsbTransfer;

/// A host-image capture source that speaks the VFS5011 protocol over a [`UsbTransport`].
pub struct UsbFrameSource<T: UsbTransport> {
    transport: T,
    /// Bulk-out endpoint for commands.
    ep_out: u8,
    /// Bulk-in endpoint for the image stream.
    ep_in: u8,
    /// Scan resolution stamped on every assembled frame.
    ppi: u16,
}

impl<T: UsbTransport> UsbFrameSource<T> {
    /// Build a VFS5011 driver over `transport`, using this crate's documented VFS5011 endpoints and
    /// scan resolution (see the `vfs5011` module — several of those values are HW-verification placeholders).
    pub fn new(transport: T) -> Self {
        UsbFrameSource {
            transport,
            ep_out: vfs5011::EP_OUT,
            ep_in: vfs5011::EP_IN,
            ppi: vfs5011::PPI,
        }
    }

    /// Borrow the underlying transport (test-only, for asserting on a mock's recorded traffic).
    #[cfg(test)]
    pub(crate) fn transport_ref(&self) -> &T {
        &self.transport
    }

    /// Replay one handshake step through the transport.
    ///
    /// A handshake only ever writes to the device, so a [`UsbTransfer::BulkIn`] here is a
    /// construction bug in the sequence, not a wire condition — the arm/disarm sequences are this
    /// crate's own [`vfs5011`] data and never contain one.
    async fn run_step(&mut self, step: &UsbTransfer) -> Result<()> {
        match step {
            UsbTransfer::Control {
                request_type,
                request,
                value,
                index,
                data,
            } => self
                .transport
                .control(*request_type, *request, *value, *index, data)
                .await
                .map(|_| ()),
            UsbTransfer::BulkOut { ep, data } => self.transport.bulk_out(*ep, data).await,
            UsbTransfer::BulkIn { .. } => Err(Error::Other(
                "init/deinit sequence contains a bulk-in transfer, which arm/disarm never replay"
                    .to_string(),
            )),
        }
    }
}

impl<T: UsbTransport> FrameSource for UsbFrameSource<T> {
    async fn capture(&mut self) -> Result<Capture> {
        // One poll boundary per capture (the drop-cancel point), before any transport I/O.
        crate::yield_now::yield_now().await;

        // Ask the sensor to capture, then read the self-describing header and the pixel payload.
        self.transport
            .bulk_out(self.ep_out, &proto::encode_capture_cmd())
            .await?;

        let header = self
            .transport
            .bulk_in(self.ep_in, proto::FRAME_HEADER_LEN)
            .await?;
        let (width, height) = proto::parse_frame_header(&header)?;

        let count = width
            .checked_mul(height)
            .ok_or_else(|| Error::Protocol(format!("frame geometry {width}x{height} overflows")))?;
        // Reject a header claiming more than the sensor's nominal maximum, so a garbled header can
        // never drive an unbounded bulk-in read.
        if count > vfs5011::MAX_FRAME_BYTES {
            return Err(Error::Protocol(format!(
                "frame header claims {count} bytes, over the {} maximum",
                vfs5011::MAX_FRAME_BYTES
            )));
        }
        let payload = self.transport.bulk_in(self.ep_in, count).await?;

        // A real swipe sensor may stream several chunks; the mock delivers one. `assemble_frame`
        // validates the total against the geometry either way.
        let frame = proto::assemble_frame(&[&payload], width, height, self.ppi)?;
        Ok(Capture::Frame(frame))
    }

    async fn arm(&mut self) -> Result<()> {
        for step in vfs5011::init_sequence() {
            self.run_step(&step).await?;
        }
        Ok(())
    }

    async fn disarm(&mut self) -> Result<()> {
        for step in vfs5011::deinit_sequence() {
            self.run_step(&step).await?;
        }
        Ok(())
    }
}
