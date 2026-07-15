# Documentation

Design and rationale for the whole stack live in the top-level
[`ARCHITECTURE.md`](../ARCHITECTURE.md). This directory holds the format specs,
algorithm references, and development notes.

## Specifications (reader-facing)

Interoperability facts and implementable specs — written from public sources, the
basis for the original Rust implementations.

- [`fp3-format.md`](fp3-format.md) — the FP3 on-disk template format: magic, the
  GVariant type signature, NBIS/RAW payloads, MOC user-id layout.
- [`bozorth3-algorithm.md`](bozorth3-algorithm.md) — the BOZORTH3 matcher: input
  coordinate system, constants, stages, and bit-exactness notes (realized as `fprint-bozorth3`).
- [`mindtct-algorithm.md`](mindtct-algorithm.md) — the MINDTCT detector: the
  nine-stage pipeline, `lfsparms_V2` parameters, and xyt output (realized as `fprint-mindtct`).

## Contributor guide

- [`adding-a-driver.md`](adding-a-driver.md) — how to add a native sensor driver
  through the capture seam (an open invitation, not a project goal).

## Development notes (internal record)

Historical and tracking documents; useful context, not entry points.

- [`M0-ground-truth.md`](M0-ground-truth.md) — the measured SLOC of upstream libfprint /
  fprintd / NBIS and the sizing conclusions that set the shim-first strategy (research log).
- [`known-issues.md`](known-issues.md) — tracked technical debt: shim FFI workarounds,
  version pins, and FP3 byte-exactness verification status.
