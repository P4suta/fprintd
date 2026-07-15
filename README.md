# fprintd

A modern, GObject-free, **pure-Rust** fingerprint stack that speaks fprintd's
D-Bus contract (`net.reactivated.Fprint`), so the existing Linux desktop / PAM
login stack (pam_fprintd, GNOME/KDE settings) runs on it unchanged — plus a
clean, embeddable `fprint-core` library underneath.

> **North star: we don't rebuild fprintd — we coexist with it.** We speak the
> ecosystem's real contract, keep the C **libfprint** underneath as a dynamically
> linked shim, and layer on top the simple, modern mechanism today's Rust makes
> possible. See [`ARCHITECTURE.md`](ARCHITECTURE.md) for the full design and
> rationale — including the prime directive that *architectural beauty is the
> supreme value of this project.*

## Crates

Dependencies flow only toward the leaves; `fprint-core` knows nothing about any
backend or wire format ([`ARCHITECTURE.md`](ARCHITECTURE.md) §The one rule).

| crate | role | platform |
|---|---|---|
| [`fprint-core`](crates/fprint-core) | the crystal: device/print domain model + `Backend`/`Device` traits, zero dependencies, `#![forbid(unsafe_code)]` | any |
| [`fprint-fp3`](crates/fprint-fp3) | FP3 on-disk template (de)serialization — a hand-rolled GVariant codec (edge translator) | any |
| [`fprint-bozorth3`](crates/fprint-bozorth3) | BOZORTH3 minutiae matcher — self-contained, zero-dependency NBIS port | any |
| [`fprint-mindtct`](crates/fprint-mindtct) | MINDTCT minutiae detector — self-contained, zero-dependency NBIS port | any |
| [`fprint-backend-native`](crates/fprint-backend-native) | virtual device + host-image matching; an **experimental** USB capture seam behind the `usb` feature | any |
| [`fprint-backend-libfprint`](crates/fprint-backend-libfprint) | the shim: dynamically links the C libfprint via the `libfprint-rs` FFI crate | Linux |
| [`fprint-integration`](crates/fprint-integration) | `CompositeBackend` / `CompositeDevice` — the one layer that knows every backend | any |
| [`fprintd`](crates/fprintd) | the `net.reactivated.Fprint` daemon (zbus + PolicyKit) | Linux |

## Status

Honest about what is verified and what is not:

| layer | state |
|---|---|
| **Crystal + arithmetic kernels + codec** (`fprint-core`, `fprint-fp3`, `fprint-bozorth3`, `fprint-mindtct`) | Complete and **golden bit-exact** — matchers/detector verified black-box against the stock C NBIS tools, FP3 verified byte-for-byte against real libfprint. All offline, no hardware. |
| **Shim daemon** (`fprintd` + `fprint-backend-libfprint`) | Implemented; CI green. Verified only against libfprint's **virtual drivers** in Docker — not yet exercised on a real sensor or a real PAM login. |
| **Native** (`fprint-backend-native`) | Host-image matching (image→minutiae→match) works offline and is tested. The USB capture seam is an **experimental, hardware-unverified invitation** — see below. |

**Native drivers are an open invitation, not a goal.** Reaching parity with
libfprint's driver estate is explicitly a non-goal ([`ARCHITECTURE.md`](ARCHITECTURE.md)
§Non-goals). If you want to bring up a sensor natively, the capture seam is the
place to plug in — see [`docs/adding-a-driver.md`](docs/adding-a-driver.md).

## Build & test

```sh
cargo test --workspace          # unit + golden-fixture tests (offline, no hardware)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
mise run reuse                  # REUSE/SPDX license-hygiene lint
mise run docker-test            # Linux shim + daemon tests against real libfprint (Docker)
```

## Documentation

- [`ARCHITECTURE.md`](ARCHITECTURE.md) — the design, the one rule, key decisions, provenance
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — how to build, test, and contribute
- [`docs/`](docs) — format specs and algorithm/development notes (see [`docs/README.md`](docs/README.md))

## License

Licensed under either of

- Apache License, Version 2.0
  ([`LICENSES/Apache-2.0.txt`](LICENSES/Apache-2.0.txt) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license
  ([`LICENSES/MIT.txt`](LICENSES/MIT.txt) or
  <https://opensource.org/licenses/MIT>)

at your option.

The repository follows the [REUSE](https://reuse.software) specification: every
file declares its licensing via an SPDX header or `REUSE.toml`, and `reuse lint`
is expected to pass. Provenance is kept clean by matching only *interoperability
facts* (enum values, wire-format signatures, D-Bus names) and never
transliterating LGPL implementation code. A backend that links the C
**libfprint** (LGPL-2.1-or-later) does so by *dynamic linking* only. Every crate here
is `MIT OR Apache-2.0`, the NBIS ports included: they are faithful ports of NBIS,
which carries no copyright at all (17 USC §105) and so restricts neither the port nor
the licence on the result. Only NIST's own golden test data stays marked public domain.
See [`ARCHITECTURE.md`](ARCHITECTURE.md) §Provenance & licensing.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
