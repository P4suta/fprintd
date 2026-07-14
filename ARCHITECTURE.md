# Architecture

A modern, GObject-free, pure-Rust fingerprint stack that speaks the **fprintd**
D-Bus contract (`net.reactivated.Fprint`) so the existing Linux desktop/PAM login
stack runs on it unchanged — while giving applications a clean, embeddable Rust
library underneath.

> **Prime directive: architectural beauty is the supreme value of this project.**
> When a decision trades beauty for speed, breadth, or expedience, beauty wins.
> Everything below is downstream of that.

---

## The layering (and the one rule)

```
  fprintd-rs (bin)        net.reactivated.Fprint over zbus, PolicyKit, /var/lib/fprint
        │                 Daemon<CompositeBackend>; one actor thread per device
        ▼
  integration crate       the ONLY layer that knows every backend
        │                 CompositeBackend / enum CompositeDevice { Native, Shim }
        ▼
  fp-backend-* (leaves)   fp-backend-libfprint (FFI shim, !Send) · fp-backend-native
        │                 each implements fp-core's traits
        ▼
  fp-core (lib)           the crystal: domain model + Backend/Device traits
                          zero dependencies · #![forbid(unsafe_code)]
```

**The one rule: dependencies flow only toward the leaves.** `fp-core` knows nothing
about any backend, any transport, any wire format. Backends know `fp-core`. The
integration crate knows the backends. The daemon knows the integration crate. There
is never an arrow pointing back up. This is what keeps the core a crystal.

---

## Principles

1. **Dependency inversion, without exception.** See the rule above. If a change would
   make `fp-core` reference an implementor, the design is wrong — lift the coupling to
   the integration crate instead.

2. **The core is a zero-dependency crystal.** `fp-core` is pure domain types and traits,
   with `#![forbid(unsafe_code)]`. No async runtime, no USB, no serialization library,
   no bitflags crate. Those all live in leaves.

3. **Wire compatibility is carried by the edge translators only.** The quirks of the
   interop surfaces — FP3's GVariant `(issbymsmsia{sv}v)`, Julian-day dates with a
   `G_MININT32` sentinel, maybe-strings, endian byteswaps, D-Bus status-string
   vocabularies, the `/var/lib/fprint/<user>/<driver>/<device_id>/<finger>` layout — never leak into
   the domain model. The in-memory model stays pure; conversion happens at the outermost
   ring (the serialization module, the D-Bus adaptor).

4. **Cancellation is dropping the future.** No `GCancellable`-style token threads through
   the API. To cancel an operation, drop its future; a backend releases the sensor in its
   own `Drop`.

5. **Invariants are spoken by types, not checked at runtime.** "One operation in flight
   per device" is expressed by taking `&mut self` on every operation — the borrow checker
   forbids a concurrent enroll/verify. No mutex, no state-guard assertion.

6. **`unsafe` is quarantined to the leaves.** Only the transport and FFI crates may use
   `unsafe`, and only where the hardware/FFI boundary demands it. The core forbids it.

7. **Thread affinity is made honest, then hidden.** The libfprint shim is `!Send`
   (GObject/GMainContext is thread-affine). The core therefore requires no `Send`. The
   daemon confines each device to a single actor thread (or a `LocalSet`), so the `!Send`
   reality is contained cleanly rather than papered over with unsound `Send` impls.

---

## Key decisions

### Dispatch: native `async fn` in trait, static, in the core

`fp-core`'s `Device`/`Backend` traits use native async fn (stabilized in Rust 1.75) with
static dispatch. We deliberately do **not** put `dyn` or `async-trait` in the core:

- Native AFIT is the modern recommendation for *defining* an async trait, gives backend
  implementors the nicest authoring experience, and produces honest compiler errors.
- It keeps the core zero-dependency (principle 2) and `!Send`-friendly (principle 7).
- **Asymmetry that settles it:** a static core can grow a `dyn` bridge at a boundary later
  (via `dynosaur` or a hand-written `DynDevice`) *without touching the core trait*. The
  reverse — putting `async-trait` in the core — is a permanent contamination. We keep the
  option that can be undone.

### Runtime backend heterogeneity: `CompositeBackend`, above the core

During migration we want "this one device is served by native Rust, the rest by the
libfprint shim." Rather than admit `dyn`/enum into the core, the integration crate defines
`CompositeBackend` whose associated `Device` is `enum CompositeDevice { Native(_), Shim(_) }`
(delegation written by hand — an explicit `match self { Native(d) => d.m(..).await, … }`
per method, no macro). It is the single crate allowed to know both backends, so the
dependency arrows stay pointed down.

Why hand-written and not `enum_dispatch`: the core trait uses native `async fn` in trait,
whose per-impl return futures are not `dyn`-object-safe, so a dispatch macro built around
object-safety is the wrong tool; the `Shim` arm is also `#[cfg(target_os = "linux")]`-gated,
which a hand `match` expresses trivially. Ten four-line arms with zero extra dependencies are
more honest — and more beautiful — than generated code here.

### fprintd compatibility, not libfprint compatibility

The ecosystem's real contract is fprintd's D-Bus interface, not libfprint's C ABI. We
match the former. libfprint drivers cannot be reused wholesale anyway — they are compiled
into the C library against a private `fpi_*` API, with no plugin/ABI boundary — so the
honest paths are FFI-linking the whole C library (the shim) or porting drivers by hand.
We do both, in that order.

---

## Provenance & licensing

The project's own crates are **`MIT OR Apache-2.0`**. libfprint is **LGPL-2.1+**; NBIS
(MINDTCT/BOZORTH3) is **US-Government public domain**. To keep our license clean we hold a
hard line between two very different activities:

- **Matching an interface or wire format is allowed and safe.** Enum values, the FP3 magic
  and GVariant type signature `(issbymsmsia{sv}v)`, D-Bus names and status strings, the
  `/var/lib/fprint` layout — these are *interoperability facts*, not copyrightable
  expression. We read upstream source to *document the format* (see `docs/fp3-format.md`)
  and then write **original** Rust from that spec.
- **Transliterating LGPL implementation code is not.** A line-by-line port of, say,
  `fp-print.c`'s logic would be a **derivative work of LGPL-2.1+** and could not be
  MIT/Apache. We do not do this. When we need behavior compatibility we implement
  originally from the spec/observed bytes and verify **black-box** (round-trip against real
  libfprint), never by copying its expression.

Consequences:

- **NBIS port** — realized as **`fp-bozorth3`** (the BOZORTH3 matcher). It is written from
  **stock upstream public-domain NBIS** (see `docs/bozorth3-algorithm.md`), never from libfprint's
  patched `nbis/` (its `g_`-prefixing and patches are LGPL), and verified black-box against the stock
  C tool. A future MINDTCT port would join it as a sibling PD crate.
- **The shim** (`fp-backend-libfprint`) *dynamically links* libfprint. LGPL explicitly
  permits this from any-licensed code (unlike GPL); LGPL obligations attach only to
  distributing the linked whole, not to our source.
- Any code that genuinely *is* derived from libfprint (e.g. if we ever port a specific
  driver by transliteration) must live in a **separate LGPL crate**, isolated from the
  MIT/Apache crystal.

### Per-crate license map

License texts live **only** in `LICENSES/` (REUSE-canonical; no root duplication), and
`reuse lint` gates every file. The crate-level split:

| crate(s) | SPDX license | note |
|---|---|---|
| `fp-core`, native-driver own code, `fp-fp3`, `fprintd-rs` | `MIT OR Apache-2.0` | inline SPDX header on every source file |
| `fp-backend-libfprint` (shim) | `MIT OR Apache-2.0` (our source) | *dynamically* links libfprint; **binary** redistribution honors LGPL-2.1 §6 — the system `.so` is replaceable, so it is auto-satisfied |
| `fp-bozorth3` (BOZORTH3 matcher) | `LicenseRef-NBIS-PD` (public domain) | isolated PD crate, zero deps; original from stock NBIS, score-verified black-box against the C tool; text under `LICENSES/` |
| a genuinely libfprint-derived driver, *if ever* | `LGPL-2.1-or-later` | isolated crate; carries its own SPDX header |

`fp-bozorth3` realizes the pre-committed NBIS-PD quarantine (the first non-permissive crate); the
LGPL row still describes code that does not exist yet. The split keeps any such contribution in an
obvious, quarantined home so it never contaminates the permissive core.

*(Not legal advice; the maintainers confirm specifics before release.)*

## Crate map

| crate | role | platform | deps of note |
|---|---|---|---|
| `fp-core` | domain model + `Backend`/`Device` traits | any | **none** |
| `fp-fp3` | FP3 print (de)serialization codec (edge translator) | any | **`fp-core` only** — GVariant hand-rolled |
| `fp-bozorth3` | BOZORTH3 minutiae matcher (public-domain NBIS port) | any | **none** — self-contained PD arithmetic kernel |
| `fp-backend-libfprint` | shim over C libfprint via the `libfprint-rs` FFI crate | Linux | libfprint-2, `!Send` |
| `fp-backend-native` | pure-Rust drivers + USB/SPI transport (host-image matching via `fp-bozorth3`) | Linux (USB) | `nusb`, async runtime |
| *integration* (`fp-integration`) | `CompositeBackend` / `CompositeDevice` | any (native-only off Linux) | both backends, hand-written `match` delegation |
| `fprintd-rs` | `net.reactivated.Fprint` daemon | Linux | `zbus` (+ its `zvariant`), `tokio`, PolicyKit |

Wire formats live only in edge modules, never in `fp-core`. **FP3** (de)serialization is a
**hand-rolled GVariant codec** in `fp-fp3` — no serialization crate, so that edge crystal
depends on `fp-core` alone (its correctness is pinned by frozen golden-byte fixtures). Only the
**D-Bus** side uses `zvariant`, and it comes transitively through `zbus`; the daemon has no
direct `zvariant` dependency.

## Non-goals

- A dlopen/plugin ABI for third-party drivers (libfprint's compiled-in model is fine; we
  add drivers in-tree).
- C-ABI drop-in compatibility with `libfprint.so`.
- Windows/macOS runtime support (the daemon targets Linux; `fp-core` merely compiles
  anywhere).
