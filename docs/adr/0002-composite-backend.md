# 0002 — Runtime backend heterogeneity: `CompositeBackend`, above the core

- Status: Accepted
- Date: 2026-07-18

## Context

One device can be served by native Rust and the rest by the libfprint shim, chosen at runtime.
Admitting `dyn` or an enum into the core to express this would violate the one rule and
[principle 2](../../ARCHITECTURE.md#principles).

## Decision

The integration crate (`fprint-integration`) defines `CompositeBackend`, whose associated
`Device` is `enum CompositeDevice { Native(_), Shim(_) }`. Delegation is a hand-written
`match self { Native(d) => d.m(..).await, … }` per method, no macro. It is the single crate
allowed to know both backends.

## Consequences

- The dependency arrows stay pointed toward the leaves.
- Hand-written `match`, not `enum_dispatch`: the core trait's native `async fn` returns
  per-impl futures that are not `dyn`-object-safe, so a macro built around object-safety does
  not apply. The `Shim` arm is `#[cfg(target_os = "linux")]`-gated, which a `match` expresses
  directly, and the whole delegation is one short `match` per method.
