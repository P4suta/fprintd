# Frozen fuzz findings

One file per input that once broke `fprint-fp3`. `tests/regressions.rs` walks this directory and
asserts that none panics the decoder, and that each one which decodes is a fixed point of
decode∘encode.

The file **name carries the error class**: `0001_<error-class>.fp3`. A failure then names the bug
rather than a number.

This directory is not `tests/fixtures/`. Those are frozen goldens, regenerated from stock NBIS by
`cargo xtask {bozorth3,mindtct}-oracle` and never edited by hand. These are hand-picked
counterexamples, added one at a time when `cargo xtask fuzz` finds one.

**The directory is empty of inputs when no campaign has found anything.** That is a fact about the
codec, and `tests/regressions.rs` passes on it. It is not an invitation to invent one.

Adding a finding:

1. Copy the artifact `cargo xtask fuzz` reported out of `fuzz/artifacts/<target>/`.
2. Name it `<next-number>_<error-class>.fp3`.
3. Declare it in a crate-local `REUSE.toml` — it is a binary file and carries no header.
4. Watch `cargo test -p fprint-fp3` fail, then fix the bug.
