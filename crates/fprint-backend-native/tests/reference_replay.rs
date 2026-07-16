// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Frozen capture golden for the `reference` driver: replaying its recording through the
//! detect -> match pipeline must reproduce the committed facts.
//!
//! `cargo xtask capture-golden reference <recording>` writes this file and the fixtures
//! beside it. Regeneration is deliberate and rewrites the goldens under
//! `CAPTURE_GOLDEN_BLESS`; a plain `cargo test` replays the recording and asserts the
//! frozen facts, with no hardware.

use std::path::{Path, PathBuf};

use fprint_backend_native::{
    extract_minutiae, nbis_match_score, template_from_images, Capture, Frame, FrameSource,
    ScriptedTransport, Session, UsbFrameSource, UsbTransfer,
};
use fprint_testkit::block_on;

/// The VFS5011 image endpoint. The scripted transport ignores it; the recorded transfer names it.
const EP_IN: u8 = 0x81;

/// The committed fixtures for this driver.
fn fixtures() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/reference"
    ))
}

/// FNV-1a over the assembled frame's pixels: a stable fingerprint of the framed bytes.
fn frame_hash(data: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Replay the frozen wire bytes through the genuine VFS5011 framing.
fn assemble() -> Frame {
    let bytes = std::fs::read(fixtures().join("wire.bin")).expect("read wire.bin");
    let mut transfers = Vec::new();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        let len = u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize;
        i += 4;
        transfers.push(UsbTransfer::BulkIn {
            ep: EP_IN,
            data: bytes[i..i + len].to_vec(),
        });
        i += len;
    }
    let session = Session {
        device: None,
        transfers,
    };
    let mut source = UsbFrameSource::new(ScriptedTransport::from_session(&session));
    block_on(source.arm()).expect("arm the scripted source");
    match block_on(source.capture()).expect("capture from the scripted source") {
        Capture::Frame(frame) => frame,
        Capture::Retry(_) => panic!("a scripted capture never retries"),
    }
}

/// Read a committed golden fact.
fn read(dir: &Path, name: &str) -> String {
    std::fs::read_to_string(dir.join(name)).unwrap_or_else(|e| panic!("read {name}: {e}"))
}

#[test]
fn reference_replay_reproduces_the_frozen_facts() {
    let frame = assemble();
    let hash = frame_hash(&frame.data);
    let minutiae = extract_minutiae(frame.as_gray());
    let count = minutiae.len();
    let mut xyt = String::new();
    for m in &minutiae {
        xyt.push_str(&format!("{} {} {}\n", m.x, m.y, m.theta));
    }
    let template = template_from_images(&[frame.as_gray()]);
    let score = nbis_match_score(&template, &template);

    let dir = fixtures();
    if std::env::var_os("CAPTURE_GOLDEN_BLESS").is_some() {
        std::fs::write(dir.join("frame.hash"), format!("{hash}\n")).expect("write frame.hash");
        std::fs::write(dir.join("minutiae.xyt"), &xyt).expect("write minutiae.xyt");
        std::fs::write(dir.join("minutiae.count"), format!("{count}\n"))
            .expect("write minutiae.count");
        std::fs::write(dir.join("self.score"), format!("{score}\n")).expect("write self.score");
        return;
    }

    assert_eq!(
        read(&dir, "frame.hash"),
        format!("{hash}\n"),
        "assembled-frame hash drifted"
    );
    assert_eq!(
        read(&dir, "minutiae.count"),
        format!("{count}\n"),
        "minutiae count drifted"
    );
    assert_eq!(
        read(&dir, "self.score"),
        format!("{score}\n"),
        "self-verify score drifted"
    );
    assert_eq!(read(&dir, "minutiae.xyt"), xyt, "xyt dump drifted");
}
