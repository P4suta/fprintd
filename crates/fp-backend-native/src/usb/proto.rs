// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Pure USB framing for the host-image driver: `Vec<u8>` in, `Vec<u8>` / [`Frame`] out.
//!
//! This module has **no transport and no `nusb`**: every function is a pure transformation of byte
//! slices, so the whole framing layer is unit-tested on any platform. The transport ([`super::
//! transport`]) moves the bytes; this module decides what they mean.
//!
//! ## Provenance
//!
//! The wire shapes here are **original code describing interoperability facts** (command opcodes,
//! header layout, image geometry) — not a transliteration of libfprint's LGPL `vfs5011.c`. Where a
//! real byte value has not been confirmed against a physical Validity sensor it is a clearly
//! documented placeholder ("HW-verified: required"), so nothing here is asserted as fact that has
//! not been observed. See [`super::vfs5011`] for the device-level constants and the same note.

use fp_core::{Error, Result};

use crate::frame::Frame;

/// Two-byte marker prefixing a captured-frame descriptor on the bulk-in stream.
///
/// HW-verified: required. This is a placeholder framing chosen so the header is self-describing and
/// round-trippable in tests; the real VFS5011 image stream delimiting must be confirmed on hardware.
const FRAME_MAGIC: [u8; 2] = [0x01, 0xFE];

/// Fixed size of the frame header this module emits/parses: `MAGIC(2) | width(2 LE) | height(2 LE)`.
pub const FRAME_HEADER_LEN: usize = 6;

/// Opcode of the "begin image capture" bulk-out command.
///
/// HW-verified: required. Placeholder opcode; the real capture request byte(s) for the VFS5011 come
/// from the observed control/bulk exchange and must be confirmed on hardware.
const CAPTURE_OPCODE: u8 = 0x50;

/// Encode the "begin image capture" command sent on the bulk-out endpoint.
///
/// Kept as a function (not a `const`) because a real driver's capture request typically carries a
/// sequence counter or mode flags; today it is a single documented opcode byte.
#[must_use]
pub fn encode_capture_cmd() -> Vec<u8> {
    vec![CAPTURE_OPCODE]
}

/// Encode a frame header for a `width`×`height` image (inverse of [`parse_frame_header`]).
///
/// Production never encodes a header — the *device* emits it and the host only parses it — so this
/// mirror exists solely to script a stream this module then parses back (round-trip tests and the
/// mock transport).
#[cfg(test)]
#[must_use]
pub(crate) fn encode_frame_header(width: u16, height: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(FRAME_HEADER_LEN);
    out.extend_from_slice(&FRAME_MAGIC);
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out
}

/// Parse a frame header, returning the image `(width, height)` in pixels.
///
/// Errors with [`Error::Protocol`] on a short buffer or a bad magic marker.
pub fn parse_frame_header(bytes: &[u8]) -> Result<(usize, usize)> {
    if bytes.len() < FRAME_HEADER_LEN {
        return Err(Error::Protocol(format!(
            "frame header is {} bytes, need at least {FRAME_HEADER_LEN}",
            bytes.len()
        )));
    }
    if bytes[0..2] != FRAME_MAGIC {
        return Err(Error::Protocol(format!(
            "bad frame header magic {:?}, expected {FRAME_MAGIC:?}",
            &bytes[0..2]
        )));
    }
    let width = u16::from_le_bytes([bytes[2], bytes[3]]) as usize;
    let height = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    Ok((width, height))
}

/// Assemble a captured [`Frame`] from its transferred payload chunks.
///
/// The chunks are concatenated in order (a real sensor streams the image in several bulk-in
/// transfers) and the total must be exactly `width * height` bytes. Errors with [`Error::Protocol`]
/// on a geometry overflow or a length mismatch, so a truncated or overrun transfer is rejected
/// rather than silently producing a mis-shaped frame.
pub fn assemble_frame(chunks: &[&[u8]], width: usize, height: usize, ppi: u16) -> Result<Frame> {
    let expected = width
        .checked_mul(height)
        .ok_or_else(|| Error::Protocol(format!("frame geometry {width}x{height} overflows")))?;
    let total: usize = chunks.iter().map(|c| c.len()).sum();
    if total != expected {
        return Err(Error::Protocol(format!(
            "assembled {total} pixel bytes, expected {width}x{height} = {expected}"
        )));
    }
    let mut data = Vec::with_capacity(expected);
    for c in chunks {
        data.extend_from_slice(c);
    }
    Ok(Frame {
        data,
        width,
        height,
        ppi,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_header_round_trips() {
        let bytes = encode_frame_header(200, 144);
        assert_eq!(bytes.len(), FRAME_HEADER_LEN);
        assert_eq!(parse_frame_header(&bytes).unwrap(), (200, 144));
    }

    #[test]
    fn parse_frame_header_rejects_short_buffer() {
        assert!(parse_frame_header(&[0x01, 0xFE, 0x00]).is_err());
    }

    #[test]
    fn parse_frame_header_rejects_bad_magic() {
        let mut bytes = encode_frame_header(8, 8);
        bytes[0] = 0x00;
        assert!(parse_frame_header(&bytes).is_err());
    }

    #[test]
    fn capture_cmd_is_stable() {
        assert_eq!(encode_capture_cmd(), vec![CAPTURE_OPCODE]);
    }

    #[test]
    fn assemble_frame_joins_chunks_in_order() {
        let a = [1u8, 2, 3, 4];
        let b = [5u8, 6, 7, 8];
        let frame = assemble_frame(&[&a, &b], 4, 2, 500).unwrap();
        assert_eq!(frame.width, 4);
        assert_eq!(frame.height, 2);
        assert_eq!(frame.ppi, 500);
        assert_eq!(frame.data, vec![1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn assemble_frame_rejects_length_mismatch() {
        let a = [1u8, 2, 3];
        assert!(assemble_frame(&[&a], 4, 2, 500).is_err()); // 3 bytes for a 4x2 = 8 image
    }

    #[test]
    fn assemble_frame_accepts_single_chunk() {
        let payload: Vec<u8> = (0..12).collect();
        let frame = assemble_frame(&[&payload], 4, 3, 500).unwrap();
        assert_eq!(frame.data, payload);
    }
}
