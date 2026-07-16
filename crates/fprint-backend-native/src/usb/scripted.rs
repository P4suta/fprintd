// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [`ScriptedTransport`]: a [`UsbTransport`] that replays recorded device bytes and records the
//! host's writes, so [`UsbFrameSource`](super::source::UsbFrameSource) runs end-to-end with no
//! hardware and no `usb` feature.
//!
//! It is the generalization of the ad-hoc mock the tests once carried: bulk-in reads are answered
//! from a queue of device-to-host payloads (scripted directly, or lifted out of a [`Session`]'s
//! [`UsbTransfer::BulkIn`] entries), and every bulk-out/control write is appended to `sent` for a
//! test to assert on. Because the same driver drives it, a scripted capture verifies through the
//! real protocol → transport → frame-assembly → detect → match path over deterministic bytes.
//!
//! A read with nothing left to replay is a [`Error::Transport`], not a panic: an exhausted script
//! is the offline analogue of a sensor that stopped returning data, and the driver already treats
//! that as a transport failure.

use std::collections::VecDeque;

use fprint_core::{Error, Result};

use crate::frame::Frame;
use crate::usb::proto;
use crate::usb::transport::UsbTransport;
use crate::usb::wire::{Session, UsbTransfer};

/// A [`UsbTransport`] whose bulk-in responses are pre-loaded and whose writes are recorded.
#[derive(Clone, Debug, Default)]
pub struct ScriptedTransport {
    /// Device-to-host payloads handed out by successive `bulk_in` calls, in order.
    inbox: VecDeque<Vec<u8>>,
    /// Every payload written via `bulk_out` / `control`, for assertions.
    sent: Vec<Vec<u8>>,
}

impl ScriptedTransport {
    /// An empty transport: nothing to replay, nothing recorded yet.
    #[must_use]
    pub fn new() -> Self {
        ScriptedTransport::default()
    }

    /// Build a transport that replays a recording's device-to-host bytes, in wire order.
    ///
    /// Only the [`UsbTransfer::BulkIn`] entries are replayed: they are the bytes the device
    /// returned. The host-to-device entries (`Control`/`BulkOut`) describe what the driver will
    /// write, and the driver writes them itself — they are recorded into `sent` as they happen.
    #[must_use]
    pub fn from_transfers(transfers: &[UsbTransfer]) -> Self {
        let inbox = transfers
            .iter()
            .filter_map(|t| match t {
                UsbTransfer::BulkIn { data, .. } => Some(data.clone()),
                _ => None,
            })
            .collect();
        ScriptedTransport {
            inbox,
            sent: Vec::new(),
        }
    }

    /// Build a transport that replays a [`Session`]'s recorded device bytes.
    #[must_use]
    pub fn from_session(session: &Session) -> Self {
        Self::from_transfers(&session.transfers)
    }

    /// Queue one device-to-host bulk-in payload to be replayed by a later `bulk_in`.
    pub fn push_bulk_in(&mut self, data: Vec<u8>) {
        self.inbox.push_back(data);
    }

    /// Script one on-the-wire frame: the self-describing header followed by the pixel payload — the
    /// exact pair one [`UsbFrameSource::capture`](super::source::UsbFrameSource) reads back (a header
    /// `bulk_in`, then a payload `bulk_in`).
    pub fn push_frame(&mut self, frame: &Frame) {
        let (w, h) = frame_dims(frame);
        self.push_bulk_in(proto::encode_frame_header(w, h));
        self.push_bulk_in(frame.data.clone());
    }

    /// The host-to-device payloads written so far, in order — the bulk-out/control traffic a test
    /// asserts the driver issued.
    #[must_use]
    pub fn sent(&self) -> &[Vec<u8>] {
        &self.sent
    }
}

impl UsbTransport for ScriptedTransport {
    async fn bulk_out(&mut self, _ep: u8, data: &[u8]) -> Result<()> {
        self.sent.push(data.to_vec());
        Ok(())
    }

    async fn bulk_in(&mut self, _ep: u8, _len: usize) -> Result<Vec<u8>> {
        self.inbox
            .pop_front()
            .ok_or_else(|| Error::Transport("scripted transport inbox exhausted".to_string()))
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

/// A frame's `(width, height)` as the `u16` pair the header carries.
fn frame_dims(frame: &Frame) -> (u16, u16) {
    let w = u16::try_from(frame.width).expect("frame width fits u16");
    let h = u16::try_from(frame.height).expect("frame height fits u16");
    (w, h)
}
