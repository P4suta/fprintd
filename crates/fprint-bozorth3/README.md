<!-- SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors -->
<!-- -->
<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# fprint-bozorth3

A pure-Rust, dependency-free reimplementation of BOZORTH3 — NIST NBIS's minutiae fingerprint
matcher, which tolerates rotation and translation between two impressions of a finger. Given
two prints as lists of minutiae, `match_score` returns an integer score reproducing the stock
NBIS tool's output; higher means more corresponding ridge structure. The matcher is a
self-contained arithmetic kernel taking its own `Minutia` (`x`, `y`, `theta`) — the `xyt`
triple is an interoperability fact — so it has no dependency on the domain model.

## Provenance

BOZORTH3 is public-domain U.S. Government software (17 USC §105). This crate is a faithful
port of the stock upstream NBIS algorithm, verified black-box against the stock C tool, and
deliberately not derived from libfprint's patched `nbis/` copy. The NBIS lineage is
provenance, not a licence; the crate carries `MIT OR Apache-2.0` like the rest of the project.

## Quickstart

```text
use fprint_bozorth3::{match_score, Minutia, MIN_COMPUTABLE_BOZORTH_MINUTIAE};

// Convert your minutiae at the boundary — one map into Minutia { x, y, theta }.
let probe:   Vec<Minutia> = /* ... */;
let gallery: Vec<Minutia> = /* ... */;

// Under the minimum, the score is 0 ("cannot tell"); check the count first.
assert!(probe.len() >= MIN_COMPUTABLE_BOZORTH_MINUTIAE);
let score = match_score(&probe, &gallery);          // -> u32; the caller sets the threshold
```

The score is asymmetric by construction — pick an order and keep it.

## Links

- API docs: <https://docs.rs/fprint-bozorth3>
- crates.io: <https://crates.io/crates/fprint-bozorth3>
- Algorithm notes: `docs/bozorth3-algorithm.md`

## License

`MIT OR Apache-2.0`, at your option.
