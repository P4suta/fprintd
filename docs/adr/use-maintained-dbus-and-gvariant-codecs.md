---
date: 2026-10-22
---

# Use maintained D-Bus and GVariant codecs

## Status

Accepted.

## Context

The standalone zvariant, zvariant_derive, and zbus_names crates are unmaintained after their merge into zbus 6.
The daemon consumes D-Bus wire types, while the independent FP3 test oracle consumes GVariant.
The published libraries expose a Rust 1.82 contract and keep wire adapters outside the domain core.

## Decision

Use zbus 6 for the daemon with explicit Tokio, proxy, service, object-manager, and tracing features.
Declare its required Rust 1.89 floor only in the unpublished daemon crate.
Use zgvariant 2 as the dev-only GVariant oracle with the same frozen fixtures and field assertions.
Keep the published libraries' Rust 1.82 floor and production dependency boundaries.
Run the FP3 oracle beside the daemon in Linux verification because the codecs use separate crates.

## Consequences

Building the daemon and its verification workspace requires Rust 1.89 or newer.
The D-Bus interface names, authorization decisions, FP3 bytes, and published library floors stay the same.
The independent GVariant reader remains part of the test gate without an advisory exception.

## Alternatives

Keeping zbus 5 retains the unmaintained D-Bus dependencies.
Suppressing their advisories leaves that runtime dependency unresolved.
Removing the independent reader reduces FP3 verification coverage.

## References

- [Upstream crate migration](https://github.com/z-galaxy/zbus/issues/2043)
- [zgvariant migration guide](https://github.com/z-galaxy/zgvariant#user-content-migrating-from-zvariants-gvariant-feature)
