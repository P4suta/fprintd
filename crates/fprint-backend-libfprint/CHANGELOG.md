# Changelog

All notable changes to this project are documented here.
## [0.1.0] - 2026-07-17

### Build System

- Split the MSRV per crate, and make CI able to tell when it is a lie
- Enforce the repository rules in a pre-commit hook
- Adopt the pending Dependabot updates and migrate the breaking ones ([#18](https://github.com/P4suta/fprintd/pull/18))

### Documentation

- Cut the historical asides and the rhetoric
- Crate landings, docs.rs metadata, the book, and the DX manifesto
- Rewrite prose in the terse house style and record decisions as ADRs

### Features

- Ship licence texts in every published crate, verified by publish-check
- Publish the host-image pipeline and idealize the public API
- Drop-cancellable worker thread; own the libfprint FFI via libfprint-sys
- Complete the device surface and harden it for release

### Refactor

- Fprintd + fprint-* family; scope crates.io to the library layer
- Idealize the public fprint-* surface
- Tighten the published surface's self-consistency

### Testing

- Prove open re-reads the device shape
- Close the M2 NBIS gap with a real libfprint blob
- One generator for every deterministic test input

### License

- Drop the NBIS-PD quarantine — one licence across the tree


