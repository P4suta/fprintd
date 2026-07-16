<!-- SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors -->
<!-- -->
<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# fprint-fp3

The FP3 fingerprint-template codec: the edge translator between libfprint/fprintd's on-disk
`"FP3"` blob and the `fprint_core::Print` domain model. An FP3 blob is the three ASCII bytes
`"FP3"` followed by a little-endian, normal-form GVariant value of type `(issbymsmsia{sv}v)`.
Everything wire-specific — the magic, the GVariant signature, the Julian-day dates with their
`G_MININT32` sentinel, the maybe-strings, the NBIS payload — lives here and never leaks up into
`fprint-core`. It depends only on `fprint-core`. The public surface is two verbs, `to_bytes`
and `from_bytes`.

## Quickstart

```text
use fprint_core::{Finger, Minutia, Print, Template};

let print = Print {
    template: Template::Nbis(vec![vec![Minutia { x: 12, y: 34, theta: 90 }]]),
    finger: Some(Finger::RightIndex),
    username: Some("alice".into()),
    ..Default::default()
};

let bytes = fprint_fp3::to_bytes(&print)?;         // -> Vec<u8>, starts with fprint_fp3::MAGIC
let same = fprint_fp3::from_bytes(&bytes)?;        // -> Print
```

The crate-root docs state exactly what round-trips and where two wire-inexpressible
distinctions collapse (`finger: None` and empty driver/device strings).

## Links

- API docs: <https://docs.rs/fprint-fp3>
- crates.io: <https://crates.io/crates/fprint-fp3>
- Format spec: `docs/fp3-format.md`

## License

`MIT OR Apache-2.0`, at your option.
