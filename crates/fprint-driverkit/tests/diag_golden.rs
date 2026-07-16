// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Golden freeze of the diagnostics over the deterministic reference finger.
//!
//! [`fprint_backend_native::SyntheticFrameSource::reference`] renders one byte-stable frame, so the
//! overlay pixels and the quality report are fixed. Freezing both to committed fixtures pins "what
//! the developer sees" (the overlay) and "what CI checks" (the report) to the same artifact: a
//! detector or renderer change that moves either fails here. Set `UPDATE_DIAG_GOLDEN=1` to rewrite
//! the fixtures after an intended change.

use pollster::block_on;

use fprint_backend_native::{Capture, Frame, FrameSource, SyntheticFrameSource};
use fprint_driverkit::diag;

const OVERLAY_PNG: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/diag_reference_overlay.png"
);
const REPORT_JSON: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/diag_reference_report.json"
);

fn reference_frame() -> Frame {
    let Capture::Frame(frame) = block_on(SyntheticFrameSource::reference().capture()).unwrap()
    else {
        panic!("the reference source yields a frame");
    };
    frame
}

fn updating() -> bool {
    std::env::var_os("UPDATE_DIAG_GOLDEN").is_some()
}

#[test]
fn overlay_matches_the_golden_png() {
    let frame = reference_frame();
    let minutiae = diag::detect(&frame);
    let overlay = diag::render_overlay(&frame, &minutiae, &diag::OverlayOptions::default());

    if updating() {
        overlay.save(OVERLAY_PNG).unwrap();
    }

    let golden = image::open(OVERLAY_PNG)
        .expect("the committed overlay golden must exist")
        .to_rgb8();
    assert_eq!(
        overlay.dimensions(),
        golden.dimensions(),
        "the overlay geometry must match the frozen golden"
    );
    assert_eq!(
        overlay.into_raw(),
        golden.into_raw(),
        "every overlay pixel must match the frozen golden"
    );
}

#[test]
fn report_matches_the_golden_json() {
    let frame = reference_frame();
    let minutiae = diag::detect(&frame);
    let report = diag::quality_report(&frame, &minutiae);
    let json = format!("{}\n", serde_json::to_string_pretty(&report).unwrap());

    if updating() {
        std::fs::write(REPORT_JSON, &json).unwrap();
    }

    let golden =
        std::fs::read_to_string(REPORT_JSON).expect("the committed report golden must exist");
    assert_eq!(
        json, golden,
        "the quality report must match the frozen golden"
    );

    // The frozen report round-trips back to an equal value, so the golden is a faithful snapshot.
    let restored: diag::QualityReport = serde_json::from_str(&golden).unwrap();
    assert_eq!(restored, report);
}
