<!-- SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors -->
<!-- -->
<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# fprint-backend-libfprint

A `fprint_core::Backend`/`fprint_core::Device` implementation that wraps the C libfprint (via
the crates.io `libfprint-rs` binding), so the pure-Rust daemon can drive every sensor libfprint
already supports. The LGPL library is linked dynamically — an interoperability boundary LGPL
explicitly permits — and this crate's own source stays MIT/Apache. Linux only: on every other
target a crate-level `#![cfg]` empties the crate so the cross-platform workspace still builds.

## Two constraints worth knowing

- `!Send` bridge — libfprint's objects are glib `GObject`s bound to their creating thread, so
  `LibfprintBackend`/`LibfprintDevice` are `!Send`. `fprint-core` never requires `Send`, so the
  daemon confines each device to a single actor thread.
- Best-effort cancellation — the binding exposes only the blocking `*_sync` entry points, so
  `fprint-core`'s "dropping the future cancels the operation" contract is only meaningful when
  the daemon can signal the thread the blocking call parks on. `fprint-backend-native` is fully
  drop-cancellable.

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
