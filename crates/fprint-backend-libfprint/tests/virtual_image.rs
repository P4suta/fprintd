// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

// Gated to Linux + the `virtual` feature, like `virtual.rs`: it drives the *real* C libfprint
// `virtual_image` driver, so it needs libfprint built with the virtual drivers.
#![cfg(all(target_os = "linux", feature = "virtual"))]

//! The NBIS half of the FP3 byte-identity proof (`docs/known-issues.md` §M2).
//!
//! `virtual.rs` covers the `Raw`/match-on-chip path via `virtual_device`. This covers the other
//! template kind: `virtual_image` is an `FpImageDevice`, so libfprint runs the real NBIS
//! minutiae extractor over the frames we feed it and serializes an `FPI_PRINT_NBIS` print. Our
//! `to_bytes` must be byte-identical to that.
//!
//! Its own test binary because the driver is selected by a process-global env var.
//!
//! No biometric data is involved: the frame is a synthetic image from `fprint-mindtct`'s golden
//! corpus (`mise run mindtct-oracle`), which stock NBIS resolves to 26 minutiae. Enrolling a
//! real finger to fill this fixture would put an irrevocable biometric in the repository.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use fprint_backend_libfprint::LibfprintBackend;
use fprint_core::{Backend, Device, Finger, Print, Template};
use fprint_testkit::block_on;

const SOCKET: &str = "/tmp/fp-virt-image.sock";
/// `virtual_image` is an image device, and those enroll in 5 stages (`IMG_ENROLL_STAGES`).
const ENROLL_STAGES: usize = 5;
/// Matches `loop_200x240.manifest` (width height dpi) in the MINDTCT corpus.
const WIDTH: i32 = 200;
const HEIGHT: i32 = 240;

fn frame() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../fprint-mindtct/tests/fixtures/loop_200x240.raw");
    let bytes = std::fs::read(&path).expect("read the synthetic MINDTCT frame");
    assert_eq!(
        bytes.len(),
        (WIDTH * HEIGHT) as usize,
        "frame size vs header"
    );
    bytes
}

/// Feed `count` copies of the frame over one connection.
///
/// Unlike `virtual_device`, `virtual-image.c` serves a single client and loops on it, so this
/// holds the connection open. The header is two native-endian `gint`s (width, height) followed
/// by `width * height` 8-bit pixels; the driver's `automatic_finger` defaults to on, so each
/// frame carries its own implicit finger-on/finger-off and needs no extra commands.
///
/// Paced like `virtual.rs`: libfprint's main loop only turns inside a blocking op, so the gaps
/// let the driver consume each frame before the next arrives.
fn feed(count: usize) -> thread::JoinHandle<()> {
    let data = frame();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        let mut stream = None;
        for _ in 0..50 {
            match UnixStream::connect(SOCKET) {
                Ok(s) => {
                    stream = Some(s);
                    break;
                }
                Err(_) => thread::sleep(Duration::from_millis(20)),
            }
        }
        let mut stream = stream.expect("connect to the virtual-image socket");
        for _ in 0..count {
            let mut msg = Vec::with_capacity(8 + data.len());
            msg.extend_from_slice(&WIDTH.to_ne_bytes());
            msg.extend_from_slice(&HEIGHT.to_ne_bytes());
            msg.extend_from_slice(&data);
            stream.write_all(&msg).expect("write frame");
            stream.flush().expect("flush frame");
            thread::sleep(Duration::from_millis(300));
        }
    })
}

#[test]
fn enrolled_nbis_print_is_byte_identical_to_libfprint() {
    let _ = std::fs::remove_file(SOCKET);
    // Must be set before the FpContext is constructed (inside `LibfprintBackend::new`).
    std::env::set_var("FP_VIRTUAL_IMAGE", SOCKET);

    let backend = LibfprintBackend::new();
    let mut dev = block_on(backend.enumerate())
        .expect("enumerate")
        .into_iter()
        .find(|d| d.info().driver.as_str() == "virtual_image")
        .expect("the virtual_image device should be present");

    block_on(dev.open()).expect("open");

    let feeder = feed(ENROLL_STAGES);
    let enrolled = block_on(dev.enroll(Print::new_for_enroll(Finger::LeftIndex), |_| {}))
        .expect("enroll should complete");
    feeder.join().unwrap();

    // The point of using an image device: libfprint ran NBIS and produced minutiae, not an
    // opaque device blob.
    let Template::Nbis(captures) = &enrolled.template else {
        panic!("expected an NBIS template, got {:?}", enrolled.template);
    };
    assert!(
        captures.iter().any(|c| !c.is_empty()),
        "NBIS should have extracted minutiae from the synthetic loop frame"
    );

    // Same fixed-point argument as `virtual.rs`: `enrolled` was decoded from libfprint's own
    // `fp_print_serialize` output, so if our re-encoding differed by one framing byte,
    // libfprint would round-trip it to different bytes.
    let bytes = fprint_fp3::to_bytes(&enrolled).expect("serialize enrolled print");
    {
        let lib_canonical = fprint_backend_libfprint::libfprint_canonical_fp3(&bytes)
            .expect("libfprint accepts our FP3 bytes and re-serializes");
        assert_eq!(
            bytes, lib_canonical,
            "fprint-fp3 output must be byte-identical to libfprint's canonical NBIS FP3"
        );
    }

    let round_tripped = fprint_fp3::from_bytes(&bytes).expect("deserialize enrolled print");
    assert_eq!(round_tripped, enrolled, "FP3 round-trip must be lossless");

    // Freeze for `fprint-fp3`'s Docker-free regression, as `virtual.rs` does for the Raw path.
    if std::env::var_os("FP3_FREEZE_FIXTURES").is_some() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fprint-fp3/tests/fixtures");
        std::fs::create_dir_all(&dir).expect("create fixtures dir");
        std::fs::write(dir.join("libfprint_virtual_image_nbis.fp3"), &bytes)
            .expect("write fixture");
    }

    block_on(dev.close()).expect("close");
}
