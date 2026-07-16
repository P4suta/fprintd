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
lift the coupling up to the integration crate instead. `fprint-core` stays
zero-dependency and `#![forbid(unsafe_code)]`.

## Building and testing

```sh
mise run hooks   # once, per clone
```

That installs a pre-commit hook (`lefthook.yml`) which formats, then runs clippy,
rustdoc, `reuse lint`, actionlint and `mise run lint`. Every one of them is also a CI
job, so the hook is a faster failure and never the only one.

Everything below runs offline, with no hardware, on any platform (the Linux-only
crates compile to near-empty crates elsewhere):

```sh
cargo test --workspace --all-features                     # unit + golden-fixture tests
cargo clippy --workspace --all-targets --all-features -- -D warnings   # warnings are hard errors, like CI
cargo fmt --all --check
mise run lint                                             # repo rules, incl. the one rule
mise run reuse                                            # REUSE/SPDX license-hygiene lint
mise run deny                                             # advisories, licences, wildcard pins
mise run publish-check                                    # the registry's rules for the published crates
```

`--all-features` is not optional politeness: without it, `fprint-backend-native`'s `usb` seam
(and its `nusb` dependency) compiles nowhere but Linux, so a contributor taking up the invitation
in [`docs/adding-a-driver.md`](docs/adding-a-driver.md) on this project's own primary dev platform
would be the first to find it broken.

The full shim + daemon path (real libfprint virtual drivers, the D-Bus daemon) runs
in Docker, mirroring the CI `linux` job:

```sh
mise run docker-test
```

CI (`.github/workflows/ci.yml`) runs the workspace tests on Windows and macOS, the
Docker path on Linux, the systemd unit, the declared MSRV, the published crates against
the registry's rules, the supply chain, and `reuse lint` — all must be green.
`.github/workflows/scheduled.yml` runs weekly and answers what no pull request asks: a new
advisory against unchanged code, and the frozen goldens against the *next* toolchain.

**`reuse lint` passing does not mean a crate can be published**, and `mise run publish-check`
is the only thing that says otherwise: REUSE accepts a custom `LicenseRef-*` identifier and
crates.io rejects it. They are different oracles, and only one of them gates publishing.

The one thing CI does not do is regenerate the NBIS golden fixtures, because that is
the thing they exist to catch:

```sh
mise run bozorth3-oracle   # DELIBERATE: overwrites frozen goldens
mise run mindtct-oracle
```

A green test suite says the goldens pass, not that they would notice if the code stopped working.
`mise run mutants` asks the second question of the published crates: it deletes a line and checks
whether anything goes red. It is deliberate for its size alone — 5,784 mutants, each a build and a
test run, so hours — and needs no container, toolchain or network. Scope it down while working:

```sh
cargo mutants -f 'crates/fprint-bozorth3/src/cluster.rs'   # one file, minutes
```

A surviving mutant is untested code, not broken code, so this gates nothing. A pull request gets the
same question asked of its own diff, and the answers arrive as annotations on the changed lines.

Tasks that only run one command live in `mise.toml`; anything else belongs in `xtask/`
(`cargo xtask <task>`), where a compiler and clippy can see it. Shell that would branch,
loop, or capture output is read by nothing in a task runner and runs under whichever shell it
picked — `cmd.exe` on Windows, `sh` in CI — so it lives in `xtask` instead. A workflow may
chain two single commands with `&&`/`||` (they mean the same under every shell a runner
provides), but `$(…)`, `set -e` and `bash -c` are banned everywhere. `mise run lint` enforces
this across `mise.toml` and every `.github/workflows/*.yml`, along with the norms no compiler
checks: no shell scripts; no comment that narrates a past or a future the reader cannot check —
say what is true now, and let git hold the history (a generated `CHANGELOG.md` is the one
exception); and **the one rule itself**. The phrases it rejects are in
[`xtask/src/lint.rs`](xtask/src/lint.rs), and the graph it holds you to is in
[`xtask/src/deps.rs`](xtask/src/deps.rs) — which reads `cargo metadata` and pins the charter
crates' dependency-freedom and the `#![forbid(unsafe_code)]` quarantine. Both are where to add
one.

### Modern where it is free

The default is to reach for a well-chosen crate or tool, not to hand-roll. Two rules bound
where that reach may go. **The one rule:** dependencies flow toward the leaves. **The lockfile
rule:** no third-party crate enters a published crate's normal dependency graph — tooling
reaches the code as an external CLI the workspace invokes but never names (nextest, llvm-cov,
release-plz, git-cliff, mdbook), as a dev-dependency Cargo strips from the tarball, or as a
dependency of a `publish = false` crate. Three crates are the **charter**: `fprint-core` and
the two NBIS kernels take no third-party dependency in any table, because the core's zero
dependencies are the architecture and the kernels' bit-exact port *is* the product. Every other
crate takes the dependencies it needs. `cargo xtask lint` checks all of this against the
resolved graph; `cargo xtask publish-check` keeps `release-plz.toml` holding back exactly the
unpublishable crates.

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
