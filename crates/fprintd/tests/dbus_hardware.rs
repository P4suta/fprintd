// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The `net.reactivated.Fprint` surface against a **real sensor**, driven by a human.
//!
//! [`dbus_integration`] proves the same contract over the virtual backend, which is what makes it
//! CI-able. This is its hardware twin: identical client flow — `GetDevices` → `Claim` →
//! `EnrollStart` → `enroll-completed` → `VerifyStart` → `verify-match` → `Release` — but the
//! daemon is wired to [`fprint_backend_libfprint`], the shim the production binary uses
//! (`fprintd::run`), so the bytes come off a physical reader.
//!
//! What this adds over `tests/hardware.rs` in the shim crate: the daemon's actor threads, its FP3
//! storage round-trip (the enrolled print is written to disk and read back to verify against),
//! and the D-Bus vocabulary a desktop client actually speaks.
//!
//! It is `#[ignore]`d because it needs hardware and a person:
//!
//! ```console
//! $ cargo test -p fprintd --test dbus_hardware -- --ignored --nocapture
//! ```
//!
//! `--nocapture` is not optional: the prompts telling you when to present a finger are this
//! test's stdout. Run it in the bring-up container (`mise run docker-hw-daemon`), which passes
//! the USB bus through and has a libfprint built with a hardware driver.
//!
//! The daemon is served on a **private session bus**, not the system bus, so this needs no D-Bus
//! policy and cannot collide with a distro `fprintd`. The three lines `serve_system` adds on top
//! of `attach` — connecting to the system bus and requesting the well-known name — are the one
//! part of the production path it does not cover.
//!
//! **No print this test creates may be committed.** The store is a temporary directory that is
//! removed on the way out, and it is never inside the source tree; `SECURITY.md` is the rule.

#![cfg(target_os = "linux")]

mod common;
use common::{DeviceProxy, ManagerProxy, PrivateBus};

use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use fprintd::{ActionSet, Authorizer, Daemon, Store};
use futures_util::StreamExt;
use tokio::time::timeout;

/// The finger name in the `net.reactivated.Fprint` vocabulary. Any of them works — what matters
/// is that enroll and verify are given the same one, and that the operator presents one finger
/// consistently. The label is metadata; the sensor never learns which finger it really was.
const FINGER: &str = "right-index-finger";

/// Generous enough for a human who has to read a prompt, find a swipe bar, and get three clean
/// passes — and still bounded, so a stalled transfer fails the test instead of hanging CI's
/// descendant forever.
const ENROLL_TIMEOUT: Duration = Duration::from_secs(180);
const VERIFY_TIMEOUT: Duration = Duration::from_secs(90);

/// Tell the operator what to do, and make sure they see it before the sensor starts waiting.
fn prompt(msg: &str) {
    println!("\n>>> {msg}");
    let _ = std::io::stdout().flush();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs a real fingerprint sensor and a human to present a finger; see the module docs"]
async fn enroll_then_verify_over_dbus_on_real_hardware() {
    let _bus = PrivateBus::shared();

    let tmp = std::env::temp_dir().join(format!("fprintd-hw-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);

    // The same factory `fprintd::run` builds in production: a fresh libfprint backend per actor
    // thread, because the `FpContext` it holds is `!Send`.
    let factory = || fprint_backend_libfprint::LibfprintBackend::new();

    let daemon = Daemon::with_store(
        factory,
        Arc::new(Authorizer::Fixed(ActionSet::ALL)),
        Store::with_root(tmp.clone()),
    );
    let builder = zbus::connection::Builder::session()
        .expect("session bus")
        .name("net.reactivated.Fprint")
        .expect("request name");
    let _daemon_conn = daemon.attach(builder).await.expect("attach daemon");

    let client = zbus::Connection::session().await.expect("client session");
    let manager = ManagerProxy::new(&client).await.expect("manager proxy");

    let devices = manager.get_devices().await.expect("get devices");
    assert!(
        !devices.is_empty(),
        "the daemon found no device — is a sensor plugged in, and was libfprint built with its \
         driver? (see `mise run docker-hw-build`)"
    );
    let device_path = devices[0].clone();

    let device = DeviceProxy::builder(&client)
        .path(device_path)
        .expect("device path")
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
        .expect("device proxy");

    // The contract's own claim: a device's shape is undefined until it is claimed, which is why
    // the property is specified as -1 rather than a guess (see `dbus_device_shape.rs`).
    assert_eq!(
        device.num_enroll_stages().await.expect("stages"),
        -1,
        "num-enroll-stages must be -1 before Claim"
    );

    device.claim("").await.expect("claim");

    let stages = device.num_enroll_stages().await.expect("stages");
    let scan_type = device.scan_type().await.expect("scan type");
    let name = device.name().await.expect("name");
    println!("\n=== device over D-Bus ===");
    println!("  name             {name}");
    println!("  scan-type        {scan_type}");
    println!("  num-enroll-stages {stages}");
    assert!(
        stages > 0,
        "a claimed device must report a real stage count, got {stages}"
    );

    // --- Enroll -------------------------------------------------------------------------------
    prompt(&format!(
        "ENROLL: present the SAME finger {stages} times when the sensor asks."
    ));
    if scan_type == "swipe" {
        prompt("This is a SWIPE sensor: draw the finger across the bar slowly and evenly.");
    }

    let mut enroll_stream = device.receive_enroll_status().await.expect("enroll stream");
    device.enroll_start(FINGER).await.expect("enroll start");
    let completed = timeout(ENROLL_TIMEOUT, async {
        while let Some(sig) = enroll_stream.next().await {
            let args = sig.args().expect("enroll args");
            if args.done {
                return args.result.to_string();
            }
            println!("  enroll: {}", args.result);
            let _ = std::io::stdout().flush();
        }
        String::from("<stream ended>")
    })
    .await
    .expect("enroll timed out — did the finger reach the sensor?");
    assert_eq!(completed, "enroll-completed");
    device.enroll_stop().await.expect("enroll stop");
    println!("  enroll-completed");

    // The print went through `fprint-fp3` to disk and is listed back by name. On a match-on-chip
    // sensor this is the whole storage story: the host keeps the opaque template the device gave
    // it, because the device keeps nothing.
    let fingers = device.list_enrolled_fingers("").await.expect("list");
    assert_eq!(fingers, vec![FINGER.to_string()]);
    println!("  stored and listed back as {FINGER}");

    // --- Verify: the enrolled finger matches ---------------------------------------------------
    prompt("VERIFY: present that SAME finger once more.");
    let mut verify_stream = device.receive_verify_status().await.expect("verify stream");
    device.verify_start(FINGER).await.expect("verify start");
    let result = timeout(VERIFY_TIMEOUT, async {
        while let Some(sig) = verify_stream.next().await {
            let args = sig.args().expect("verify args");
            if args.done {
                return args.result.to_string();
            }
            println!("  verify: {}", args.result);
            let _ = std::io::stdout().flush();
        }
        String::from("<stream ended>")
    })
    .await
    .expect("verify timed out — did the finger reach the sensor?");
    assert_eq!(result, "verify-match");
    device.verify_stop().await.expect("verify stop");
    println!("  verify-match");

    // --- Verify: a different finger does not ---------------------------------------------------
    // Opt-in, for the same reason as the shim's hardware test: a suite that only ever checks the
    // positive case cannot tell "matching works" from "always says yes".
    if std::env::var_os("FP_HW_NEGATIVE").is_some() {
        prompt("NEGATIVE CHECK: present a DIFFERENT finger now.");
        let mut stream = device.receive_verify_status().await.expect("verify stream");
        device.verify_start(FINGER).await.expect("verify start");
        let result = timeout(VERIFY_TIMEOUT, async {
            while let Some(sig) = stream.next().await {
                let args = sig.args().expect("verify args");
                if args.done {
                    return args.result.to_string();
                }
            }
            String::from("<stream ended>")
        })
        .await
        .expect("verify timed out");
        assert_eq!(
            result, "verify-no-match",
            "a different finger must not match the enrolled print"
        );
        device.verify_stop().await.expect("verify stop");
        println!("  verify-no-match");
    } else {
        println!("\n  (set FP_HW_NEGATIVE=1 to also check that a different finger is rejected)");
    }

    device.release().await.expect("release");

    let _ = std::fs::remove_dir_all(&tmp);
}
