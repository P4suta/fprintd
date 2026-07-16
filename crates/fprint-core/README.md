<!-- SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors -->
<!-- -->
<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# fprint-core

The GObject-free core of an fprintd-compatible fingerprint stack: the domain model
(fingers, device capabilities, prints/templates) plus the `Backend`/`Device` traits a
concrete backend implements. Zero dependencies, `#![forbid(unsafe_code)]`. Both traits are
native `async fn` traits with no `dyn` and no runtime, so a whole backend is a struct and two
impl blocks. It contains no drivers, no USB code, and no matching algorithms — those live in
downstream crates, and the daemon depends only on these traits, so the backend can be swapped
without touching it.

## Quickstart

```text
use fprint_core::{Backend, Device, DeviceId, Error, Result};

struct MyBackend;

impl Backend for MyBackend {
    type Device = MyDevice;
    async fn enumerate(&self) -> Result<Vec<MyDevice>> { /* probe the bus */ }
    async fn open(&self, id: &DeviceId) -> Result<MyDevice> { /* claim a reader */ }
}

// impl Device for MyDevice { open/close/enroll/verify/identify/... }
```

The crate-root docs carry a complete, compiled `Backend` + `Device` implementation that
depends on nothing but `std`.

## Links

- API docs: <https://docs.rs/fprint-core>
- crates.io: <https://crates.io/crates/fprint-core>
- Design and the one rule (dependencies flow toward the leaves): `ARCHITECTURE.md`

## License

`MIT OR Apache-2.0`, at your option.
