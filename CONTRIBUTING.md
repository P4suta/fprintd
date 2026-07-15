# Contributing

Thanks for your interest. This is a pure-Rust fingerprint stack that **coexists**
with the existing Linux ecosystem — it speaks fprintd's D-Bus contract and keeps
the C libfprint as a shim — rather than trying to replace it. Contributions that
sharpen that idea are very welcome.

Please also read [`ARCHITECTURE.md`](ARCHITECTURE.md); it is short and it explains
the shape of everything.

## The one rule

> **Dependencies flow only toward the leaves.** `fp-core` knows nothing about any
> backend, transport, or wire format. Backends know `fp-core`. The integration crate
> knows the backends. The daemon knows the integration crate. There is never an arrow
> pointing back up.

This is the core norm for every change. If a patch would make `fp-core` reference a
backend, a runtime, a USB stack, or a serialization format, the design is wrong —
lift the coupling up to the integration crate instead. Keeping the core a
zero-dependency, `#![forbid(unsafe_code)]` crystal is what makes the rest possible.

## Building and testing

Everything below runs offline, with no hardware, on any platform (the Linux-only
crates compile to near-empty crates elsewhere):

```sh
cargo test --workspace                                    # unit + golden-fixture tests
cargo clippy --workspace --all-targets -- -D warnings     # warnings are hard errors, like CI
cargo fmt --all --check
mise run reuse                                            # REUSE/SPDX license-hygiene lint
```

The full shim + daemon path (real libfprint virtual drivers, the D-Bus daemon) runs
in Docker, mirroring the CI `linux` job:

```sh
mise run docker-test
```

CI (`.github/workflows/ci.yml`) runs the workspace tests on Windows and macOS, the
Docker path on Linux, and `reuse lint` — all must be green.

## License hygiene

The repository follows [REUSE](https://reuse.software): every file declares its
license via an inline SPDX header (`.rs`) or a `REUSE.toml` bulk annotation
(manifests, docs). Keep provenance clean by matching only *interoperability facts*
(enum values, wire signatures, D-Bus names) and never transliterating LGPL
implementation code. See [`ARCHITECTURE.md`](ARCHITECTURE.md) §Provenance & licensing,
and — for sensor drivers specifically — [`docs/adding-a-driver.md`](docs/adding-a-driver.md).

## Adding a native driver

Native sensor drivers are an open invitation, not a project goal. If you want to try,
[`docs/adding-a-driver.md`](docs/adding-a-driver.md) walks through the capture seam,
the reference template, and the acceptance criteria.

## Conduct

By participating you agree to uphold our [Code of Conduct](CODE_OF_CONDUCT.md).

## Developer Certificate of Origin / licensing

Unless you explicitly state otherwise, any contribution you intentionally submit for
inclusion in the work, as defined in the Apache-2.0 license, shall be dual licensed as
`MIT OR Apache-2.0`, without any additional terms or conditions. Contributions to the
public-domain NBIS crates (`fp-bozorth3`, `fp-mindtct`) follow those crates' own
notices.
