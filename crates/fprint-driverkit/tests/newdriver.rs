// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `fpdev new-driver` is deterministic: rendering the `acme` scaffold must byte-match the committed
//! golden fixture, file for file. This is the offline oracle behind `--check` — a template edited
//! without its golden (or the reverse) fails here, on any platform, with no hardware.
//!
//! The golden tree under `tests/golden/newdriver_acme/` is itself a real driver of the `usb/`
//! shape: the living proof it compiles and self-verifies is `fprint-backend-native`'s `vfs5011`,
//! which shares this exact layering.

use std::fs;
use std::path::{Path, PathBuf};

use fprint_driverkit::newdriver::{self, Family, NewDriverOptions};

/// The `acme` scaffold parameters the committed golden fixture was generated with.
fn acme_options() -> NewDriverOptions {
    NewDriverOptions::from_args(
        "acme",
        "1c7a",
        "0570",
        Family::HostImage,
        "vfs5011",
        None,
        false,
    )
    .expect("acme options parse")
}

/// The committed golden fixture directory.
fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join("newdriver_acme")
}

#[test]
fn render_byte_matches_the_golden_fixture() {
    let generated = newdriver::render(&acme_options());
    assert_eq!(
        generated.len(),
        5,
        "a host-image scaffold is exactly five files"
    );

    for file in &generated {
        let golden_path = golden_dir().join(&file.name);
        let golden = fs::read_to_string(&golden_path)
            .unwrap_or_else(|e| panic!("read golden {}: {e}", golden_path.display()));
        assert_eq!(
            file.contents, golden,
            "{} drifted from its golden fixture — regenerate the golden or fix the template",
            file.name
        );
    }
}

#[test]
fn the_golden_tree_has_exactly_the_generated_files() {
    let generated: Vec<String> = newdriver::render(&acme_options())
        .into_iter()
        .map(|f| f.name)
        .collect();
    let mut on_disk: Vec<String> = fs::read_dir(golden_dir())
        .expect("read golden dir")
        .map(|e| {
            e.expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    on_disk.sort();
    let mut expected = generated.clone();
    expected.sort();
    assert_eq!(
        on_disk, expected,
        "the golden directory must hold exactly the generated files, no more, no less"
    );
}

// REUSE-IgnoreStart
#[test]
fn every_generated_file_carries_the_spdx_header() {
    for file in newdriver::render(&acme_options()) {
        assert!(
            file.contents
                .contains("SPDX-License-Identifier: MIT OR Apache-2.0"),
            "{} is missing the SPDX license header",
            file.name
        );
        assert!(
            file.contents
                .contains("SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors"),
            "{} is missing the SPDX copyright header",
            file.name
        );
    }
}
// REUSE-IgnoreEnd

#[test]
fn device_file_tags_every_value_hw_verified() {
    let files = newdriver::render(&acme_options());
    let device = files
        .iter()
        .find(|f| f.name == "acme.rs")
        .expect("device module is <name>.rs");
    assert!(
        device.contents.matches("HW-verified: required").count() >= 5,
        "every placeholder device value must be tagged `HW-verified: required`"
    );
    assert!(device.contents.contains("VENDOR_ID: u16 = 0x1c7a"));
    assert!(device.contents.contains("PRODUCT_ID: u16 = 0x0570"));
}

#[test]
fn mock_tests_reference_the_offline_self_verify_harness() {
    let files = newdriver::render(&acme_options());
    let mock = files
        .iter()
        .find(|f| f.name == "mock_tests.rs")
        .expect("mock_tests.rs is generated");
    assert!(
        mock.contents.contains("SyntheticFrameSource"),
        "the mock test scripts the synthetic reference finger"
    );
    assert!(
        mock.contents.contains("ScriptedTransport"),
        "the mock test drives the driver over a scripted transport"
    );
    assert!(
        mock.contents.contains("enroll") && mock.contents.contains("verify"),
        "the mock test enrolls then self-verifies so the driver is green from minute one"
    );
}
