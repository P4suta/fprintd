<!--
SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
SPDX-License-Identifier: MIT OR Apache-2.0
-->

# Known issues & tracked debt

This project keeps its tracked liabilities here rather than as prose scattered in module
docs. Each entry states **what**, **why it exists**, and the **removal condition**. Source
sites carry a `HW-verified: required` marker pointing back here.

## Pinned versions

- `glib`/`gio` track whatever generation `libfprint-sys` resolves (a mismatch links two
  `libgobject` symbol sets). Keep them aligned.
- `docker/Dockerfile` `LIBFPRINT_REF=v1.94.10` is pinned to match the vendored `reference/`
  copy so the shim's `bindgen` output is deterministic — this is good practice, not debt.

## FP3 codec: byte-exactness vs. real libfprint (M2) — VALIDATED

`fprint-fp3` hand-rolls the GVariant serialization (no `zvariant`). Its correctness is proven three
ways: self round-trip identity (`from_bytes(to_bytes(p)) == p`); byte-for-byte equality against
**frozen golden fixtures**; and **byte-identity against a real C libfprint blob**:

- The shim's Docker tests enroll on the real virtual drivers and assert our `to_bytes` output is a
  **fixed point of the shim's own libfprint FFI `deserialize`/`serialize`** (`src/ffi.rs`, exposed
  to the tests as `libfprint_canonical_fp3`) — i.e. byte-identical to libfprint's canonical FP3.
  Both template kinds are covered: `tests/virtual.rs` drives `virtual_device` for
  the opaque `Raw`/match-on-chip path, and `tests/virtual_image.rs` drives `virtual_image`, an
  image device, so libfprint runs the real NBIS extractor and serializes an `FPI_PRINT_NBIS` print.
- Both real blobs are frozen under `crates/fprint-fp3/tests/fixtures/`
  (`libfprint_virtual_device.fp3`, `libfprint_virtual_image_nbis.fp3`), and `fprint-fp3`'s
  `tests/libfprint_fixture.rs` re-validates decode + byte-exact re-encode **on any platform,
  without Docker**.

The NBIS fixture carries no biometric data: the frames fed to `virtual_image` are a synthetic image
from `fprint-mindtct`'s golden corpus (`loop_200x240.raw`, which stock NBIS resolves to 26 minutiae).
Enrolling a real finger to fill this gap would put an irrevocable biometric in the repository, so it
is not an option — see `SECURITY.md`.

## Experimental native USB capture seam (unpublished, off by default)

`fprint-backend-native`'s `usb` feature and the `fpdev` (`fprint-driverkit`) live-USB paths are a
**worked example of a native host-image driver, not a working one**. Native drivers are a non-goal;
see [ADR 0004](adr/0004-coexistence-shim-first.md). This is a hardware-gated boundary. Both crates
are `publish = false`; the feature is off by default, so nothing on crates.io reaches any of it.

| id | site | what | removal condition |
|---|---|---|---|
| **HW-1** | `crates/fprint-backend-native/src/usb/vfs5011.rs` | The VFS5011 device constants and init/deinit handshake **bytes** (endpoints, `WIDTH`/`HEIGHT`/`PPI`, the reset/configure/stop transfers) are structurally plausible placeholders marked `HW-verified: required`; the module never asserts as fact a byte it has not observed on a sensor. | Confirm each value against a physical Validity VFS5011, replacing the placeholder with the observed byte. |
| **HW-2** | `crates/fprint-backend-native/src/usb/transport.rs` (`NusbTransport`) | The real `nusb`-backed bulk/control transport renders the intended transfer calls but has done **no real I/O**; the exact `nusb` call shapes can only be confirmed against hardware. | Drive a real capture end-to-end through `NusbTransport` and reconcile the calls. |

## Native USB capture seam — offline layers

Everything above the two hardware-gated spots is exercised offline and is **complete**: the protocol framing
(`usb::proto`), the scripted transport and cassette replay (`usb::scripted`, `usb::wire`), the
`UsbFrameSource` → detect → match path (`crates/fprint-backend-native/tests/reference_replay.rs`),
frame decode, and `fpdev`'s `probe`/`import`/`replay`/`frame`/`match`/`doctor` commands. Live bus
**enumeration** (`fpdev probe` with no selector, `list_usb_devices`) is the one live-USB path that
needs no specific sensor — it opens nothing — and is wired behind the `usb` feature. The seam
graduates from worked-example to working driver when **HW-1** and **HW-2** are confirmed on a
physical unit.

## Supply chain

`cargo deny` gates advisories, licences, bans and sources (the `deny` CI job). Two advisories are
**deliberately ignored**, recorded here so each decision is re-read every run rather than buried in
`deny.toml`. Both are *unmaintained* advisories (informational, not vulnerabilities), and both reach
the graph only through `fprint-driverkit`'s (`publish = false`) diagnostic imaging — nothing shipped
or published reaches either:

| id | crate | reachability | why ignored |
|---|---|---|---|
| **RUSTSEC-2026-0192** | `ttf-parser` (unmaintained) | `imageproc` -> `ab_glyph` -> `owned_ttf_parser` -> `ttf-parser` | Arrives solely through imageproc's glyph/text-drawing path, which this repo never calls — the `fpdev` diagnostic overlay draws circles and lines (`crates/fprint-driverkit/src/diag/overlay.rs`), never text. Revisit if imageproc is dropped or `ttf-parser` gains a real vulnerability advisory. |
| **RUSTSEC-2024-0436** | `paste` (unmaintained) | `image` -> `exr` -> `pulp` -> `paste` | A proc-macro helper reached only through the `image` crate's OpenEXR codec — a format `fpdev` never saves or opens (it uses PNG). Revisit if `image` drops the codec or `paste` gains a real vulnerability advisory. |

The permissive licences `Zlib` (the `image` crate's PNG deflate codec — unavoidable for the overlay's
PNG save/open) and `NCSA` (`libfuzzer-sys`, the fuzz harness runtime) are allow-listed in `deny.toml`:
both are permissive, and the only boundary the licence policy polices is LGPL, which nothing here crosses.
