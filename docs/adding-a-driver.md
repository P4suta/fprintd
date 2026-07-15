# Adding a native driver — an open invitation

Bringing up a fingerprint sensor with a pure-Rust driver is **welcome, and never
required**. This project's north star is coexistence: we speak fprintd's contract
and keep the C libfprint as a shim so real hardware works today. Reaching parity
with libfprint's driver estate is explicitly a non-goal (see
[`ARCHITECTURE.md`](../ARCHITECTURE.md) §Non-goals) — so a native driver is a gift
to the ecosystem, contributed at your pace, not a milestone the project is racing
toward.

If you'd like to try, here is where to plug in.

## The capture seam

A native, host-image sensor is expressed as one small async trait,
[`FrameSource`](../crates/fprint-backend-native/src/frame_source.rs), in
`fprint-backend-native`:

```rust
pub trait FrameSource {
    async fn capture(&mut self) -> Result<Capture>;   // the one poll boundary
    async fn arm(&mut self) -> Result<()> { Ok(()) }    // default no-op
    async fn disarm(&mut self) -> Result<()> { Ok(()) }
}
```

Your driver's whole job is to turn hardware into a grayscale `Frame`. Everything
downstream is already built and verified:

- [`ImageDevice<S: FrameSource>`](../crates/fprint-backend-native/src/image_device.rs)
  drives your source and is a complete `fprint_core::Device` (enroll / verify / identify).
- [`detector`](../crates/fprint-backend-native/src/detector.rs) runs `fprint-mindtct`
  (frame → minutiae) and [`matcher`](../crates/fprint-backend-native/src/matcher.rs)
  runs `fprint-bozorth3` (minutiae → score). Both are golden bit-exact.

So a new driver is *just* a `FrameSource` — you do not touch `fprint-core`, the daemon,
or the matcher.

## Reference template

Three `FrameSource` implementors already exist to copy from:

- [`SyntheticFrameSource`](../crates/fprint-backend-native/src/sources/synthetic.rs) and
  [`FileFrameSource`](../crates/fprint-backend-native/src/sources/file.rs) — hardware-free.
- [`UsbFrameSource`](../crates/fprint-backend-native/src/usb/source.rs) — an **experimental,
  hardware-unverified** worked example for the Validity VFS5011, layered as
  `proto` (pure framing) → `transport` (the `nusb` seam) → `source` (the driver) →
  `vfs5011` (device constants). Its protocol values are placeholders marked
  "HW-verified: required"; treat it as a shape to follow, not a working driver.

A good pattern is to keep protocol framing in pure, unit-tested `Vec<u8>` code and
confine `unsafe`/`nusb` I/O to the transport leaf, exactly as `usb/` does.

## License discipline

This is the one hard rule for driver contributions (see
[`ARCHITECTURE.md`](../ARCHITECTURE.md) §Provenance & licensing):

- **Original code from interoperability facts is fine.** VID/PID, endpoints, frame
  geometry, register names, and init sequences are *facts*, not copyrightable
  expression. Document them and write original Rust.
- **Transliterating a libfprint driver is not.** A line-by-line port of LGPL driver
  code is a derivative work; it cannot be `MIT OR Apache-2.0`. If you genuinely port
  one, it must live in a **separate LGPL-2.1-or-later crate**, isolated from the
  permissive core, carrying its own SPDX header.

## Acceptance criteria

- `#![forbid(unsafe_code)]` holds, except in the transport leaf where the FFI/USB
  boundary genuinely needs it (quarantined, as `nusb` is today).
- Verified black-box: golden fixtures, mock-transport tests, or captured-frame
  round-trips — the way `sources/` and `usb/mock_tests.rs` are.
- REUSE clean: every new file declares its license (inline SPDX for `.rs`, or the
  `REUSE.toml` bulk annotation), and `mise run reuse` passes.
- Passes the workspace lints: `cargo clippy --workspace --all-targets -- -D warnings`
  and `cargo fmt --all --check`.

Open a draft PR early — we're happy to help shape the seam with you.
