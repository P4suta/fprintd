# Quickstart

From a fresh clone to a working enroll-and-verify in about thirty seconds. No
fingerprint hardware, no Docker, no daemon — synthetic prints and the pure-Rust
crates.

## The demo

```console
$ cargo run -p fprint-cli -- demo
```

The `fprint` CLI enrolls a synthetic minutiae template on the virtual device, accepts a recapture, and rejects a different synthetic finger.
The example exercises template matching through the device interface.
It does not run image detection or a physical sensor.
Add `--json` for machine-readable output.

## The pieces on their own

Each layer ships a runnable example so a single command exercises it in
isolation. All depend only on `std` or their existing normal dependencies, so
nothing is added to the published graph.

```console
# The whole loop in code (fprint-pipeline): detect, enroll, verify, reject.
$ cargo run -p fprint-pipeline --example enroll_verify

# The matcher (fprint-bozorth3): score two xyt minutiae files.
$ cargo run -p fprint-bozorth3 --example match_xyt a.xyt b.xyt

# The detector (fprint-mindtct): find minutiae in a grayscale PGM.
$ cargo run -p fprint-mindtct --example detect_pgm finger.pgm

# The format (fprint-fp3): encode a print to FP3 bytes and read it back.
$ cargo run -p fprint-fp3 --example roundtrip
```

## What just ran

The stack layers strictly, dependencies flowing toward the leaves:

- `fprint-core` — zero-dependency domain types and the backend traits.
- `fprint-mindtct` — minutiae detection (a bit-exact NBIS port).
- `fprint-bozorth3` — minutiae matching (a bit-exact NBIS port).
- `fprint-pipeline` — the glue that joins the two kernels into image → minutiae
  → template → match; the published front door for host-image matching.
- `fprint-fp3` — the on-disk template format libfprint reads and writes.
- the backends — native (host-image sensors) and the C-libfprint shim.

## The xyt boundary

Minutiae cross crate boundaries as `(x, y, theta)` triples in the NBIS xyt
convention: `x` and `y` in pixels, `theta` in degrees. Each kernel owns its own
`Minutia` rather than sharing a type — that triple is the interop fact, the same
one the `.xyt` files above hold. The detector's `fprint_mindtct::Minutia`
exposes `as_xyt` (its output), and `fprint_bozorth3::Minutia` and
`fprint_core::Minutia` carry the full `from_xyt` / `as_xyt` / `From<(i32, i32,
i32)>` trio. `fprint-pipeline` owns the conversions between them
(`minutia_to_core`, `minutiae_to_bozorth`), so you never write that boundary by
hand.

## Where to next

- [Architecture & provenance](ARCHITECTURE.md) — the traits, the layering, and
  where each byte-exact guarantee comes from. Implementing a backend starts from
  the `fprint-core` traits documented in the [API docs](api/fprint_core/index.html).
- [The BOZORTH3 matcher](bozorth3-algorithm.md) and
  [The MINDTCT detector](mindtct-algorithm.md) — the kernel specs.
- [The FP3 print format](fp3-format.md) — the serialization contract.
- [Adding a native driver](adding-a-driver.md) — the capture seam.
