# Quickstart

From a fresh clone to a working enroll-and-verify in about thirty seconds. No
fingerprint hardware, no Docker, no daemon — synthetic prints and the pure-Rust
crates.

## The demo

```console
$ cargo run -p fprint-cli -- demo
```

The `fprint` CLI wraps the native backend and runs a scripted story: enroll a
synthetic finger, verify the same finger (PASS), then present a different finger
(REJECT). Add `--json` for machine-readable output. This is the whole stack —
detection, templating, and matching — end to end.

## The pieces on their own

Each published crate ships a runnable example so a single command exercises one
layer in isolation. All depend only on `std` or their existing normal
dependencies, so nothing is added to the published graph.

```console
# The matcher (fprint-bozorth3): score two xyt minutiae files.
$ cargo run -p fprint-bozorth3 --example match_xyt a.xyt b.xyt

# The detector (fprint-mindtct): find minutiae in a grayscale PGM.
$ cargo run -p fprint-mindtct --example detect_pgm finger.pgm

# The format (fprint-fp3): encode a print to FP3 bytes and read it back.
$ cargo run -p fprint-fp3 --example roundtrip

# The whole loop in code (fprint-backend-native): enroll, verify, reject.
$ cargo run -p fprint-backend-native --example enroll_verify
```

## What just ran

The stack layers strictly, dependencies flowing toward the leaves:

- `fprint-core` — zero-dependency domain types and the backend traits.
- `fprint-mindtct` — minutiae detection (a bit-exact NBIS port).
- `fprint-bozorth3` — minutiae matching (a bit-exact NBIS port).
- `fprint-fp3` — the on-disk template format libfprint reads and writes.
- the backends — native (host-image sensors) and the C-libfprint shim.

## The xyt boundary

Minutiae cross crate boundaries as `(x, y, theta)` triples in the NBIS xyt
convention: `x` and `y` in pixels, `theta` in degrees. `Minutia` carries
`from_xyt` / `as_xyt` (and `From<(i32, i32, i32)>`) in each crate that owns a
minutia type, so the detector's output feeds the matcher without a shared type.
That triple is the interop fact — the same one the `.xyt` files above hold.

## Where to next

- [Architecture & provenance](ARCHITECTURE.md) — the traits, the layering, and
  where each byte-exact guarantee comes from. Implementing a backend starts from
  the `fprint-core` traits documented in the [API docs](api/fprint_core/index.html).
- [The BOZORTH3 matcher](bozorth3-algorithm.md) and
  [The MINDTCT detector](mindtct-algorithm.md) — the kernel specs.
- [The FP3 print format](fp3-format.md) — the serialization contract.
- [Adding a native driver](adding-a-driver.md) — the capture seam.
