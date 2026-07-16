// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The Windows USBPcap pseudo-header decoder (`DLT_USBPCAP`).
//!
//! USBPcap prepends a packed, little-endian header to each captured packet, and the captured data
//! follows it. The header's `headerLen` field is the offset to that data, which absorbs the extra
//! stage byte a control transfer's header carries. The field layout is an interoperability fact,
//! reproduced from its documentation in `docs/re-capture-formats.md`.

use crate::capture::event::{DeviceKey, Endian, Stage, TransferType, UsbEvent};

/// The shortest USBPcap header (a non-control transfer): `headerLen` through `dataLength`.
const MIN_HEADER_LEN: usize = 27;
/// `info` bit 0 (`PDO_TO_FDO`): set on a packet travelling up from the device — a completion.
const INFO_PDO_TO_FDO: u8 = 0x01;

/// Decode one USBPcap packet — its pseudo-header followed by the captured data — into a
/// [`UsbEvent`]. A control submission's payload opens with the 8-byte setup packet, which this
/// lifts into [`UsbEvent::setup`]; the remaining bytes are the data stage.
///
/// Returns `None` when the packet is too short for its own header, or its transfer type is unknown.
pub(crate) fn decode(packet: &[u8], endian: Endian) -> Option<UsbEvent> {
    if packet.len() < MIN_HEADER_LEN {
        return None;
    }
    let header_len = endian.u16([packet[0], packet[1]]) as usize;
    if header_len < MIN_HEADER_LEN || packet.len() < header_len {
        return None;
    }

    let id = {
        let mut b = [0u8; 8];
        b.copy_from_slice(&packet[2..10]);
        match endian {
            Endian::Little => u64::from_le_bytes(b),
            Endian::Big => u64::from_be_bytes(b),
        }
    };
    let info = packet[16];
    let bus = endian.u16([packet[17], packet[18]]);
    let address = endian.u16([packet[19], packet[20]]);
    let endpoint = packet[21];
    let transfer_type = TransferType::from_code(packet[22])?;
    let data_length = endian.u32([packet[23], packet[24], packet[25], packet[26]]) as usize;

    let stage = if info & INFO_PDO_TO_FDO != 0 {
        Stage::Complete
    } else {
        Stage::Submit
    };

    let data_end = (header_len + data_length).min(packet.len());
    let mut data = packet[header_len..data_end].to_vec();

    // A control submission carries the 8-byte setup packet at the front of its payload; the rest is
    // the OUT data stage. A completion carries only the returned data.
    let setup =
        if transfer_type == TransferType::Control && stage == Stage::Submit && data.len() >= 8 {
            let mut s = [0u8; 8];
            s.copy_from_slice(&data[0..8]);
            data.drain(0..8);
            Some(s)
        } else {
            None
        };

    Some(UsbEvent {
        id,
        key: DeviceKey { bus, address },
        endpoint,
        transfer_type,
        stage,
        setup,
        data,
    })
}
