// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Freezing a driver's captured recording as a golden for the pipeline.
//!
//! A bring-up records a live USB session (or imports a trace) into a `.cassette`, or captures a raw
//! frame as a `.pgm`. Once that recording decodes to a matching frame it is the driver's ground
//! truth. This task lifts the recording into `fprint-backend-native`'s fixtures in a serde-free
//! form, generates an ordinary `#[test]` that replays it through the detect -> match pipeline, and
//! freezes the derived facts (the assembled-frame hash, the xyt dump, the minutiae count, and the
//! self-verify score) as goldens beside it.
//!
//! The split follows the NBIS oracles and the fuzz survivors. Regeneration is **deliberate**: this
//! task drives the generated test under `CAPTURE_GOLDEN_BLESS` to overwrite the goldens, then runs
//! it plain to confirm they hold. Plain CI runs only the frozen test, which needs no hardware, no
//! nightly toolchain and no container: it reads the committed recording, replays it, and asserts
//! the committed facts.
//!
//! Two recording shapes, matching the two capture seams:
//!
//! * A `.cassette` replays through `ScriptedTransport`: its device-to-host bytes are frozen as
//!   `wire.bin` and the generated test drives the genuine VFS5011 framing over them.
//! * A `.pgm` replays through `FileFrameSource`: the image is frozen as `frame.pgm` and the
//!   generated test parses it back.

use std::path::Path;
use std::process::Command;

/// Which capture seam a recording drives, chosen by its extension.
#[derive(Clone, Copy)]
enum Recording {
    /// A `.cassette` of recorded USB traffic, replayed through the scripted transport.
    Cassette,
    /// A binary (`P5`) PGM image, replayed through the file source.
    Pgm,
}

/// `capture-golden <driver> <recording-path>`: freeze `recording-path` as the driver's golden.
///
/// # Errors
/// Returns an error if the driver name is unsafe, the recording is an unknown shape or cannot be
/// decoded, a file cannot be written, or the generated test does not reproduce the frozen facts.
pub fn run(root: &Path, driver: &str, recording: &Path) -> Result<(), String> {
    check_driver_name(driver)?;
    let kind = recording_kind(recording)?;

    let backend = root.join("crates/fprint-backend-native");
    let fixtures = backend.join("tests/fixtures").join(driver);
    std::fs::create_dir_all(&fixtures)
        .map_err(|e| format!("create {}: {e}", fixtures.display()))?;

    freeze_recording(recording, kind, &fixtures)?;
    write_reuse(&backend.join("tests/fixtures"))?;

    let test_path = backend.join("tests").join(format!("{driver}_replay.rs"));
    std::fs::write(&test_path, generated_test(driver, kind))
        .map_err(|e| format!("write {}: {e}", test_path.display()))?;
    rustfmt(&test_path)?;

    println!("xtask: regenerating the {driver} goldens (deliberate)");
    run_generated_test(root, driver, true)?;
    println!("xtask: replaying the recording against the frozen facts");
    run_generated_test(root, driver, false)?;

    println!(
        "xtask: froze {} and generated {}",
        fixtures.display(),
        test_path.display()
    );
    Ok(())
}

/// A driver name must be a plain lowercase snake token: it becomes a fixtures directory and a test
/// file stem, so anything else is rejected before it reaches the filesystem.
fn check_driver_name(driver: &str) -> Result<(), String> {
    if driver.is_empty() {
        return Err("driver name is empty".to_string());
    }
    if !driver
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
        return Err(format!(
            "driver name `{driver}` is not a lowercase snake token ([a-z0-9_])"
        ));
    }
    Ok(())
}

/// Classify a recording by its file extension.
fn recording_kind(recording: &Path) -> Result<Recording, String> {
    match recording.extension().and_then(|e| e.to_str()) {
        Some("cassette") => Ok(Recording::Cassette),
        Some("pgm") => Ok(Recording::Pgm),
        _ => Err(format!(
            "{}: unknown recording (expected a `.cassette` or a `.pgm`)",
            recording.display()
        )),
    }
}

/// Lift the recording into the fixtures directory in the serde-free form the generated test replays.
fn freeze_recording(recording: &Path, kind: Recording, fixtures: &Path) -> Result<(), String> {
    match kind {
        Recording::Cassette => {
            let json = std::fs::read_to_string(recording)
                .map_err(|e| format!("read {}: {e}", recording.display()))?;
            let payloads = cassette_bulk_in_payloads(&json)?;
            if payloads.len() < 2 {
                return Err(format!(
                    "{}: fewer than two bulk-in transfers, so no complete frame to freeze",
                    recording.display()
                ));
            }
            let wire = fixtures.join("wire.bin");
            std::fs::write(&wire, framed(&payloads[..2]))
                .map_err(|e| format!("write {}: {e}", wire.display()))
        }
        Recording::Pgm => {
            let bytes = std::fs::read(recording)
                .map_err(|e| format!("read {}: {e}", recording.display()))?;
            if !bytes.starts_with(b"P5") {
                return Err(format!(
                    "{}: not a binary PGM (magic is not `P5`)",
                    recording.display()
                ));
            }
            let pgm = fixtures.join("frame.pgm");
            std::fs::write(&pgm, bytes).map_err(|e| format!("write {}: {e}", pgm.display()))
        }
    }
}

/// Extract the device-to-host payloads from a `.cassette`, in wire order.
///
/// A cassette is `serde_json::to_string_pretty` over a `Session`, so its layout is stable: one
/// transfer per array element, a variant key line (`"BulkIn": {`) followed by the transfer's single
/// `"data": "<hex>"` line. Only `BulkIn` payloads are the bytes the device returned; the
/// host-to-device writes replay from the driver itself.
///
/// The scan is coupled to that pretty layout deliberately: xtask holds no serde stack, and the
/// format is this repository's own, written only by `cassette::save`.
fn cassette_bulk_in_payloads(json: &str) -> Result<Vec<Vec<u8>>, String> {
    let mut payloads = Vec::new();
    let mut in_bulk_in = false;
    for line in json.lines() {
        let trimmed = line.trim();
        if let Some(key) = variant_key(trimmed) {
            in_bulk_in = key == "BulkIn";
        } else if in_bulk_in && trimmed.starts_with("\"data\":") {
            let hex = trimmed
                .split('"')
                .nth(3)
                .ok_or_else(|| format!("malformed data line: {trimmed}"))?;
            payloads.push(decode_hex(hex)?);
        }
    }
    Ok(payloads)
}

/// The transfer-variant a `"BulkIn": {` / `"BulkOut": {` / `"Control": {` line opens, if any.
fn variant_key(trimmed: &str) -> Option<&str> {
    if trimmed.starts_with('"') && trimmed.ends_with('{') {
        return trimmed
            .split('"')
            .nth(1)
            .filter(|k| matches!(*k, "BulkIn" | "BulkOut" | "Control"));
    }
    None
}

/// Decode an even-length hex string into bytes.
fn decode_hex(hex: &str) -> Result<Vec<u8>, String> {
    if hex.len() % 2 != 0 {
        return Err(format!(
            "hex payload has an odd digit count ({})",
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

/// Frame each payload with a little-endian `u32` length prefix, so the generated test splits the
/// concatenation back into the exact transfers without a serde stack.
fn framed(payloads: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for p in payloads {
        let len = u32::try_from(p.len()).expect("a bulk-in payload fits u32");
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(p);
    }
    out
}

/// Write the fixtures directory's REUSE annotation, so the golden blobs (which carry no inline SPDX
/// header) pass `reuse lint`.
fn write_reuse(fixtures_root: &Path) -> Result<(), String> {
    std::fs::create_dir_all(fixtures_root)
        .map_err(|e| format!("create {}: {e}", fixtures_root.display()))?;
    let path = fixtures_root.join("REUSE.toml");
    std::fs::write(&path, REUSE_TOML).map_err(|e| format!("write {}: {e}", path.display()))
}

/// REUSE annotation for the capture-golden fixture blobs.
// REUSE-IgnoreStart
const REUSE_TOML: &str = "\
# SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# The capture-golden fixtures are the frozen recording and the facts derived from it, written by
# `cargo xtask capture-golden`. They carry no inline SPDX header and encode only interoperability
# facts (a device's framed pixels and the minutiae they yield), so they are ours under the crate's
# own license.

version = 1

[[annotations]]
path = [
  \"*/wire.bin\",
  \"*/frame.pgm\",
  \"*/frame.hash\",
  \"*/minutiae.xyt\",
  \"*/minutiae.count\",
  \"*/self.score\",
]
precedence = \"aggregate\"
SPDX-FileCopyrightText = \"2026 fprintd (pure-Rust) contributors\"
SPDX-License-Identifier = \"MIT OR Apache-2.0\"
";
// REUSE-IgnoreEnd

/// Emit the generated replay test for `driver`, tailored to the recording seam.
fn generated_test(driver: &str, kind: Recording) -> String {
    let (imports, consts, assemble) = match kind {
        Recording::Cassette => (
            "Capture, Frame, FrameSource, ScriptedTransport, Session, UsbFrameSource, UsbTransfer,",
            "/// The VFS5011 image endpoint. The scripted transport ignores it; the recorded \
             transfer names it.\nconst EP_IN: u8 = 0x81;\n\n",
            CASSETTE_ASSEMBLE,
        ),
        Recording::Pgm => (
            "Capture, FileFrameSource, Frame, FrameSource,",
            "/// The scan resolution the file source stamps on the replayed frame (the NBIS \
             reference). The detector's thresholds are resolution-relative.\nconst PPI: u16 = \
             500;\n\n",
            PGM_ASSEMBLE,
        ),
    };

    // REUSE-IgnoreStart
    format!(
        "// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors\n\
         //\n\
         // SPDX-License-Identifier: MIT OR Apache-2.0\n\
         \n\
         //! Frozen capture golden for the `{driver}` driver: replaying its recording through the\n\
         //! detect -> match pipeline must reproduce the committed facts.\n\
         //!\n\
         //! `cargo xtask capture-golden {driver} <recording>` writes this file and the fixtures\n\
         //! beside it. Regeneration is deliberate and rewrites the goldens under\n\
         //! `CAPTURE_GOLDEN_BLESS`; a plain `cargo test` replays the recording and asserts the\n\
         //! frozen facts, with no hardware.\n\
         \n\
         use std::path::{{Path, PathBuf}};\n\
         \n\
         use fprint_backend_native::{{{imports}}};\n\
         use fprint_pipeline::{{extract_minutiae, nbis_match_score, template_from_images}};\n\
         use fprint_testkit::block_on;\n\
         \n\
         {consts}\
         /// The committed fixtures for this driver.\n\
         fn fixtures() -> PathBuf {{\n\
         PathBuf::from(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/tests/fixtures/{driver}\"))\n\
         }}\n\
         \n\
         /// FNV-1a over the assembled frame's pixels: a stable fingerprint of the framed bytes.\n\
         fn frame_hash(data: &[u8]) -> String {{\n\
         let mut h: u64 = 0xcbf2_9ce4_8422_2325;\n\
         for &b in data {{\n\
         h ^= u64::from(b);\n\
         h = h.wrapping_mul(0x0000_0100_0000_01b3);\n\
         }}\n\
         format!(\"{{h:016x}}\")\n\
         }}\n\
         \n\
         {assemble}\
         \n\
         /// Read a committed golden fact.\n\
         fn read(dir: &Path, name: &str) -> String {{\n\
         std::fs::read_to_string(dir.join(name)).unwrap_or_else(|e| panic!(\"read {{name}}: {{e}}\"))\n\
         }}\n\
         \n\
         #[test]\n\
         fn {driver}_replay_reproduces_the_frozen_facts() {{\n\
         let frame = assemble();\n\
         let hash = frame_hash(&frame.data);\n\
         let minutiae = extract_minutiae(frame.as_gray());\n\
         let count = minutiae.len();\n\
         let mut xyt = String::new();\n\
         for m in &minutiae {{\n\
         xyt.push_str(&format!(\"{{}} {{}} {{}}\\n\", m.x, m.y, m.theta));\n\
         }}\n\
         let template = template_from_images(&[frame.as_gray()]);\n\
         let score = nbis_match_score(&template, &template);\n\
         \n\
         let dir = fixtures();\n\
         if std::env::var_os(\"CAPTURE_GOLDEN_BLESS\").is_some() {{\n\
         std::fs::write(dir.join(\"frame.hash\"), format!(\"{{hash}}\\n\")).expect(\"write frame.hash\");\n\
         std::fs::write(dir.join(\"minutiae.xyt\"), &xyt).expect(\"write minutiae.xyt\");\n\
         std::fs::write(dir.join(\"minutiae.count\"), format!(\"{{count}}\\n\"))\n\
         .expect(\"write minutiae.count\");\n\
         std::fs::write(dir.join(\"self.score\"), format!(\"{{score}}\\n\")).expect(\"write self.score\");\n\
         return;\n\
         }}\n\
         \n\
         assert_eq!(read(&dir, \"frame.hash\"), format!(\"{{hash}}\\n\"), \"assembled-frame hash drifted\");\n\
         assert_eq!(read(&dir, \"minutiae.count\"), format!(\"{{count}}\\n\"), \"minutiae count drifted\");\n\
         assert_eq!(read(&dir, \"self.score\"), format!(\"{{score}}\\n\"), \"self-verify score drifted\");\n\
         assert_eq!(read(&dir, \"minutiae.xyt\"), xyt, \"xyt dump drifted\");\n\
         }}\n"
    )
    // REUSE-IgnoreEnd
}

/// The cassette seam's `assemble`: rebuild the recorded transfers and drive the VFS5011 source.
const CASSETTE_ASSEMBLE: &str = "\
/// Replay the frozen wire bytes through the genuine VFS5011 framing.\n\
fn assemble() -> Frame {\n\
let bytes = std::fs::read(fixtures().join(\"wire.bin\")).expect(\"read wire.bin\");\n\
let mut transfers = Vec::new();\n\
let mut i = 0;\n\
while i + 4 <= bytes.len() {\n\
let len = u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize;\n\
i += 4;\n\
transfers.push(UsbTransfer::BulkIn {\n\
ep: EP_IN,\n\
data: bytes[i..i + len].to_vec(),\n\
});\n\
i += len;\n\
}\n\
let session = Session {\n\
device: None,\n\
transfers,\n\
};\n\
let mut source = UsbFrameSource::new(ScriptedTransport::from_session(&session));\n\
block_on(source.arm()).expect(\"arm the scripted source\");\n\
match block_on(source.capture()).expect(\"capture from the scripted source\") {\n\
Capture::Frame(frame) => frame,\n\
Capture::Retry(_) => panic!(\"a scripted capture never retries\"),\n\
}\n\
}\n";

/// The PGM seam's `assemble`: parse the committed image and drive the file source.
const PGM_ASSEMBLE: &str = "\
/// Replay the committed PGM through the file source.\n\
fn assemble() -> Frame {\n\
let pgm = std::fs::read(fixtures().join(\"frame.pgm\")).expect(\"read frame.pgm\");\n\
let mut source = FileFrameSource::from_pgm(&pgm, PPI).expect(\"parse the committed PGM\");\n\
match block_on(source.capture()).expect(\"capture from the file source\") {\n\
Capture::Frame(frame) => frame,\n\
Capture::Retry(_) => panic!(\"a file capture never retries\"),\n\
}\n\
}\n";

/// Format the generated test in place, so a re-run of this task leaves it byte-identical.
fn rustfmt(path: &Path) -> Result<(), String> {
    let status = Command::new("rustfmt")
        .arg("--edition")
        .arg("2021")
        .arg(path)
        .status()
        .map_err(|e| format!("spawn rustfmt: {e} (is rustfmt installed?)"))?;
    if !status.success() {
        return Err(format!("rustfmt {} failed ({status})", path.display()));
    }
    Ok(())
}

/// Drive the generated test for `driver`. With `bless`, it rewrites the goldens; without, it asserts
/// them. The regeneration is deliberate, so a plain CI run never sets the env.
fn run_generated_test(root: &Path, driver: &str, bless: bool) -> Result<(), String> {
    let target = format!("{driver}_replay");
    let mut cmd = Command::new("cargo");
    cmd.args(["test", "-p", "fprint-backend-native", "--test", &target])
        .current_dir(root);
    if bless {
        cmd.env("CAPTURE_GOLDEN_BLESS", "1");
    }
    let status = cmd.status().map_err(|e| format!("spawn cargo test: {e}"))?;
    if !status.success() {
        return Err(format!("cargo test --test {target} failed ({status})"));
    }
    Ok(())
}
