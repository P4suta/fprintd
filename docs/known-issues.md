<!--
SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
SPDX-License-Identifier: MIT OR Apache-2.0
-->

# Known issues & tracked debt

This project keeps its tracked liabilities here rather than as prose scattered in module
docs. Each entry states **what**, **why it exists**, and the **removal condition**. Source
sites carry an `// UPSTREAM(...)` / `// TRACKED(...)` marker pointing back here.

## libfprint-rs 0.3.1 FFI-binding workarounds (shim only)

The `fp-backend-libfprint` shim links the C libfprint through the crates.io `libfprint-rs`
0.3.1 binding. Four workarounds exist because that binding version under-delivers. All are
isolated to the shim, `// SAFETY:`-documented, and **pinned to `=0.3.1` / `=0.2.0`** (see
below) so a patch release cannot silently change the behavior they depend on.

| id | site | what | removal condition |
|---|---|---|---|
| **M2-A** | `crates/fp-backend-libfprint/src/convert.rs` (`device_name`) | 0.3.1's `FpDevice::name()` wraps the *transfer-none* `fp_device_get_name` with `from_glib_full` → tries to free a string it doesn't own → panics on the virtual driver. We read the name ourselves with correct transfer-none semantics via raw FFI. | Binding fixes the transfer annotation → delete `device_name`, use `dev.name()`. |
| **M2-B** | `crates/fp-backend-libfprint/src/storage.rs` (whole module) | 0.3.1's `list_prints`/`delete_print`/`clear_storage` wrappers are `unimplemented!()`. We call the C `fp_device_*_sync` entry points directly through `libfprint-sys` and translate GLib ownership at the boundary. | Binding implements the wrappers → replace the raw module with binding calls. |
| **M2-C** | `crates/fp-backend-libfprint/src/convert.rs` (non-x86 `features`) | The binding gates its safe `features()` iterator to `x86`/`x86_64`; other arches need a raw `fp_device_get_features` FFI call. | Binding drops the arch gate. (Also: exercise aarch64 in CI so the fallback doesn't rot.) |
| **M2-D** | `crates/fp-backend-libfprint/src/convert.rs` (error domains) | The binding never wrapped `FpDeviceError`/`FpDeviceRetry` as typed glib error domains, so we introduce local zero-cost `ErrorDomain` markers and `as`-cast the enums (discriminants are interop facts). | Binding exports typed error domains → drop the local markers. |

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

## FP3 codec: byte-exactness vs. real libfprint (M2)

`fp-fp3` hand-rolls the GVariant serialization (no `zvariant`). Its correctness is proven
**today** two ways: self round-trip identity (`from_bytes(to_bytes(p)) == p`), and byte-for-byte
equality against **frozen golden fixtures** captured from a known-correct GVariant encoder
(these live in the codec tests and are permanent oracles — they already caught two real framing
bugs during the hand-roll). What remains for **M2** is validation against blobs produced by a
*real* libfprint/fprintd (enroll via the `virtual-image` driver, or read `/var/lib/fprint`
fixtures) — i.e. confirming interop with the actual on-disk stores, not just internal
consistency. Tracked here; not a blocker for M1.
