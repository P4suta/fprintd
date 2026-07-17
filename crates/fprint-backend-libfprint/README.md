<!-- SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors -->
<!-- -->
<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# fprint-backend-libfprint

A `fprint_core::Backend`/`fprint_core::Device` implementation that owns the C libfprint FFI
directly through `libfprint-sys`, so the pure-Rust daemon can drive every sensor libfprint
already supports. The LGPL library is linked dynamically — an interoperability boundary LGPL
explicitly permits — and this crate's own source stays MIT/Apache. Linux only: on every other
target a crate-level `#![cfg]` empties the crate so the cross-platform workspace still builds.

## Two things worth knowing

- `!Send` backend, worker-thread device — libfprint's objects are glib `GObject`s bound to their
  creating thread, so `LibfprintBackend` (which holds the `FpContext`) is `!Send`. Each device is
  driven on its own worker thread that owns the `FpDevice`, so `LibfprintDevice` is a `Send` handle
  to it. `fprint-core` never requires `Send`.
- Drop-cancellable — the worker parks inside each blocking `*_sync`, so the operation future yields
  and dropping it fires a `Send` `gio::Cancellable` cross-thread to cancel the call. The shim is
  fully drop-cancellable, like `fprint-backend-native`.

## Quickstart

```text
use fprint_core::{Backend, DeviceId};
use fprint_backend_libfprint::LibfprintBackend;

let backend = LibfprintBackend::new();
let devices = backend.enumerate().await?;           // libfprint's discovered readers
let id: DeviceId = devices[0].info().id.clone();
let mut device = backend.open(&id).await?;
```

## Links

- API docs (built on Linux): <https://docs.rs/fprint-backend-libfprint>
- crates.io: <https://crates.io/crates/fprint-backend-libfprint>
- Design and provenance: `ARCHITECTURE.md`

## License

`MIT OR Apache-2.0`, at your option.
