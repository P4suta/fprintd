<!--
SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
SPDX-License-Identifier: MIT OR Apache-2.0
-->

# Known issues & tracked debt

This project keeps its tracked liabilities here rather than as prose scattered in module
docs. Each entry states **what**, **why it exists**, and the **removal condition**. Source
sites carry an `// UPSTREAM(...)` / `// TRACKED(...)` marker pointing back here.

## libfprint-rs 0.3.1 FFI-binding workarounds (shim only)

The `fprint-backend-libfprint` shim links the C libfprint through the crates.io `libfprint-rs`
0.3.1 binding. Five workarounds exist because that binding version under-delivers. All are
isolated to the shim, `// SAFETY:`-documented, and **pinned to `=0.3.1` / `=0.2.0`** (see
below) so a patch release cannot silently change the behavior they depend on.

| id | site | what | removal condition |
|---|---|---|---|
| **M2-A** | `crates/fprint-backend-libfprint/src/convert.rs` (`device_name`) | 0.3.1's `FpDevice::name()` wraps the *transfer-none* `fp_device_get_name` with `from_glib_full` → tries to free a string it doesn't own → panics on the virtual driver. We read the name ourselves with correct transfer-none semantics via raw FFI. | Binding fixes the transfer annotation → delete `device_name`, use `dev.name()`. |
| **M2-B** | `crates/fprint-backend-libfprint/src/storage.rs` (whole module) | 0.3.1's `list_prints`/`delete_print`/`clear_storage` wrappers are `unimplemented!()`. We call the C `fp_device_*_sync` entry points directly through `libfprint-sys` and translate GLib ownership at the boundary. | Binding implements the wrappers → replace the raw module with binding calls. |
| **M2-C** | `crates/fprint-backend-libfprint/src/convert.rs` (non-x86 `features`) | The binding gates its safe `features()` iterator to `x86`/`x86_64`; other arches need a raw `fp_device_get_features` FFI call. | Binding drops the arch gate. (Also: exercise aarch64 in CI so the fallback doesn't rot.) |
| **M2-D** | `crates/fprint-backend-libfprint/src/convert.rs` (error domains) | The binding never wrapped `FpDeviceError`/`FpDeviceRetry` as typed glib error domains, so we introduce local zero-cost `ErrorDomain` markers and `as`-cast the enums (discriminants are interop facts). | Binding exports typed error domains → drop the local markers. |
| **M2-E** | `crates/fprint-backend-libfprint/src/convert.rs` (`temperature`, `finger_status`) | The binding exposes no `FpDevice` temperature getter at all, and its `FpDevice::finger_status()` folds the `FpFingerStatusFlags` bitmask into a three-value enum that `panic!`s on any combination of flags. We read both via the raw `fp_device_get_temperature` / `fp_device_get_finger_status` FFI and map the raw values ourselves (temperature → `DeviceInfo::temperature`, finger status → `EnrollProgress::finger_status`). | Binding adds a temperature getter and a non-panicking finger-status accessor → drop both helpers, use the binding. |

**Longer-term option:** three of these four already reach *past* the binding into `libfprint-sys`.
If that trend continues, the shim could depend on `libfprint-sys` directly and drop the
`libfprint-rs` binding entirely (it would then earn its keep only for `FpContext`/`FpDevice`
object-lifetime management). Evaluate at M2.

## Exact version pins

- `libfprint-rs = "=0.3.1"`, `libfprint-sys = "=0.2.0"` — **exact**, because the M2-A/M2-B
  workarounds depend on 0.3.1's precise (buggy) behavior; a `0.3.2` that fixed them would turn
  our workaround into a double-free (M2-A) or dead code (M2-B). Bump deliberately, together
  with removing the matching workaround.
- `glib`/`gio` track whatever generation `libfprint-rs` resolves (a mismatch links two
  `libgobject` symbol sets). Keep aligned with the binding.
- `docker/Dockerfile` `LIBFPRINT_REF=v1.94.10` is pinned to match the vendored `reference/`
  copy so the shim's `bindgen` output is deterministic — this is good practice, not debt.

## FP3 codec: byte-exactness vs. real libfprint (M2) — VALIDATED

`fprint-fp3` hand-rolls the GVariant serialization (no `zvariant`). Its correctness is proven three
ways: self round-trip identity (`from_bytes(to_bytes(p)) == p`); byte-for-byte equality against
**frozen golden fixtures** (permanent oracles that caught two real framing bugs during the
hand-roll); and **byte-identity against a real C libfprint blob**:

- The shim's Docker tests enroll on the real virtual drivers and assert our `to_bytes` output is a
  **fixed point of libfprint's own `deserialize`/`serialize`** — i.e. byte-identical to libfprint's
  canonical FP3. Both template kinds are covered: `tests/virtual.rs` drives `virtual_device` for
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
