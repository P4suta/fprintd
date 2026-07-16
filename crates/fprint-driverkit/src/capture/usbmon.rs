// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Linux `usbmon` decoders: the binary `DLT_USB_LINUX` / `DLT_USB_LINUX_MMAPPED` pseudo-header, and
//! the `/sys/kernel/debug/usb/usbmon/<n>u` ASCII text format.
//!
//! Both are an interoperability fact — a fixed field layout the kernel emits — reproduced here from
//! the layout documentation, not from any capture-tool source. The field offsets are spelled out in
//! `docs/re-capture-formats.md`.

use crate::capture::event::{DeviceKey, Endian, Stage, TransferType, UsbEvent};

/// Length of the `DLT_USB_LINUX` pseudo-header: `id` through the 8-byte `setup` field.
const HEADER_LEN_LEGACY: usize = 48;
/// Length of the `DLT_USB_LINUX_MMAPPED` pseudo-header: the legacy fields plus `interval`,
/// `start_frame`, `xfer_flags`, and `ndesc`.
const HEADER_LEN_MMAPPED: usize = 64;

/// A submit event's `type` byte (`'S'`).
const TYPE_SUBMIT: u8 = b'S';
/// A completion event's `type` byte (`'C'`).
const TYPE_COMPLETE: u8 = b'C';
/// A `flag_setup` of `0` marks the 8-byte setup field as present (a control submission).
const SETUP_PRESENT: u8 = 0;

/// Decode one usbmon binary packet — the pseudo-header followed by its captured data — into a
/// [`UsbEvent`]. `mmapped` selects the 64-byte `DLT_USB_LINUX_MMAPPED` header over the 48-byte
/// `DLT_USB_LINUX` one; the fields this reads share offsets across both.
///
/// Returns `None` for a packet that is neither a submit nor a completion, or whose transfer type is
/// unknown, or that is too short to hold its header.
pub(crate) fn decode_binary(packet: &[u8], endian: Endian, mmapped: bool) -> Option<UsbEvent> {
    let header_len = if mmapped {
        HEADER_LEN_MMAPPED
    } else {
        HEADER_LEN_LEGACY
    };
    if packet.len() < header_len {
        return None;
    }

    let id = {
        let mut b = [0u8; 8];
        b.copy_from_slice(&packet[0..8]);
        match endian {
            Endian::Little => u64::from_le_bytes(b),
            Endian::Big => u64::from_be_bytes(b),
        }
    };
    let type_ = packet[8];
    let xfer_type = packet[9];
    let endpoint = packet[10];
    let address = u16::from(packet[11]);
    let busnum = endian.u16([packet[12], packet[13]]);
    let flag_setup = packet[14];
    let len_cap = endian.u32([packet[36], packet[37], packet[38], packet[39]]) as usize;

    let stage = match type_ {
        TYPE_SUBMIT => Stage::Submit,
        TYPE_COMPLETE => Stage::Complete,
        _ => return None,
    };
    let transfer_type = TransferType::from_code(xfer_type)?;

    let setup = if transfer_type == TransferType::Control && flag_setup == SETUP_PRESENT {
        let mut s = [0u8; 8];
        s.copy_from_slice(&packet[40..48]);
        Some(s)
    } else {
        None
    };

    let data_end = (header_len + len_cap).min(packet.len());
    let data = packet[header_len..data_end].to_vec();

    Some(UsbEvent {
        id,
        key: DeviceKey {
            bus: busnum,
            address,
        },
        endpoint,
        transfer_type,
        stage,
        setup,
        data,
    })
}

/// Parse the usbmon ASCII text format into events, one per non-blank line.
///
/// A line is: URB tag, timestamp, event type (`S`/`C`/`E`), the `TDb:bus:dev:ep` address word, then
/// either a control setup (`s` and five hex fields) or a status word, a data length, and an
/// optional `= <hex words>` data block. Lines that do not parse — including `E` error events — are
/// skipped so a stray line cannot fail an otherwise good capture.
pub(crate) fn parse_text(text: &str) -> Vec<UsbEvent> {
    text.lines().filter_map(parse_text_line).collect()
}

/// Parse one usbmon text line into a [`UsbEvent`], or `None` if it is blank, an error event, or
/// malformed.
fn parse_text_line(line: &str) -> Option<UsbEvent> {
    let mut tok = line.split_whitespace();
    let id = u64::from_str_radix(tok.next()?, 16).ok()?;
    let _timestamp = tok.next()?;
    let stage = match tok.next()? {
        "S" => Stage::Submit,
        "C" => Stage::Complete,
        _ => return None,
    };

    let (transfer_type, endpoint, key) = parse_address_word(tok.next()?)?;

    // The setup-or-status field: `s` opens a control setup packet, anything else is the status word
    // that precedes the data length.
    let mut peek = tok.next()?;
    let setup = if peek == "s" {
        let s = parse_setup(&mut tok)?;
        peek = tok.next()?;
        Some(s)
    } else {
        None
    };

    // `peek` now holds the status word (non-control) or the data length (control, consumed above as
    // the token after the setup). Re-align: for a control line the length is `peek`; otherwise the
    // length is the next token after the status word in `peek`.
    let length_tok = if setup.is_some() { peek } else { tok.next()? };
    let length: usize = length_tok.parse().ok()?;

    let data = match tok.next() {
        Some("=") => parse_hex_words(tok, length),
        _ => Vec::new(),
    };

    Some(UsbEvent {
        id,
        key,
        endpoint,
        transfer_type,
        stage,
        setup,
        data,
    })
}

/// Decode the `TDb:bus:dev:ep` address word: a type letter (`C`/`Z`/`I`/`B`), a direction letter
/// (`i`/`o`), then the bus, device address, and endpoint number. Returns the transfer type, the
/// endpoint address with its IN bit, and the device key.
fn parse_address_word(word: &str) -> Option<(TransferType, u8, DeviceKey)> {
    let mut chars = word.chars();
    let type_letter = chars.next()?;
    let dir_letter = chars.next()?;
    let transfer_type = match type_letter {
        'C' => TransferType::Control,
        'Z' => TransferType::Isochronous,
        'I' => TransferType::Interrupt,
        'B' => TransferType::Bulk,
        _ => return None,
    };
    let dir_in = match dir_letter {
        'i' => true,
        'o' => false,
        _ => return None,
    };

    let rest = chars.as_str();
    let mut parts = rest.split(':');
    // The character after the direction letter is the ':' before the bus number.
    if !parts.next()?.is_empty() {
        return None;
    }
    let bus: u16 = parts.next()?.parse().ok()?;
    let address: u16 = parts.next()?.parse().ok()?;
    let ep_num: u8 = parts.next()?.parse().ok()?;

    let endpoint = if dir_in { ep_num | 0x80 } else { ep_num };
    Some((transfer_type, endpoint, DeviceKey { bus, address }))
}

/// Read the five hex fields of a control setup packet — `bmRequestType bRequest wValue wIndex
/// wLength` — into the 8 wire bytes. usbmon prints `wValue`/`wIndex`/`wLength` as logical 16-bit
/// values, so they are stored little-endian to match the on-wire setup packet.
fn parse_setup<'a>(tok: &mut impl Iterator<Item = &'a str>) -> Option<[u8; 8]> {
    let request_type = u8::from_str_radix(tok.next()?, 16).ok()?;
    let request = u8::from_str_radix(tok.next()?, 16).ok()?;
    let value = u16::from_str_radix(tok.next()?, 16).ok()?;
    let index = u16::from_str_radix(tok.next()?, 16).ok()?;
    let length = u16::from_str_radix(tok.next()?, 16).ok()?;

    let mut setup = [0u8; 8];
    setup[0] = request_type;
    setup[1] = request;
    setup[2..4].copy_from_slice(&value.to_le_bytes());
    setup[4..6].copy_from_slice(&index.to_le_bytes());
    setup[6..8].copy_from_slice(&length.to_le_bytes());
    Some(setup)
}

/// Join the remaining space-separated hex words into a byte vector, capped at `length` bytes. Each
/// word is up to 4 bytes printed in memory order, so the concatenated hex digits are the byte
/// sequence directly.
fn parse_hex_words<'a>(tok: impl Iterator<Item = &'a str>, length: usize) -> Vec<u8> {
    let hex: String = tok.collect();
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    let raw = hex.as_bytes();
    let mut i = 0;
    while i + 1 < raw.len() {
        let hi = (raw[i] as char).to_digit(16);
        let lo = (raw[i + 1] as char).to_digit(16);
        match (hi, lo) {
            (Some(hi), Some(lo)) => bytes.push((hi * 16 + lo) as u8),
            _ => break,
        }
        i += 2;
    }
    bytes.truncate(length);
    bytes
}
