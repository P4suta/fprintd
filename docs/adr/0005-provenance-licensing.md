# 0005 — Provenance & licensing boundary

- Status: Accepted
- Date: 2026-07-18

## Context

Project crates are `MIT OR Apache-2.0`. libfprint is LGPL-2.1+. NBIS (MINDTCT/BOZORTH3) is
US-Government public domain. The boundary that governs reuse is whose copyright a source
carries, not whether code is copied.

## Decision

Three rules:

1. **Matching an interface or wire format is permitted.** Enum values, the FP3 magic and
   GVariant signature `(issbymsmsia{sv}v)`, D-Bus names and status strings, the
   `/var/lib/fprint` layout are interoperability facts, not copyrightable expression. Read
   upstream to document the format (`docs/fp3-format.md`), then write original Rust.
2. **Porting public-domain code is permitted.** NBIS carries no copyright (17 USC §105).
   `fprint-bozorth3` and `fprint-mindtct` port stock NBIS line for line, because bit-exactness
   against the stock tools requires it.
3. **Transliterating LGPL implementation code is not permitted.** A line-by-line port of, say,
   `fp-print.c` would be a derivative work of LGPL-2.1+. Behavior compatibility there is
   implemented from the spec or observed bytes and verified black-box (round-trip against real
   libfprint).

## Consequences

- The NBIS ports are faithful ports of **stock** public-domain NBIS, never libfprint's patched
  `nbis/` copy (whose `g_`-prefixing and patches are LGPL), verified black-box against the
  stock C tools. They carry the workspace `MIT OR Apache-2.0`; NBIS lineage is provenance
  recorded in `docs/bozorth3-algorithm.md` and `docs/mindtct-algorithm.md`. Only NIST's golden
  fixtures stay marked `LicenseRef-NBIS-PD`, via `REUSE.toml`.
- The shim (`fprint-backend-libfprint`) dynamically links libfprint. LGPL permits this from
  any-licensed code; its obligations attach to distributing the linked binary, not to our
  source.
- Any code genuinely derived from libfprint lives in a separate `LGPL-2.1-or-later` crate,
  isolated from the permissive tree. No crate occupies that slot today.
- `MIT OR Apache-2.0` is the Rust ecosystem default distros accept. Fedora
  [disallows CC0 for code](https://lwn.net/Articles/902410/), which waives no patent rights;
  Apache-2.0 grants them. Permissive code flows into surrounding GPL projects; GPL code could
  not flow back.

*(Not legal advice; the maintainers confirm specifics before release.)*
