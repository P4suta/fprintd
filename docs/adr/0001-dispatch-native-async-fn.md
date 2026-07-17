# 0001 — Dispatch: native `async fn` in trait, static, in the core

- Status: Accepted
- Date: 2026-07-18

## Context

`fprint-core`'s `Device` and `Backend` traits have async methods. The options are native
`async fn` in trait (AFIT, stable since Rust 1.75) with static dispatch, or `dyn` / the
`async-trait` crate.

## Decision

Use native `async fn` in trait with static dispatch. The core carries no `dyn` and no
`async-trait`.

## Consequences

- Keeps the core zero-dependency ([principle 2](../../ARCHITECTURE.md#principles)) and
  `!Send`-friendly (principle 7).
- The choice is reversible in one direction only: a static core can grow a `dyn` bridge at a
  boundary later (via `dynosaur` or a hand-written `DynDevice`) without touching the core
  trait. Putting `async-trait` in the core is permanent. The reversible option is kept.
