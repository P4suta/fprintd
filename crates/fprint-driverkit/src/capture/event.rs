// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The format-neutral middle of `fpdev import`: one [`UsbEvent`] per captured URB stage, and the
//! [`assemble`] pass that folds a stream of them into replayable [`UsbTransfer`]s.
//!
//! Every capture format (pcapng USBPcap, pcapng/pcap usbmon, usbmon text) decodes to the same
//! [`UsbEvent`] shape, so the submit/complete pairing, the endpoint-direction rules, and the device
//! descriptor sniffing live here once. A URB is captured twice — a *submit* going down to the
//! device and a *complete* coming back up — and the payload lives on whichever half matches the
//! transfer's data direction: an OUT write carries its bytes on the submit, an IN read on the
//! complete. [`assemble`] emits each [`UsbTransfer`] at the moment its bytes are known, which is the
//! wire order a replay depends on.

use std::collections::HashMap;

use fprint_backend_native::{UsbId, UsbTransfer};

/// Byte order of a capture's binary pseudo-headers, taken from its container (the pcapng section
/// header or the classic-pcap magic). The usbmon and USBPcap headers are written in the capturing
/// host's endianness, which is the container's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Endian {
    Little,
    Big,
}

impl Endian {
    pub(crate) fn u16(self, b: [u8; 2]) -> u16 {
        match self {
            Endian::Little => u16::from_le_bytes(b),
            Endian::Big => u16::from_be_bytes(b),
        }
    }

    pub(crate) fn u32(self, b: [u8; 4]) -> u32 {
        match self {
            Endian::Little => u32::from_le_bytes(b),
            Endian::Big => u32::from_be_bytes(b),
        }
    }
}

/// The four USB transfer types, encoded identically by usbmon and USBPcap: ISO=0, INT=1, CTRL=2,
/// BULK=3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TransferType {
    Isochronous,
    Interrupt,
    Control,
    Bulk,
}

impl TransferType {
    /// Decode the on-wire transfer-type code shared by the usbmon and USBPcap pseudo-headers.
    pub(crate) fn from_code(code: u8) -> Option<TransferType> {
        match code {
            0 => Some(TransferType::Isochronous),
            1 => Some(TransferType::Interrupt),
            2 => Some(TransferType::Control),
            3 => Some(TransferType::Bulk),
            _ => None,
        }
    }
}

/// Which half of a URB's life a captured event is: the host's submission down to the device, or the
/// device's completion back up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    Submit,
    Complete,
}

/// The device a transfer belongs to, keyed the way a capture keys it: by bus and device address.
/// A capture identifies devices by this pair, not by vendor/product id, so this is the unit the
/// device filter isolates and the descriptor sniffer maps to a [`UsbId`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct DeviceKey {
    pub(crate) bus: u16,
    pub(crate) address: u16,
}

/// One decoded capture record: a single submit-or-complete of one URB, format-neutral.
///
/// `endpoint` is the raw endpoint address including the `0x80` IN bit, matching the `ep` a
/// [`UsbTransfer`] carries. `setup` is the 8-byte control setup packet, present on a control
/// submission. `data` is the payload this stage carried — empty on the half that does not move the
/// data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UsbEvent {
    pub(crate) id: u64,
    pub(crate) key: DeviceKey,
    pub(crate) endpoint: u8,
    pub(crate) transfer_type: TransferType,
    pub(crate) stage: Stage,
    pub(crate) setup: Option<[u8; 8]>,
    pub(crate) data: Vec<u8>,
}

/// A `bmRequestType` with the direction bit set marks a device-to-host (IN) control transfer.
fn setup_is_in(setup: &[u8; 8]) -> bool {
    setup[0] & 0x80 != 0
}

/// The control request's `(request_type, request, value, index)`, read from a setup packet. `value`
/// and `index` are logical 16-bit values in the byte order the setup carries them (little-endian on
/// the wire; the usbmon text decoder hands them over already in logical order).
fn setup_fields(setup: &[u8; 8]) -> (u8, u8, u16, u16) {
    let value = u16::from_le_bytes([setup[2], setup[3]]);
    let index = u16::from_le_bytes([setup[4], setup[5]]);
    (setup[0], setup[1], value, index)
}

/// One assembled transfer tagged with the device it belongs to.
pub(crate) struct DeviceTransfer {
    pub(crate) key: DeviceKey,
    pub(crate) transfer: UsbTransfer,
}

/// The result of folding a capture: the transfers in wire order, and every device identity the
/// capture revealed through a `GET_DESCRIPTOR(DEVICE)` response.
pub(crate) struct Assembled {
    pub(crate) transfers: Vec<DeviceTransfer>,
    pub(crate) descriptors: HashMap<DeviceKey, UsbId>,
}

/// Fold a stream of captured events into replayable transfers.
///
/// An OUT transfer's bytes are emitted at its submit, an IN transfer's at its complete, so a
/// transfer appears in the output exactly when its payload becomes known — the wire order. Control
/// submissions that read (IN) are held until their completion supplies the returned bytes; every
/// other stage is resolved in place. A `GET_DESCRIPTOR(DEVICE)` completion also records the
/// responding device's vendor/product id.
pub(crate) fn assemble(events: &[UsbEvent]) -> Assembled {
    let mut transfers = Vec::new();
    let mut descriptors = HashMap::new();
    // Control reads awaiting their completion, keyed by URB id: the setup is on the submit, the
    // returned bytes on the complete.
    let mut pending_control_in: HashMap<u64, (DeviceKey, [u8; 8])> = HashMap::new();

    for ev in events {
        let is_in = ev.endpoint & 0x80 != 0;
        match (ev.transfer_type, ev.stage) {
            (TransferType::Control, Stage::Submit) => {
                let Some(setup) = ev.setup else { continue };
                if setup_is_in(&setup) {
                    pending_control_in.insert(ev.id, (ev.key, setup));
                } else {
                    let (request_type, request, value, index) = setup_fields(&setup);
                    transfers.push(DeviceTransfer {
                        key: ev.key,
                        transfer: UsbTransfer::Control {
                            request_type,
                            request,
                            value,
                            index,
                            data: ev.data.clone(),
                        },
                    });
                }
            }
            (TransferType::Control, Stage::Complete) => {
                let Some((key, setup)) = pending_control_in.remove(&ev.id) else {
                    continue;
                };
                let (request_type, request, value, index) = setup_fields(&setup);
                record_descriptor(&mut descriptors, key, &setup, &ev.data);
                transfers.push(DeviceTransfer {
                    key,
                    transfer: UsbTransfer::Control {
                        request_type,
                        request,
                        value,
                        index,
                        data: ev.data.clone(),
                    },
                });
            }
            (TransferType::Bulk | TransferType::Interrupt, Stage::Submit) if !is_in => {
                transfers.push(DeviceTransfer {
                    key: ev.key,
                    transfer: UsbTransfer::BulkOut {
                        ep: ev.endpoint,
                        data: ev.data.clone(),
                    },
                });
            }
            (TransferType::Bulk | TransferType::Interrupt, Stage::Complete) if is_in => {
                transfers.push(DeviceTransfer {
                    key: ev.key,
                    transfer: UsbTransfer::BulkIn {
                        ep: ev.endpoint,
                        data: ev.data.clone(),
                    },
                });
            }
            _ => {}
        }
    }

    Assembled {
        transfers,
        descriptors,
    }
}

/// If `setup`/`data` are a `GET_DESCRIPTOR(DEVICE)` response, read the vendor/product id out of the
/// device descriptor and remember it for `key`. The device descriptor carries `idVendor` at offset
/// 8 and `idProduct` at offset 10, both little-endian.
fn record_descriptor(
    descriptors: &mut HashMap<DeviceKey, UsbId>,
    key: DeviceKey,
    setup: &[u8; 8],
    data: &[u8],
) {
    let is_get_descriptor = setup[0] == 0x80 && setup[1] == 0x06;
    let is_device_descriptor = setup[3] == 0x01;
    if is_get_descriptor && is_device_descriptor && data.len() >= 12 {
        let vid = u16::from_le_bytes([data[8], data[9]]);
        let pid = u16::from_le_bytes([data[10], data[11]]);
        descriptors.entry(key).or_insert(UsbId { vid, pid });
    }
}
