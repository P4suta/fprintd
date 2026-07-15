# Contributing

Thanks for your interest. This is a pure-Rust fingerprint stack that **coexists**
with the existing Linux ecosystem — it speaks fprintd's D-Bus contract and keeps
the C libfprint as a shim — rather than trying to replace it. Contributions that
sharpen that idea are very welcome.

Please also read [`ARCHITECTURE.md`](ARCHITECTURE.md); it is short and it explains
the shape of everything.

## The one rule

> **Dependencies flow only toward the leaves.** `fprint-core` knows nothing about any
> backend, transport, or wire format. Backends know `fprint-core`. The integration crate
> knows the backends. The daemon knows the integration crate. There is never an arrow
> pointing back up.

This is the core norm for every change. If a patch would make `fprint-core` reference a
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
Docker path on Linux, the declared MSRV, and `reuse lint` — all must be green.

Two checks are not in CI because they need Docker for reasons CI's Linux job does not
cover; run them when you touch what they check:

```sh
mise run unit-verify     # if you touched crates/fprintd/dbus/ — see xtask/src/main.rs
mise run mindtct-oracle  # DELIBERATE: regenerates frozen golden fixtures
```

Tasks that only run one command live in `mise.toml`. Anything that has to *decide*
something belongs in `xtask/` (`cargo xtask <task>`), where a compiler and clippy can
see it: shell quoted inside TOML is read by nothing and rots quietly. The oracle tasks
were exactly that, and had stopped working on Windows without anyone noticing.

## License hygiene

The repository follows [REUSE](https://reuse.software): every file declares its
license via an inline SPDX header (`.rs`) or a `REUSE.toml` bulk annotation
(manifests, docs). Keep provenance clean by matching only *interoperability facts*
(enum values, wire signatures, D-Bus names) and never transliterating LGPL
implementation code. See [`ARCHITECTURE.md`](ARCHITECTURE.md) §Provenance & licensing,
and — for sensor drivers specifically — [`docs/adding-a-driver.md`](docs/adding-a-driver.md).

Every crate here is `MIT OR Apache-2.0` (`license.workspace = true`). Keep it that way
unless there is a real reason not to, and mind this trap if you ever break the rule:

> **`reuse lint` passing does not mean the crate can be published.** REUSE accepts a
> custom `LicenseRef-*` identifier; **crates.io does not** — it requires a name from the
> [SPDX license list](https://doc.rust-lang.org/cargo/reference/manifest.html#the-license-and-license-file-fields),
> or a `license-file`. This project shipped a bespoke `LicenseRef-NBIS-PD` for months with
> a green lint, and it would have blocked publishing the two crates with the most to give.

A public-domain source is not a reason to break it: PD grants without demanding, so it
constrains neither a port nor the licence on the result. Only genuinely LGPL-derived code
needs its own crate. Non-code files that are somebody else's (the NIST golden fixtures)
are declared where they live — see the crate-local `REUSE.toml` files.

## Adding a native driver

Native sensor drivers are an open invitation, not a project goal. If you want to try,
[`docs/adding-a-driver.md`](docs/adding-a-driver.md) walks through the capture seam,
the reference template, and the acceptance criteria.

## Conduct

By participating you agree to uphold our [Code of Conduct](CODE_OF_CONDUCT.md).

## Developer Certificate of Origin / licensing

Unless you explicitly state otherwise, any contribution you intentionally submit for
inclusion in the work, as defined in the Apache-2.0 license, shall be dual licensed as
`MIT OR Apache-2.0`, without any additional terms or conditions. That applies to every
crate here, the NBIS ports included.
