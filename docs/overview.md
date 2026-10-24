# Project overview

The Linux daemon implements fprintd's `net.reactivated.Fprint` D-Bus contract through a dynamically linked libfprint backend.
The Linux desktop integration uses the upstream fprintd package's PAM module, D-Bus policy, and PolicyKit actions.
The Rust libraries also support fingerprint image processing, template persistence, and matching without a Linux service.
[Architecture and provenance](../ARCHITECTURE.md) describes the dependency boundaries and interoperability evidence.

## Public libraries

`fprint-core` defines the device and print domain, including the `Backend` and `Device` traits, without dependencies or unsafe code.
`fprint-mindtct` detects minutiae, and `fprint-bozorth3` implements the BOZORTH3 matcher.
Both kernels are self-contained ports of the public-domain NBIS algorithms.
`fprint-pipeline` joins images, minutiae, templates, and match scores.
`fprint-fp3` reads and writes libfprint's FP3 template format.
`fprint-backend-libfprint` provides the Linux backend through `libfprint-sys`.

These six crates form the published package surface.
The kernel tests use frozen C NBIS results, the FP3 tests compare real libfprint bytes, and pipeline tests exercise image processing through matching.

## Internal components

`fprint-backend-native` supplies virtual devices and host-image matching.
Its USB capture feature remains experimental and lacks hardware verification.
`fprint-integration` combines backends, but the daemon wires the libfprint backend directly and does not expose native capture to users.
`fprintd` contains the Linux service.
`fprint-cli` supplies the `fprint` demonstration tool, and `fprint-driverkit` supplies the `fpdev` driver workbench.
`fprint-testkit` supplies deterministic test inputs, and `xtask` contains development commands.
These packages have `publish = false`.

The demonstrated source installation installs the hardware-free `fprint` CLI rather than the Linux service.
Its `demo` command enrolls a synthetic finger, accepts a recapture, and rejects a different synthetic finger without persistent data or a sensor.
[The quickstart](quickstart.md) describes the examples and library boundaries.

## Verification limits

Linux integration tests exercise the daemon and libfprint's virtual drivers on a private D-Bus instance.
The documented real-sensor coverage is limited to the UPEK TouchStrip `0483:2016` with the `upekts` driver.
Real PAM login and the system bus lack end-to-end verification.
Native USB drivers remain outside the supported service path.
[The driver guide](adding-a-driver.md) describes the experimental capture seam.

## Development

Windows and macOS build the portable crates while the Linux service modules remain inactive.
The existing development checks are:

```sh
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
mise run lint
mise run reuse
```

Linux service and native libfprint verification use the documented container:

```sh
mise run docker-test
```

[The contributor guide](../CONTRIBUTING.md) describes the development gates and package checks.
[The documentation index](README.md) links the format specifications, algorithm references, and operating notes.

## Licensing

The Rust code uses `MIT OR Apache-2.0`, including the NBIS ports.
The existing texts remain in [the license directory](../LICENSES), and each published crate retains its packaged license assets.
The libfprint shim links LGPL-2.1-or-later libfprint dynamically.
The NIST golden fixtures keep their public-domain declarations.
REUSE annotations identify each file's copyright and license.
