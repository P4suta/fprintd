# 0003 — fprintd compatibility, not libfprint compatibility

- Status: Accepted
- Date: 2026-07-18

## Context

The ecosystem's real contract is fprintd's D-Bus interface, not libfprint's C ABI. libfprint
drivers are compiled into the C library against a private `fpi_*` API with no plugin or ABI
boundary, so they cannot be reused wholesale. The available paths are FFI-linking the whole C
library (the shim) or porting drivers by hand.

## Decision

Match fprintd's D-Bus contract. The shim (`fprint-backend-libfprint`, dynamically linked
libfprint) is the main line. Native pure-Rust drivers are a non-goal the project does not
measure itself against; they enter through the capture seam (`docs/adding-a-driver.md`).

## Consequences

- Real hardware works today through the shim, across libfprint's ~28 drivers — an unbounded,
  device-dependent axis (`docs/M0-ground-truth.md`).
- Native drivers are welcome contributions, never required. See
  [Non-goals](../../ARCHITECTURE.md#non-goals).
- The C ABI is not a compatibility target; only the D-Bus contract is.
