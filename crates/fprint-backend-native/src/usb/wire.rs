// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A transport-agnostic record of USB traffic: the bytes a driver writes and the bytes a device
//! returns, captured as data so they can be replayed with no hardware.
//!
//! One [`UsbTransfer`] is a single bulk/control exchange, oriented by who sent the payload:
//! [`UsbTransfer::Control`] and [`UsbTransfer::BulkOut`] are host-to-device writes; [`UsbTransfer::BulkIn`]
//! is the device-to-host half — the bytes the sensor returned. A [`Session`] is an ordered run of
//! these plus the optional device identity they belong to, enough to both script a bring-up
//! handshake (the [`vfs5011`](super::vfs5011) sequences are `Vec<UsbTransfer>`) and drive
//! [`super::scripted::ScriptedTransport`] end-to-end.
//!
//! The model is deliberately hardware- and time-free: no timestamps, no endpoint state, just the
//! bytes in order. That is what keeps replay deterministic and identical on every platform.

/// One USB bulk or control transfer, recorded as bytes and oriented by direction.
///
/// `Control`/`BulkOut` carry a payload the host wrote; `BulkIn` carries the payload the device
/// returned. Keeping the device-to-host half explicit is what lets a whole capture round-trip: a
/// recorded `BulkIn` is exactly what a replaying transport hands back on the next read.
///
/// Under the `serde` feature each variant serializes with its `data` as a lowercase hex string, so a
/// recorded transfer is human-diffable in a `.cassette` file.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum UsbTransfer {
    /// A control transfer: raw `bmRequestType`, `bRequest`, `wValue`, `wIndex`, then the payload.
    Control {
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        #[cfg_attr(feature = "serde", serde(with = "hex_payload"))]
        data: Vec<u8>,
    },
    /// A host-to-device write to bulk-out endpoint `ep`.
    BulkOut {
        ep: u8,
        #[cfg_attr(feature = "serde", serde(with = "hex_payload"))]
        data: Vec<u8>,
    },
    /// A device-to-host read from bulk-in endpoint `ep`: the bytes the device returned.
    BulkIn {
        ep: u8,
        #[cfg_attr(feature = "serde", serde(with = "hex_payload"))]
        data: Vec<u8>,
    },
}

/// A USB device identity (vendor/product id), the match a future enumerator keys on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UsbId {
    pub vid: u16,
    pub pid: u16,
}

/// An ordered run of [`UsbTransfer`]s and the device they belong to: a portable, replayable
/// recording of one driver-device conversation.
///
/// `device` is optional because a fragment of traffic (an init sequence, a single capture) is
/// useful without naming a unit. The order of `transfers` is the wire order and is what replay
/// depends on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Session {
    pub device: Option<UsbId>,
    pub transfers: Vec<UsbTransfer>,
}

impl Session {
    /// An empty session with no device recorded.
    #[must_use]
    pub fn new() -> Self {
        Session::default()
    }

    /// An empty session tagged with the device its traffic belongs to.
    #[must_use]
    pub fn for_device(device: UsbId) -> Self {
        Session {
            device: Some(device),
            transfers: Vec::new(),
        }
    }

    /// Append one transfer, returning `&mut self` for chaining.
    pub fn push(&mut self, transfer: UsbTransfer) -> &mut Self {
        self.transfers.push(transfer);
        self
    }

    /// The device-to-host payloads in order — what a replaying transport hands back on successive
    /// bulk-in reads.
    pub fn bulk_in_payloads(&self) -> impl Iterator<Item = &[u8]> {
        self.transfers.iter().filter_map(|t| match t {
            UsbTransfer::BulkIn { data, .. } => Some(data.as_slice()),
            _ => None,
        })
    }
}

/// A `serde` adapter that carries a `Vec<u8>` payload as a lowercase hex string in a human-readable
/// format (what a `.cassette` uses), and as raw bytes in a binary one.
///
/// Text is the point: a recorded transfer diffs and reads as `"01fe0400"`, not a decimal byte array.
/// Named in each payload field's `#[serde(with = "hex_payload")]`.
#[cfg(feature = "serde")]
mod hex_payload {
    use std::fmt::Write as _;

    use serde::{Deserialize as _, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            let mut hex = String::with_capacity(bytes.len() * 2);
            for b in bytes {
                let _ = write!(hex, "{b:02x}");
            }
            serializer.serialize_str(&hex)
        } else {
            serializer.serialize_bytes(bytes)
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        if deserializer.is_human_readable() {
            let hex = String::deserialize(deserializer)?;
            decode(&hex).map_err(serde::de::Error::custom)
        } else {
            Vec::<u8>::deserialize(deserializer)
        }
    }

    /// Decode an even-length lowercase/uppercase hex string into bytes.
    fn decode(hex: &str) -> Result<Vec<u8>, String> {
        if hex.len() % 2 != 0 {
            return Err(format!(
                "hex payload has an odd digit count ({}) — each byte is two hex digits",
                hex.len()
            ));
        }
        (0..hex.len())
            .step_by(2)
            .map(|i| {
                u8::from_str_radix(&hex[i..i + 2], 16)
                    .map_err(|_| format!("`{}` is not a hex byte", &hex[i..i + 2]))
            })
            .collect()
    }
}
