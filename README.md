# libfprint-rs (working title)

A modern, GObject-free, **pure-Rust** fingerprint stack that speaks fprintd's
D-Bus contract (`net.reactivated.Fprint`), so the existing Linux desktop / PAM
login stack (pam_fprintd, GNOME/KDE settings) runs on it unchanged — plus a
clean, embeddable `fp-core` library underneath.

> Status: **M0** (ground-truth / scaffolding). Design and rationale live in
> [`ARCHITECTURE.md`](ARCHITECTURE.md).

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
**libfprint** (LGPL-2.1-or-later) does so by *dynamic linking* only; any code
genuinely derived from libfprint — or a future public-domain NBIS port — lives
in a separately-licensed crate isolated from the `MIT OR Apache-2.0` core. See
[`ARCHITECTURE.md`](ARCHITECTURE.md) §Provenance & licensing.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
