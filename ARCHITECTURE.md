# Architecture

A GObject-free, pure-Rust fingerprint stack that implements the **fprintd** D-Bus contract
(`net.reactivated.Fprint`), so the existing Linux desktop/PAM stack runs on it unchanged, and
exposes an embeddable Rust library underneath.

Design priority: architectural consistency ranks above speed, breadth, and expedience. The
operating model is coexistence with the fprintd ecosystem — the daemon speaks fprintd's D-Bus
contract, links the C **libfprint** as a shim rather than reimplementing its drivers, and
depends on the fprintd package for `pam_fprintd`, the D-Bus policy, and the PolicyKit actions.
One sensor, one `/var/lib/fprint`, one D-Bus name owner: only one daemon runs at a time, so
coexistence means installable beside upstream and activated by the administrator. See
[ADR 0003](docs/adr/0003-fprintd-not-libfprint.md) and [ADR 0004](docs/adr/0004-coexistence-shim-first.md).

---

## The layering (and the one rule)

```
  fprintd (bin)        net.reactivated.Fprint over zbus, PolicyKit, /var/lib/fprint
        │                 Daemon<F>, generic over a backend factory; one actor thread per device
        │                 the shipped binary hands it fprint-backend-libfprint
        ▼
  fprint-backend-* (leaves)  fprint-backend-libfprint (FFI shim, !Send) · fprint-backend-native
        │                    each implements fprint-core's traits
        ▼
  fprint-pipeline (lib)      host-image glue: image → minutiae → template → match
        │                    joins the two NBIS kernels + fprint-core; the published front door
        ▼
  fprint-core (lib)          domain model + Backend/Device traits
                             zero dependencies · #![forbid(unsafe_code)]
    fprint-mindtct · fprint-bozorth3 (kernels)   dependency-free NBIS ports, below the pipeline

  fprint-integration      the ONLY layer that may know every backend
                          CompositeBackend / enum CompositeDevice { Native, Shim }
                          Not in the shipped graph: no artifact consumes it, and the daemon
                          would offer the native virtual device to real users if it did.
```

**The one rule: dependencies flow only toward the leaves.** `fprint-core` knows no backend,
transport, or wire format. Backends know `fprint-core`; the integration crate knows the
backends; the daemon knows a backend. No arrow points back up.

Machine-enforced by `cargo xtask lint` (`xtask/src/deps.rs`): the full graph crate by crate,
`fprint-core`'s dependency-freedom (principle 2), and the `#![forbid(unsafe_code)]` quarantine
(principle 6). A dev-dependency ships in nothing, so the graph rule does not reach it; it may
not close a cycle, which keeps the domain model untested in an implementor's terms.

---

## Principles

1. **Dependency inversion.** Dependencies flow toward the leaves. Coupling that would make
   `fprint-core` reference an implementor lifts to the integration crate.
2. **The core has zero dependencies.** `fprint-core` is domain types and traits with
   `#![forbid(unsafe_code)]`: no async runtime, USB, serialization library, or bitflags crate.
3. **Wire compatibility lives in the edge translators only.** Interop quirks — FP3's GVariant
   `(issbymsmsia{sv}v)`, Julian-day dates with a `G_MININT32` sentinel, maybe-strings, endian
   byteswaps, D-Bus status vocabularies, the
   `/var/lib/fprint/<user>/<driver>/<device_id>/<finger>` layout — convert at the outermost
   ring (serialization module, D-Bus adaptor), never in the domain model.
4. **Cancellation is dropping the future.** No cancellation token threads through the API; to
   cancel an operation, drop its future. A backend releases the sensor in its `Drop`.
5. **Invariants are spoken by types, not checked at runtime.** "One operation in flight per
   device" is `&mut self` on every operation; the borrow checker forbids a concurrent
   enroll/verify.
6. **`unsafe` is quarantined to the leaves.** Only transport and FFI crates use `unsafe`, at
   the hardware/FFI boundary. The core forbids it.
7. **Thread affinity is confined.** libfprint's GObject/GMainContext is thread-affine, so
   `LibfprintBackend` (holding the `FpContext`) is `!Send`. The shim confines each device's
   `!Send` `FpDevice` to a worker thread and exposes a `Send` handle, so only `Send` values
   cross threads. The core requires no `Send`.

---

## Key decisions

Recorded as ADRs in [`docs/adr/`](docs/adr/):

| ADR | Decision |
|---|---|
| [0001](docs/adr/0001-dispatch-native-async-fn.md) | Dispatch: native `async fn` in trait, static, in the core |
| [0002](docs/adr/0002-composite-backend.md) | Runtime backend heterogeneity: `CompositeBackend`, above the core |
| [0003](docs/adr/0003-fprintd-not-libfprint.md) | fprintd compatibility, not libfprint compatibility |
| [0004](docs/adr/0004-coexistence-shim-first.md) | Coexistence: what we install, what we borrow |
| [0005](docs/adr/0005-provenance-licensing.md) | Provenance & licensing boundary |

---

## Provenance & licensing

Project crates are **`MIT OR Apache-2.0`**. libfprint is **LGPL-2.1+**; NBIS (MINDTCT/BOZORTH3)
is **US-Government public domain**. The boundary is whose copyright a source carries. Three
rules, with rationale in [ADR 0005](docs/adr/0005-provenance-licensing.md):

1. **Matching an interface or wire format is permitted.** Enum values, the FP3 magic and
   GVariant signature `(issbymsmsia{sv}v)`, D-Bus names and status strings, the
   `/var/lib/fprint` layout — interoperability facts, not copyrightable expression. Read
   upstream to document the format (`docs/fp3-format.md`), then write original Rust.
2. **Porting public-domain code is permitted.** NBIS carries no copyright (17 USC §105).
   `fprint-bozorth3` and `fprint-mindtct` port stock NBIS line for line, because bit-exactness
   against the stock tools requires it.
3. **Transliterating LGPL implementation code is not permitted.** A line-by-line port of, say,
   `fp-print.c` would be a derivative work of LGPL-2.1+. Behavior compatibility there is
   implemented from the spec/observed bytes and verified black-box (round-trip against real
   libfprint). The NBIS ports come from stock NBIS, never libfprint's patched `nbis/` copy,
   whose changes are LGPL.

The NBIS ports carry the workspace `MIT OR Apache-2.0`; NBIS lineage is provenance recorded in
docs (`docs/bozorth3-algorithm.md`, `docs/mindtct-algorithm.md`), not a licence. Only NIST's
golden fixtures stay marked `LicenseRef-NBIS-PD`, via `REUSE.toml`.

The shim (`fprint-backend-libfprint`) dynamically links libfprint; LGPL permits this from
any-licensed code, and its obligations attach to distributing the linked whole, not to our
source. Any code genuinely derived from libfprint lives in a separate LGPL crate.

### Per-crate license map

Canonical licence texts live in `LICENSES/` (REUSE-canonical), mirrored into each published
crate by `cargo xtask sync-licenses`; `publish-check` verifies each shipped copy is
byte-identical. `reuse lint` gates every file.

| crate(s) | SPDX license | note |
|---|---|---|
| every crate in this workspace | `MIT OR Apache-2.0` | inline SPDX header on every source file, NBIS ports included |
| `fprint-backend-libfprint` (shim) | `MIT OR Apache-2.0` (our source) | dynamically links libfprint; binary redistribution honors LGPL-2.1 §6, auto-satisfied because the system `.so` is replaceable |
| NIST golden fixtures (test data) | `LicenseRef-NBIS-PD` | stock-NBIS reference output and NIST imagery, annotated in `REUSE.toml`; text under `LICENSES/` |
| a libfprint-derived driver, if ever | `LGPL-2.1-or-later` | isolated crate; own SPDX header |

One licence across the tree; the only boundary worth policing is LGPL. No crate occupies that
row — the slot exists so such a contribution has a quarantined home.

`MIT OR Apache-2.0` also suits a system daemon, which lives or dies by distro packaging: it is
the Rust ecosystem default every distro accepts (Fedora
[disallows CC0 for code](https://lwn.net/Articles/902410/), which waives no patent rights;
Apache-2.0 grants them). Permissive code flows into surrounding GPL projects; GPL code could
not flow back.

*(Not legal advice; the maintainers confirm specifics before release.)*

## Crate map

| crate | role | platform | deps of note |
|---|---|---|---|
| `fprint-core` | domain model + `Backend`/`Device` traits | any | **none** |
| `fprint-fp3` | FP3 print (de)serialization codec (edge translator) | any | **`fprint-core` only** — GVariant hand-rolled |
| `fprint-bozorth3` | BOZORTH3 minutiae matcher (port from public-domain NBIS) | any | **none** |
| `fprint-mindtct` | MINDTCT minutiae detector (port from public-domain NBIS) | any | **none** |
| `fprint-pipeline` | host-image glue (image→minutiae→template→match); the published front door for matching | any | `fprint-core`, `fprint-mindtct`, `fprint-bozorth3` — owns the boundary conversions |
| `fprint-backend-libfprint` | shim owning the C libfprint FFI directly via `libfprint-sys` | Linux | libfprint-2, `!Send` |
| `fprint-backend-native` | virtual device + host-image `Device` (matching via `fprint-pipeline`); an **experimental** USB capture seam behind the `usb` feature | any | `fprint-pipeline`, `fprint-mindtct`; `nusb` *(optional, experimental)* — async is hand-rolled, no runtime dep |
| *integration* (`fprint-integration`) | `CompositeBackend` / `CompositeDevice` | any (native-only off Linux) | both backends, hand-written `match` delegation |
| `fprintd` | `net.reactivated.Fprint` daemon | Linux | `zbus` (+ its `zvariant`), `tokio`, PolicyKit |

Wire formats live only in edge modules, never in `fprint-core`. FP3 (de)serialization is a
hand-rolled GVariant codec in `fprint-fp3` — no serialization crate, so it depends on
`fprint-core` alone; its correctness is pinned by frozen golden-byte fixtures. Only the D-Bus
side uses `zvariant`, transitively through `zbus`; the daemon has no direct `zvariant`
dependency.

## Non-goals

- **Reimplementing libfprint's driver estate in Rust.** Parity with its ~28 hardware drivers
  is an unbounded, device-dependent axis. Native drivers are contributions through the capture
  seam (`docs/adding-a-driver.md`), not a project yardstick.
- **Extending the `net.reactivated.Fprint` contract.** We implement it; we do not add to it.
  The borrowed D-Bus policy enforces this — see [ADR 0004](docs/adr/0004-coexistence-shim-first.md).
- **Our own PAM module, D-Bus policy, or PolicyKit actions.** They come from the fprintd
  package ([ADR 0004](docs/adr/0004-coexistence-shim-first.md)).
- **A dlopen/plugin ABI for third-party drivers.** A contributed native driver goes in-tree;
  no plugin boundary.
- **C-ABI drop-in compatibility with `libfprint.so`.**
- **Windows/macOS runtime support.** The daemon targets Linux; `fprint-core` merely compiles
  anywhere.
