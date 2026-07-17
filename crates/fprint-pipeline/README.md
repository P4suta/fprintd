<!-- SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors -->
<!-- -->
<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# fprint-pipeline

The host-image fingerprint pipeline in a few lines. It joins the three published leaves —
`fprint-mindtct` (minutiae detection), `fprint-bozorth3` (minutiae matching) and `fprint-core`
(the domain `Print`/`Template`/`Minutia`) — into the one path a host-image sensor walks:
**image → minutiae → template → match**.

The two NBIS kernels are dependency-free and each defines its own `Minutia` (the `xyt` triple is
an interoperability fact, not a shared type). This crate owns the small conversions between them
and the domain model, so you do not have to write them. Add just this crate: it re-exports
`fprint_core`, `fprint_mindtct` and `fprint_bozorth3`, so their types are reachable without naming
them as separate dependencies.

## Quickstart

```text
use fprint_pipeline::{template_from_images, nbis_match_score, GrayImage};

// Build a frame view over your captured 8-bit grayscale pixels.
let img = GrayImage::new(&pixels, width, height, ppi)?;

// Enroll one capture, then score a second capture of the same finger against it.
let enrolled = template_from_images(&[img]);
let scanned  = template_from_images(&[scan]);
let score = nbis_match_score(&enrolled, &scanned);   // -> u32; the caller sets the threshold
```

To persist an enrolled print in libfprint's on-disk format, add `fprint-fp3` and call its
`to_bytes` / `from_bytes` on a `Print` carrying the template — persistence is a separate crate.

See `docs/quickstart.md` for a runnable walkthrough, and
`cargo run -p fprint-pipeline --example enroll_verify` for the whole loop in code.

## Links

- API docs: <https://docs.rs/fprint-pipeline>
- crates.io: <https://crates.io/crates/fprint-pipeline>
- Architecture & provenance: `ARCHITECTURE.md`

## License

`MIT OR Apache-2.0`, at your option.
