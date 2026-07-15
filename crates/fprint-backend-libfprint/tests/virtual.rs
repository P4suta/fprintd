// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

// Gated to Linux + the `virtual` feature: it drives the *real* C libfprint `virtual_device`
// driver over its debug socket, so it needs libfprint built with the virtual drivers. On any
// other target, or without `--features virtual`, this test crate compiles to nothing.
#![cfg(all(target_os = "linux", feature = "virtual"))]

//! End-to-end smoke test of the shim against libfprint's `virtual_device`.
//!
//! The virtual driver listens on the `FP_VIRTUAL_DEVICE` socket and is scripted with commands
//! (`SCAN <id>`, …). Its listener has a backlog of 1 and *closes the previous connection when a
//! new one arrives*, and it only turns the crank while libfprint's main loop runs — which, with
//! our blocking `*_sync` shim, is only *inside* an operation. So we cannot pre-queue commands:
//! we feed them from a background thread, one at a time with a gap, so the main loop (running
//! inside the blocking `enroll`/`verify` on the test thread) accepts and drains each command
//! before the next connection arrives. (libfprint's own Python harness gets the same effect by
//! pumping `ctx.iteration()` after every `send_command`.)
//!
//! We enroll a finger (the driver defaults to 5 stages — `virtual-device.c` `nr_enroll_stages`),
//! then verify the same id (match) and a different id (no match), and confirm the enrolled
//! [`Print`] round-trips through `fprint-fp3` — the D1 template-unification guarantee.

use std::future::Future;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::pin::pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use std::thread;
use std::time::Duration;

use fprint_backend_libfprint::LibfprintBackend;
use fprint_core::{Backend, Device, Finger, Print, Template};

const SOCKET: &str = "/tmp/fp-virt.sock";
const ENROLL_STAGES: usize = 5; // virtual_device's built-in default
const FINGER_ID: &str = "virtual-finger-1";
const OTHER_ID: &str = "virtual-finger-2";

/// Send one scripted command over its own short-lived connection.
fn send(cmd: &str) {
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
    let mut stream = stream.expect("connect to the virtual-device socket");
    stream.write_all(cmd.as_bytes()).expect("write command");
    // Dropping the stream closes our end; the buffered bytes remain readable by the driver.
}

/// Feed a sequence of commands from a background thread, paced so the driver's main loop
/// (running inside the blocking op on the caller's thread) drains each before the next arrives.
/// The initial gap lets the blocking op start its main loop before the first command lands.
fn feed(cmds: Vec<String>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        for cmd in cmds {
            send(&cmd);
            thread::sleep(Duration::from_millis(200));
        }
    })
}

#[test]
fn enroll_then_verify_over_virtual_socket() {
    let _ = std::fs::remove_file(SOCKET);
    // Must be set before the FpContext is constructed (inside `LibfprintBackend::new`).
    std::env::set_var("FP_VIRTUAL_DEVICE", SOCKET);

    let backend = LibfprintBackend::new();

    let mut dev = block_on(backend.enumerate())
        .expect("enumerate")
        .into_iter()
        .find(|d| d.info().driver.0 == "virtual_device")
        .expect("the virtual_device should be present");

    block_on(dev.open()).expect("open");
    assert_eq!(dev.info().enroll_stages as usize, ENROLL_STAGES);

    // --- Enroll: feed one SCAN of the same id per stage, paced, while enroll blocks. --------
    let feeder = feed(vec![format!("SCAN {FINGER_ID}"); ENROLL_STAGES]);
    let mut stages_seen = 0u32;
    let enrolled = block_on(dev.enroll(Print::new_for_enroll(Finger::LeftIndex), |p| {
        if p.retry.is_none() {
            stages_seen = p.completed_stages;
        }
    }))
    .expect("enroll should complete");
    feeder.join().unwrap();

    assert_eq!(
        stages_seen, ENROLL_STAGES as u32,
        "every stage should report"
    );
    assert_eq!(enrolled.finger, Some(Finger::LeftIndex));
    assert!(
        matches!(enrolled.template, Template::Raw(_)),
        "the virtual device produces a device/raw template, got {:?}",
        enrolled.template
    );

    // The enrolled print must survive a round-trip through the FP3 codec unchanged — this is
    // what lets the daemon store shim- and native-made prints uniformly (D1).
    let bytes = fprint_fp3::to_bytes(&enrolled).expect("serialize enrolled print");
    let round_tripped = fprint_fp3::from_bytes(&bytes).expect("deserialize enrolled print");
    assert_eq!(round_tripped, enrolled, "FP3 round-trip must be lossless");

    // --- M2: byte-compatibility with real libfprint -----------------------------------------
    // `enrolled` was decoded from libfprint's own `fp_print_serialize` output (see
    // `print::fp_to_core`). Our re-encoding must be *byte-identical* to libfprint's canonical FP3,
    // which we prove by showing our bytes are a fixed point of libfprint's own (de)serialize: if a
    // single framing byte differed (field order, maybe-string tag, the empty reserved vardict, the
    // Julian-day sentinel), libfprint would round-trip to different bytes and this would fail.
    {
        use libfprint_rs::FpPrint;
        let lib_canonical = FpPrint::deserialize(&bytes)
            .expect("libfprint accepts our FP3 bytes")
            .serialize()
            .expect("libfprint re-serializes");
        assert_eq!(
            bytes, lib_canonical,
            "fprint-fp3 output must be byte-identical to libfprint's canonical FP3"
        );

        // Freeze the real blob into fprint-fp3's fixtures (that crate is cross-platform and owns the
        // Docker-free native regression, tests/libfprint_fixture.rs). Opt-in so ordinary runs never
        // touch the source tree.
        if std::env::var_os("FP3_FREEZE_FIXTURES").is_some() {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../fprint-fp3/tests/fixtures");
            std::fs::create_dir_all(&dir).expect("create fixtures dir");
            std::fs::write(dir.join("libfprint_virtual_device.fp3"), &bytes)
                .expect("write fixture");
        }
    }

    // --- Verify: same id matches, a different id does not. ----------------------------------
    let feeder = feed(vec![format!("SCAN {FINGER_ID}")]);
    let good = block_on(dev.verify(&enrolled)).expect("verify (same id)");
    feeder.join().unwrap();
    assert!(good.matched, "verifying the enrolled id should match");

    let feeder = feed(vec![format!("SCAN {OTHER_ID}")]);
    let bad = block_on(dev.verify(&enrolled)).expect("verify (other id)");
    feeder.join().unwrap();
    assert!(!bad.matched, "verifying a different id should not match");

    block_on(dev.close()).expect("close");
}

// --- A dependency-free executor -----------------------------------------------------------
//
// The shim's async fns contain no `.await` points (every op is a blocking `*_sync` FFI call),
// so each future is ready on its first poll. A no-op waker is all we need.

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(out) = future.as_mut().poll(&mut cx) {
            return out;
        }
    }
}

fn noop_waker() -> Waker {
    const VTABLE: RawWakerVTable = RawWakerVTable::new(|_| RAW, |_| {}, |_| {}, |_| {});
    const RAW: RawWaker = RawWaker::new(std::ptr::null(), &VTABLE);
    // SAFETY: the vtable's clone returns the same no-op RawWaker and wake/drop do nothing, so
    // the waker upholds the Waker contract trivially.
    unsafe { Waker::from_raw(RAW) }
}
