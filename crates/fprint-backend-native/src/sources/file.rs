// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [`FileFrameSource`]: a hardware-free [`FrameSource`] that replays committed frames.
//!
//! This is the file-backed counterpart of libfprint's `virtual-image` driver: instead of
//! rendering a synthetic finger ([`crate::SyntheticFrameSource`]) it serves pre-captured raw
//! 8-bit grayscale frames one per [`capture`](FrameSource::capture). That lets a golden corpus of
//! real sensor bytes drive the full detect -> match pipeline offline, on any platform (Windows
//! included), with byte-stable, reproducible results.
//!
//! Frames are served in order and the sequence **cycles**: once the last frame is handed out the
//! next `capture` wraps to the first. A single frame therefore serves both a multi-stage
//! `enroll` (the same finger presented N times) and a one-shot `verify`, while a multi-frame
//! sequence models distinct presses. An empty source is rejected at construction, so `capture`
//! can never fail for want of a frame.
//!
//! The [PGM](https://netpbm.sourceforge.net/doc/pgm.html) (`P5`, binary) parser and writer are
//! split out as pure functions so the framing is unit-tested directly, independent of any device.

use fprint_core::{Error, Result};

use crate::frame::Frame;
use crate::frame_source::{Capture, FrameSource};

/// A capture source that replays a fixed, cyclic sequence of committed grayscale frames.
#[derive(Clone)]
pub struct FileFrameSource {
    /// The frames to replay, in order; never empty (enforced by every constructor).
    frames: Vec<Frame>,
    /// Index of the frame the next `capture` will return (wraps modulo `frames.len()`).
    next: usize,
}

impl FileFrameSource {
    /// Build a source from an explicit, non-empty sequence of frames.
    ///
    /// Returns [`Error::Protocol`] if `frames` is empty — a source with nothing to capture is a
    /// caller error, not a runtime capture failure.
    pub fn new(frames: Vec<Frame>) -> Result<Self> {
        if frames.is_empty() {
            return Err(Error::Protocol(
                "FileFrameSource requires at least one frame".to_string(),
            ));
        }
        Ok(FileFrameSource { frames, next: 0 })
    }

    /// A source that replays one frame forever — the natural fit for `enroll` (the same finger
    /// presented every stage) and for `verify` (a single scan).
    pub fn repeating(frame: Frame) -> Self {
        FileFrameSource {
            frames: vec![frame],
            next: 0,
        }
    }

    /// Build a single-frame source from a raw row-major 8-bit buffer and its geometry.
    ///
    /// Returns [`Error::Protocol`] if `data.len()` is not exactly `width * height`.
    pub fn from_raw(data: Vec<u8>, width: usize, height: usize, ppi: u16) -> Result<Self> {
        Ok(Self::repeating(frame_from_raw(data, width, height, ppi)?))
    }

    /// Build a single-frame source by parsing a binary PGM (`P5`) image, tagging it with `ppi`
    /// (PGM carries no resolution, so the caller supplies the scan resolution the detector needs).
    ///
    /// Returns [`Error::Protocol`] on any malformed header or truncated pixel payload.
    pub fn from_pgm(bytes: &[u8], ppi: u16) -> Result<Self> {
        Ok(Self::repeating(parse_pgm(bytes, ppi)?))
    }
}

impl FrameSource for FileFrameSource {
    async fn capture(&mut self) -> Result<Capture> {
        // One poll boundary per capture, matching `SyntheticFrameSource`: this is the drop-cancel
        // point the `ImageDevice::enroll` loop relies on.
        crate::yield_now::yield_now().await;

        // `frames` is non-empty (constructor invariant), so this index is always valid.
        let frame = self.frames[self.next].clone();
        self.next = (self.next + 1) % self.frames.len();
        Ok(Capture::Frame(frame))
    }
}

/// Validate a raw buffer's length against its geometry and wrap it as a [`Frame`] (pure).
fn frame_from_raw(data: Vec<u8>, width: usize, height: usize, ppi: u16) -> Result<Frame> {
    let expected = width
        .checked_mul(height)
        .ok_or_else(|| Error::Protocol(format!("frame geometry {width}x{height} overflows")))?;
    if data.len() != expected {
        return Err(Error::Protocol(format!(
            "raw frame is {} bytes, expected {width}x{height} = {expected}",
            data.len()
        )));
    }
    Ok(Frame {
        data,
        width,
        height,
        ppi,
    })
}

/// Parse a binary PGM (`P5`) image into a [`Frame`] (pure).
///
/// Accepts the canonical grammar `P5` whitespace `<width>` whitespace `<height>` whitespace
/// `<maxval>` single-whitespace `<binary pixels>`, with `#` comment lines permitted anywhere in
/// the header (skipped to end of line). Only 8-bit images (`maxval <= 255`) are supported, since
/// [`Frame`] is 8-bit; a larger `maxval` (16-bit big-endian samples) is rejected rather than
/// silently read wrong.
fn parse_pgm(bytes: &[u8], ppi: u16) -> Result<Frame> {
    let mut cur = 0usize;

    // The magic number must be exactly "P5".
    let magic = next_token(bytes, &mut cur)?;
    if magic != b"P5" {
        return Err(Error::Protocol(format!(
            "not a binary PGM: magic {magic:?}, expected \"P5\""
        )));
    }

    let width = parse_dim(&next_token(bytes, &mut cur)?, "width")?;
    let height = parse_dim(&next_token(bytes, &mut cur)?, "height")?;
    let maxval = parse_dim(&next_token(bytes, &mut cur)?, "maxval")?;
    if maxval == 0 || maxval > 255 {
        return Err(Error::Protocol(format!(
            "unsupported PGM maxval {maxval} (only 8-bit, 1..=255, is supported)"
        )));
    }

    // Exactly one whitespace byte separates the header from the pixel payload.
    if cur >= bytes.len() || !bytes[cur].is_ascii_whitespace() {
        return Err(Error::Protocol(
            "PGM header not terminated by whitespace before pixel data".to_string(),
        ));
    }
    cur += 1;

    let expected = width
        .checked_mul(height)
        .ok_or_else(|| Error::Protocol(format!("PGM geometry {width}x{height} overflows")))?;
    let payload = &bytes[cur..];
    if payload.len() != expected {
        return Err(Error::Protocol(format!(
            "PGM pixel payload is {} bytes, expected {width}x{height} = {expected}",
            payload.len()
        )));
    }

    Ok(Frame {
        data: payload.to_vec(),
        width,
        height,
        ppi,
    })
}

/// Serialize a [`Frame`] as a binary PGM (`P5`) image with `maxval` 255 (pure; inverse of
/// [`parse_pgm`] for 8-bit data).
#[cfg(test)]
fn write_pgm(frame: &Frame) -> Vec<u8> {
    let mut out = format!("P5\n{} {}\n255\n", frame.width, frame.height).into_bytes();
    out.extend_from_slice(&frame.data);
    out
}

/// Read the next whitespace-delimited token from `bytes` starting at `*cur`, skipping leading
/// whitespace and `#` comment lines, and advance `*cur` past the token.
fn next_token(bytes: &[u8], cur: &mut usize) -> Result<Vec<u8>> {
    loop {
        // Skip runs of whitespace.
        while *cur < bytes.len() && bytes[*cur].is_ascii_whitespace() {
            *cur += 1;
        }
        // A comment runs to end of line; skip it and re-scan for whitespace/comments.
        if *cur < bytes.len() && bytes[*cur] == b'#' {
            while *cur < bytes.len() && bytes[*cur] != b'\n' {
                *cur += 1;
            }
            continue;
        }
        break;
    }
    let start = *cur;
    while *cur < bytes.len() && !bytes[*cur].is_ascii_whitespace() {
        *cur += 1;
    }
    if *cur == start {
        return Err(Error::Protocol(
            "unexpected end of PGM header while reading a token".to_string(),
        ));
    }
    Ok(bytes[start..*cur].to_vec())
}

/// Parse an ASCII decimal header field into a `usize`.
fn parse_dim(token: &[u8], what: &str) -> Result<usize> {
    let s = core::str::from_utf8(token)
        .map_err(|_| Error::Protocol(format!("PGM {what} is not ASCII: {token:?}")))?;
    s.parse::<usize>()
        .map_err(|_| Error::Protocol(format!("PGM {what} is not a decimal integer: {s:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame_source::Capture;
    use fprint_testkit::block_on;

    fn tiny_frame() -> Frame {
        // A 4x3 gradient — distinct bytes so a round-trip that transposes or truncates is caught.
        Frame {
            data: (0u8..12).collect(),
            width: 4,
            height: 3,
            ppi: 500,
        }
    }

    #[test]
    fn from_raw_rejects_wrong_length() {
        assert!(FileFrameSource::from_raw(vec![0; 11], 4, 3, 500).is_err());
        assert!(FileFrameSource::from_raw(vec![0; 12], 4, 3, 500).is_ok());
    }

    #[test]
    fn new_rejects_empty() {
        assert!(FileFrameSource::new(Vec::new()).is_err());
    }

    #[test]
    fn pgm_round_trips_exactly() {
        let frame = tiny_frame();
        let pgm = write_pgm(&frame);
        let parsed = parse_pgm(&pgm, 500).expect("valid PGM parses");
        assert_eq!(parsed.width, frame.width);
        assert_eq!(parsed.height, frame.height);
        assert_eq!(parsed.ppi, 500);
        assert_eq!(parsed.data, frame.data);
    }

    #[test]
    fn pgm_accepts_comment_lines() {
        let mut pgm = b"P5\n# a comment\n4 3\n# another\n255\n".to_vec();
        pgm.extend_from_slice(&(0u8..12).collect::<Vec<u8>>());
        let parsed = parse_pgm(&pgm, 500).expect("commented PGM parses");
        assert_eq!((parsed.width, parsed.height), (4, 3));
        assert_eq!(parsed.data, (0u8..12).collect::<Vec<u8>>());
    }

    #[test]
    fn pgm_rejects_bad_magic() {
        let mut pgm = b"P2\n4 3\n255\n".to_vec();
        pgm.extend_from_slice(&[0u8; 12]);
        assert!(parse_pgm(&pgm, 500).is_err());
    }

    #[test]
    fn pgm_rejects_wrong_payload_length() {
        let mut pgm = b"P5\n4 3\n255\n".to_vec();
        pgm.extend_from_slice(&[0u8; 11]); // one short
        assert!(parse_pgm(&pgm, 500).is_err());
    }

    #[test]
    fn pgm_rejects_16bit_maxval() {
        let mut pgm = b"P5\n4 3\n65535\n".to_vec();
        pgm.extend_from_slice(&[0u8; 24]);
        assert!(parse_pgm(&pgm, 500).is_err());
    }

    #[test]
    fn capture_cycles_through_frames() {
        let a = Frame {
            data: vec![1; 12],
            width: 4,
            height: 3,
            ppi: 500,
        };
        let b = Frame {
            data: vec![2; 12],
            width: 4,
            height: 3,
            ppi: 500,
        };
        let mut src = FileFrameSource::new(vec![a, b]).unwrap();

        let grab = |src: &mut FileFrameSource| -> u8 {
            match block_on(src.capture()).unwrap() {
                Capture::Frame(f) => f.data[0],
                Capture::Retry(_) => unreachable!("FileFrameSource never retries"),
            }
        };
        assert_eq!(grab(&mut src), 1);
        assert_eq!(grab(&mut src), 2);
        assert_eq!(grab(&mut src), 1, "sequence cycles back to the first frame");
    }
}
