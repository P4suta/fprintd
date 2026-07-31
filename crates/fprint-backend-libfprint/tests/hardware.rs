// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

// Linux-only like the rest of this crate, but deliberately *not* behind a feature: gating it
// would take it out of `cargo clippy --all-targets` too, and a test nobody compiles rots. It is
// `#[ignore]`d instead, so it is built and linted on every Linux run and executed only when
// someone asks for it by name.
#![cfg(target_os = "linux")]

//! The shim against a **real sensor**, driven by a human.
//!
//! Everything else in this crate's tests runs against libfprint's virtual drivers, which is what
//! makes them CI-able and what makes them incomplete: a virtual driver cannot tell you that the
//! device's shape is reported correctly, that a real match-on-chip template survives the FP3
//! codec, or that the worker thread survives an operation that blocks for twenty seconds on a
//! finger. This test is the one that can.
//!
//! It is `#[ignore]`d because it needs hardware and a person:
//!
//! ```console
//! $ cargo test -p fprint-backend-libfprint --test hardware -- --ignored --nocapture
//! ```
//!
//! `--nocapture` is not optional. The prompts telling you when to swipe are this test's output;
//! without it you are swiping blind.
//!
//! Run it in the bring-up container, which passes the USB bus through and has a libfprint built
//! with a hardware driver — `mise run docker-hw-shell`, then the command above. See
//! `docs/bringing-up-a-sensor.md`.
//!
//! **No print this test creates may be committed.** A real finger's template is an irrevocable
//! biometric; `SECURITY.md` is the standing rule, and the fixtures this repository ships are
//! synthetic for exactly that reason. Nothing here writes to the source tree.

use std::io::Write;

use fprint_backend_libfprint::LibfprintBackend;
use fprint_core::{Backend, Device, DeviceFeature, Finger, Print, ScanType, Template};
use fprint_testkit::block_on;

/// Drivers that are not hardware. The device under test is the first one that is not one of
/// these, so the test does not need to know which sensor is plugged in.
const VIRTUAL_DRIVERS: &[&str] = &[
    "virtual_device",
    "virtual_device_storage",
    "virtual_image",
    "virtual_device_connection",
];

/// Tell the operator what to do, and make sure they see it before the sensor starts waiting.
fn prompt(msg: &str) {
    println!("\n>>> {msg}");
    let _ = std::io::stdout().flush();
}

#[test]
#[ignore = "needs a real fingerprint sensor and a human to swipe it; see the module docs"]
fn enroll_then_verify_on_real_hardware() {
    let backend = LibfprintBackend::new();

    let mut dev = block_on(backend.enumerate())
        .expect("enumerate")
        .into_iter()
        .find(|d| !VIRTUAL_DRIVERS.contains(&d.info().driver.as_str()))
        .expect(
            "no hardware sensor found — is one plugged in, and was libfprint built with its \
             driver? (the default container image builds virtual drivers only; see \
             `mise run docker-hw-build`)",
        );

    block_on(dev.open()).expect("open");

    // The device's self-reported shape. Printed rather than asserted in general, because this
    // test is meant to work for whatever sensor is attached — these are the facts a bring-up
    // records. `dbus_device_shape.rs` models this same split for the daemon.
    let info = dev.info().clone();
    println!("\n=== device under test ===");
    println!("  driver         {}", info.driver.as_str());
    println!("  name           {}", info.name);
    println!("  scan type      {:?}", info.scan_type);
    println!("  enroll stages  {}", info.enroll_stages);
    println!(
        "  capture        {}",
        dev.has_feature(DeviceFeature::CAPTURE)
    );
    println!(
        "  verify         {}",
        dev.has_feature(DeviceFeature::VERIFY)
    );
    println!(
        "  identify       {}",
        dev.has_feature(DeviceFeature::IDENTIFY)
    );

    assert!(
        dev.has_feature(DeviceFeature::VERIFY),
        "a sensor that cannot verify has nothing for this test to check"
    );
    assert!(
        info.enroll_stages > 0,
        "a device that needs no presentations to enroll cannot be enrolled"
    );

    // The UPEK TouchStrip's shape is the one this repository already asserts from documentation
    // (`crates/fprintd/tests/dbus_device_shape.rs`): a swipe reader with 3 enroll stages, the
    // only libfprint driver that does not use 5. Hardware gets to confirm or refute it.
    if info.driver.as_str() == "upekts" {
        assert_eq!(
            info.enroll_stages, 3,
            "upekts is documented as libfprint's only 3-stage driver"
        );
        assert_eq!(
            info.scan_type,
            ScanType::Swipe,
            "the TouchStrip is a swipe sensor"
        );
    }

    // --- Enroll ------------------------------------------------------------------------------
    prompt(&format!(
        "ENROLL: present the SAME finger {} times when the sensor asks.",
        info.enroll_stages
    ));
    if info.scan_type == ScanType::Swipe {
        prompt("This is a SWIPE sensor: draw the finger across the bar slowly and evenly.");
    }

    let mut stages_seen = 0u32;
    let enrolled = block_on(dev.enroll(Print::new_for_enroll(Finger::RightIndex), |p| {
        match p.retry {
            Some(reason) => println!("  ... retry: {reason:?} — present the finger again"),
            None => {
                stages_seen = p.completed_stages;
                println!("  stage {}/{} captured", p.completed_stages, p.total_stages);
            }
        }
        let _ = std::io::stdout().flush();
    }))
    .expect("enroll should complete");

    // Deliberately *not* `stages_seen == enroll_stages`, which is what `virtual.rs` asserts and
    // what this test asserted until hardware refuted it. A real driver is not obliged to report
    // the final stage as progress: upekts reports a stage only once the *next* poll says another
    // presentation is needed (`upekts.c`, `enroll_passed` -> `fpi_device_enroll_progress`), and
    // the poll that follows the last swipe is `0x00 "enrollment complete"`, which reports
    // nothing and hands over the template instead. So a healthy 3-stage enroll here emits
    // progress twice and then completes. libfprint's own example client shows the same shape
    // ("Enroll stage 1 of 3", "stage 2 of 3", completion) — this is the contract, not a defect.
    //
    // What is genuinely required: enrollment completed (asserted above by `expect`), progress was
    // observed at all, and it never claimed more stages than the device declared.
    assert!(
        stages_seen > 0,
        "enroll reported no progress at all; the caller has nothing to drive a UI with"
    );
    assert!(
        stages_seen <= info.enroll_stages,
        "progress claimed stage {stages_seen} of a {}-stage enroll",
        info.enroll_stages
    );
    println!(
        "  enroll completed after {} progress report(s) of {} stages",
        stages_seen, info.enroll_stages
    );
    assert_eq!(enrolled.finger, Some(Finger::RightIndex));

    // A match-on-chip sensor hands back an opaque blob, not minutiae: the host stores the
    // template and gives it back to the device to compare. That is the `Raw` arm, and it is the
    // path `virtual.rs` covers with a virtual device and this covers with a real one.
    match &enrolled.template {
        Template::Raw(bytes) => println!("\n  template: Raw, {} bytes", bytes.len()),
        other => println!("\n  template: {other:?}"),
    }

    // --- The FP3 codec, against a template a real sensor produced ----------------------------
    // The same guarantee `virtual.rs` proves against `virtual_device`, which is the weaker claim:
    // this one is a blob a physical device generated.
    let bytes = fprint_fp3::to_bytes(&enrolled).expect("serialize enrolled print");
    let round_tripped = fprint_fp3::from_bytes(&bytes).expect("deserialize enrolled print");
    assert_eq!(round_tripped, enrolled, "FP3 round-trip must be lossless");

    println!("  FP3: round-trips losslessly");

    // The stronger claim — that our bytes are a *fixed point* of libfprint's own
    // deserialize/serialize, i.e. byte-identical to its canonical FP3 — needs the FFI escape
    // hatch, which lives behind `virtual` because that is what the virtual tests needed it for.
    // Run with `--features virtual` to include it; it is the check worth having here, because
    // this blob came off a physical sensor rather than a scripted one.
    #[cfg(feature = "virtual")]
    {
        let lib_canonical = fprint_backend_libfprint::libfprint_canonical_fp3(&bytes)
            .expect("libfprint accepts our FP3 bytes and re-serializes");
        assert_eq!(
            bytes, lib_canonical,
            "fprint-fp3 output must be byte-identical to libfprint's canonical FP3"
        );
        println!("  FP3: byte-identical to libfprint's own encoding");
    }
    #[cfg(not(feature = "virtual"))]
    println!("  (run with `--features virtual` to also check byte-identity with libfprint)");

    // --- Verify: the enrolled finger matches -------------------------------------------------
    prompt("VERIFY: present that SAME finger once more.");
    let good = block_on(dev.verify(&enrolled)).expect("verify (enrolled finger)");
    assert!(
        good.matched,
        "the enrolled finger should match; the sensor said it did not"
    );
    println!("  MATCH — as expected");

    // --- Verify: a different finger does not -------------------------------------------------
    // Opt-in, because it costs another presentation and a test that only ever checks the
    // positive case cannot tell "matching works" from "always says yes".
    if std::env::var_os("FP_HW_NEGATIVE").is_some() {
        prompt("NEGATIVE CHECK: present a DIFFERENT finger now.");
        let bad = block_on(dev.verify(&enrolled)).expect("verify (different finger)");
        assert!(
            !bad.matched,
            "a different finger must not match the enrolled print"
        );
        println!("  NO MATCH — as expected");
    } else {
        println!("\n  (set FP_HW_NEGATIVE=1 to also check that a different finger is rejected)");
    }

    block_on(dev.close()).expect("close");
}
