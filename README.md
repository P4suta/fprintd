# fprintd

A GObject-free, **pure-Rust** fingerprint stack that speaks fprintd's D-Bus contract
(`net.reactivated.Fprint`), so the existing Linux desktop / PAM login stack (pam_fprintd,
GNOME/KDE settings) runs on it unchanged, plus an embeddable `fprint-core` library underneath.

It keeps the C **libfprint** underneath as a dynamically linked shim and depends on the fprintd
package for pam_fprintd, the D-Bus policy, and the PolicyKit actions. See
[`ARCHITECTURE.md`](ARCHITECTURE.md) for the design and decisions.

## Crates

Dependencies flow only toward the leaves; `fprint-core` knows nothing about any
backend or wire format ([`ARCHITECTURE.md`](ARCHITECTURE.md) §The one rule).

| crate | role | platform |
|---|---|---|
| [`fprint-core`](crates/fprint-core) | device/print domain model + `Backend`/`Device` traits, zero dependencies, `#![forbid(unsafe_code)]` | any |
| [`fprint-fp3`](crates/fprint-fp3) | FP3 on-disk template (de)serialization — a hand-rolled GVariant codec (edge translator) | any |
| [`fprint-bozorth3`](crates/fprint-bozorth3) | BOZORTH3 minutiae matcher — self-contained, zero-dependency NBIS port | any |
| [`fprint-mindtct`](crates/fprint-mindtct) | MINDTCT minutiae detector — self-contained, zero-dependency NBIS port | any |
| [`fprint-pipeline`](crates/fprint-pipeline) | host-image glue: image → minutiae → template → match — the published front door for matching | any |
| [`fprint-backend-libfprint`](crates/fprint-backend-libfprint) | the shim: dynamically links the C libfprint, owning the FFI directly via `libfprint-sys` | Linux |

The published crates.io surface is exactly those six. The rest are internal
(`publish = false`) and never leave the workspace:

| crate | role | platform |
|---|---|---|
| [`fprint-backend-native`](crates/fprint-backend-native) | virtual device + host-image `Device` for offline testing; an **experimental** USB capture seam behind the `usb` feature | any |
| [`fprint-integration`](crates/fprint-integration) | `CompositeBackend` / `CompositeDevice` — the one layer that may know every backend. **Not in any shipped artifact**: no binary consumes it (the daemon wires the shim directly), so the pure-Rust native device is never offered to real users through the daemon | any |
| [`fprintd`](crates/fprintd) | the `net.reactivated.Fprint` daemon (zbus + PolicyKit) | Linux |
| [`fprint-cli`](crates/fprint-cli) (`fprint`) · [`fprint-driverkit`](crates/fprint-driverkit) (`fpdev`) | demo and driver-author workbench binaries | any |

## Status

What is verified, and what is not:

| layer | state |
|---|---|
| **Core + arithmetic kernels + codec + pipeline** (`fprint-core`, `fprint-fp3`, `fprint-bozorth3`, `fprint-mindtct`, `fprint-pipeline`) | Complete and **golden bit-exact** — matchers/detector verified black-box against the stock C NBIS tools, FP3 verified byte-for-byte against real libfprint, the pipeline glue tested end-to-end (image → minutiae → match). All offline, no hardware. |
| **Shim daemon** (`fprintd` + `fprint-backend-libfprint`) | Implemented; CI green. Verified only against libfprint's **virtual drivers** in Docker — not yet exercised on a real sensor or a real PAM login. |
| **Native** (`fprint-backend-native`) | Host-image matching (image→minutiae→match) works offline and is tested. The USB capture seam is **experimental** and hardware-unverified — see below. |

Native drivers are a non-goal ([`ARCHITECTURE.md`](ARCHITECTURE.md) §Non-goals). To bring up a
sensor natively, plug into the capture seam — see
[`docs/adding-a-driver.md`](docs/adding-a-driver.md).

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

The repository follows the [REUSE](https://reuse.software) specification: every file declares
its licensing via an SPDX header or `REUSE.toml`, and `reuse lint` passes. Every crate is
`MIT OR Apache-2.0`, the NBIS ports included; the shim links the C **libfprint**
(LGPL-2.1-or-later) by dynamic linking only. Only NIST's golden test data stays marked public
domain. See [`ARCHITECTURE.md`](ARCHITECTURE.md) §Provenance & licensing.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
