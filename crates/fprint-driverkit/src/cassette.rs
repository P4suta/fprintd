// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The `.cassette` on-disk recording: one [`Session`] of USB traffic, saved as pretty JSON with hex
//! byte payloads.
//!
//! A cassette is the portable unit the whole toolkit passes around: `fpdev import` writes one from a
//! `.pcapng` capture, `fpdev record` writes one from live hardware, and `fpdev replay` / `fpdev
//! frame` read one back. The format is [`fprint_backend_native::Session`] under `serde_json` — the
//! backend's `serde` feature carries the derives, and payload bytes serialize as a lowercase hex
//! string, so a recording diffs and reviews as text.

use std::path::Path;

use fprint_backend_native::Session;

/// A failure while reading or writing a `.cassette`.
#[derive(Debug)]
pub enum CassetteError {
    /// The file could not be read or written.
    Io(std::io::Error),
    /// The bytes were not a valid cassette (malformed JSON, or a bad hex payload).
    Format(serde_json::Error),
}

impl std::fmt::Display for CassetteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "cassette i/o: {e}"),
            Self::Format(e) => write!(f, "cassette format: {e}"),
        }
    }
}

impl std::error::Error for CassetteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Format(e) => Some(e),
        }
    }
}

impl From<std::io::Error> for CassetteError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for CassetteError {
    fn from(e: serde_json::Error) -> Self {
        Self::Format(e)
    }
}

/// Serialize `session` to `path` as pretty JSON with a trailing newline.
///
/// # Errors
/// Returns [`CassetteError`] if the session cannot be serialized or the file cannot be written.
pub fn save(session: &Session, path: impl AsRef<Path>) -> Result<(), CassetteError> {
    let mut json = serde_json::to_string_pretty(session)?;
    json.push('\n');
    std::fs::write(path, json)?;
    Ok(())
}

/// Read a [`Session`] back from a `.cassette` at `path`.
///
/// # Errors
/// Returns [`CassetteError`] if the file cannot be read or its contents are not a valid cassette.
pub fn load(path: impl AsRef<Path>) -> Result<Session, CassetteError> {
    let text = std::fs::read_to_string(path)?;
    let session = serde_json::from_str(&text)?;
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fprint_backend_native::{UsbId, UsbTransfer};

    /// A small session: a control init, a bulk-out capture command, then a frame header and its
    /// pixel payload as two bulk-in reads.
    fn sample() -> Session {
        let mut session = Session::for_device(UsbId {
            vid: 0x138a,
            pid: 0x0011,
        });
        session
            .push(UsbTransfer::Control {
                request_type: 0x40,
                request: 0x01,
                value: 0x0000,
                index: 0x0000,
                data: vec![0x02, 0xff],
            })
            .push(UsbTransfer::BulkOut {
                ep: 0x02,
                data: vec![0x50],
            })
            .push(UsbTransfer::BulkIn {
                ep: 0x81,
                data: vec![0x01, 0xfe, 0x04, 0x00, 0x02, 0x00],
            })
            .push(UsbTransfer::BulkIn {
                ep: 0x81,
                data: vec![0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80],
            });
        session
    }

    #[test]
    fn save_then_load_round_trips() {
        let session = sample();
        let dir = std::env::temp_dir();
        let path = dir.join(format!("fpdev-roundtrip-{}.cassette", std::process::id()));
        save(&session, &path).unwrap();
        let back = load(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(session, back);
    }

    #[test]
    fn committed_fixture_parses_to_the_expected_session() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/sample.cassette"
        );
        let loaded = load(path).unwrap();
        assert_eq!(loaded, sample());
    }

    #[test]
    fn payloads_are_written_as_hex_strings() {
        let json = serde_json::to_string(&sample()).unwrap();
        assert!(json.contains("\"01fe04000200\""));
        assert!(json.contains("\"1020304050607080\""));
    }
}
