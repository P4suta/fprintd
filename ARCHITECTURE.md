# Architecture

A modern, GObject-free, pure-Rust fingerprint stack that speaks the **fprintd**
D-Bus contract (`net.reactivated.Fprint`) so the existing Linux desktop/PAM login
stack runs on it unchanged — while giving applications a clean, embeddable Rust
library underneath.

> **North star: we coexist with the fprintd ecosystem.**
> This daemon does reimplement fprintd; what it does not do is compete with the
> ecosystem around it. It *speaks* fprintd's D-Bus contract rather than replacing
> it, keeps the C **libfprint** underneath as a dynamically linked shim rather
> than reimplementing its driver estate, and depends on the fprintd package for
> pam_fprintd, the D-Bus policy and the PolicyKit actions rather than shipping
> rivals to them. On top of that it layers the simple, modern, genuinely
> nice-to-use mechanism that today's Rust makes possible. Native drivers are
> **not** a goal we measure ourselves against — they are an open invitation
> anyone can take up through the capture seam.
>
> Two fingerprint daemons cannot run at once: there is one sensor, one
> `/var/lib/fprint`, and a D-Bus name has one owner. So coexistence does not mean
> running alongside upstream's daemon. It means being installable beside it and
> taking the seat only when the administrator says so — see §Coexistence.

> **Prime directive: architectural beauty is the supreme value of this project.**
> When a decision trades beauty for speed, breadth, or expedience, beauty wins.
> Everything below is downstream of that. Coexistence is how that beauty reaches
> the world without waging a rewrite war.

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

**The one rule: dependencies flow only toward the leaves.** `fprint-core` knows nothing
about any backend, any transport, any wire format. Backends know `fprint-core`. The
integration crate knows the backends. The daemon knows a backend. There is never an
arrow pointing back up.

The rule is machine-enforced: `cargo xtask lint` holds the whole graph, crate by crate, in
`xtask/src/deps.rs` — along with `fprint-core`'s dependency-freedom (principle 2) and the
`#![forbid(unsafe_code)]` quarantine (principle 6). A dev-dependency ships in nothing, so the
rule does not reach it; what it may not do is close a cycle, which is how "the domain model is
never tested in an implementor's terms" is checked without being a special case.

---

## Principles

1. **Dependency inversion, without exception.** See the rule above. If a change would
   make `fprint-core` reference an implementor, the design is wrong — lift the coupling to
   the integration crate instead.

2. **The core has zero dependencies.** `fprint-core` is pure domain types and traits,
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
   own `Drop`. Both shipped backends honour this fully: `fprint-backend-native` yields once
   per capture stage, and the libfprint shim confines its blocking `*_sync` calls to a
   per-device worker thread, so the operation future yields and its `Drop` fires a `Send`
   `gio::Cancellable` that cancels the parked call cross-thread.

5. **Invariants are spoken by types, not checked at runtime.** "One operation in flight
   per device" is expressed by taking `&mut self` on every operation — the borrow checker
   forbids a concurrent enroll/verify. No mutex, no state-guard assertion.

6. **`unsafe` is quarantined to the leaves.** Only the transport and FFI crates may use
   `unsafe`, and only where the hardware/FFI boundary demands it. The core forbids it.

7. **Thread affinity is confined, not papered over.** libfprint's GObject/GMainContext is
   thread-affine, so `LibfprintBackend` (holding the `FpContext`) is `!Send`. The shim confines
   each device's `!Send` `FpDevice` to a dedicated worker thread and exposes a `Send` handle to
   it, so only `Send` values cross threads — never an unsound `Send` impl. The core therefore
   requires no `Send`, and the daemon confines each device to a single actor thread (or a
   `LocalSet`) regardless.

---

## Key decisions

### Dispatch: native `async fn` in trait, static, in the core

`fprint-core`'s `Device`/`Backend` traits use native async fn (stabilized in Rust 1.75) with
static dispatch. We deliberately do **not** put `dyn` or `async-trait` in the core:

- Native AFIT is the modern recommendation for *defining* an async trait.
- It keeps the core zero-dependency (principle 2) and `!Send`-friendly (principle 7).
- **Asymmetry that settles it:** a static core can grow a `dyn` bridge at a boundary later
  (via `dynosaur` or a hand-written `DynDevice`) *without touching the core trait*. The
  reverse — putting `async-trait` in the core — is a permanent contamination. We keep the
  option that can be undone.

### Runtime backend heterogeneity: `CompositeBackend`, above the core

One device can be served by native Rust and the rest by the libfprint shim. Rather than
admit `dyn`/enum into the core, the integration crate defines
`CompositeBackend` whose associated `Device` is `enum CompositeDevice { Native(_), Shim(_) }`
(delegation written by hand — an explicit `match self { Native(d) => d.m(..).await, … }`
per method, no macro). It is the single crate allowed to know both backends, so the
dependency arrows stay pointed down.

Why hand-written and not `enum_dispatch`: the core trait uses native `async fn` in trait,
whose per-impl return futures are not `dyn`-object-safe, so a dispatch macro built around
object-safety is the wrong tool; the `Shim` arm is also `#[cfg(target_os = "linux")]`-gated,
which a hand `match` expresses trivially, and the whole delegation is one short `match` per method.

### fprintd compatibility, not libfprint compatibility

The ecosystem's real contract is fprintd's D-Bus interface, not libfprint's C ABI. We
match the former. libfprint drivers cannot be reused wholesale anyway — they are compiled
into the C library against a private `fpi_*` API, with no plugin/ABI boundary — so the
available paths are FFI-linking the whole C library (the shim) or porting drivers by hand.

**The shim is the main line: coexistence, not conquest.** Dynamically linking the C
library lets real hardware work today and keeps us *with* the ecosystem rather than racing
to out-implement its ~28 hardware drivers — an unbounded, device-dependent axis
(`docs/M0-ground-truth.md`). Native pure-Rust drivers are therefore **not a project goal
we measure success against**; they are an open invitation that plugs into the capture seam
(see `docs/adding-a-driver.md`). Growing one is welcome, never required.

### Coexistence: what we install, and what we borrow

The daemon ships **one file**: the systemd unit (`crates/fprintd/dbus/`). The D-Bus
policy, the PolicyKit actions and `pam_fprintd.so` come from the fprintd package, which is
therefore a hard dependency (`Depends:`, not `Recommends:`). The package is named
`fprintd-rs` and the binary installs as `/usr/libexec/fprintd-rs`, so both can be
installed at once; only the bus name is shared.

The dependency is not a matter of degree. Without the D-Bus policy nothing may own
`net.reactivated.Fprint`, not even root, so the daemon does not start at all; without the
PolicyKit actions every privileged method is denied. It is all or nothing, which settles
two questions:

- **We do not write our own PAM module.** It would not remove the dependency — the policy
  and actions still come from upstream — so it buys nothing and puts 26KB of new code on
  the authentication path. `pam_fprintd` is a D-Bus client; it works as long as we keep
  the contract.
- **We cannot extend the D-Bus contract.** The borrowed policy allowlists exactly
  `net.reactivated.Fprint.Manager`, `.Device` and the three standard interfaces, so a
  method on any interface of our own would never reach us. See Non-goals.

Taking the seat is `systemctl enable fprintd-rs`, whose `Alias=fprintd.service` shadows
the upstream unit from `/etc/systemd/system`, so D-Bus activation reaches us. `disable`
gives it back. Do not add `Conflicts=fprintd.service`: under the alias that name is our
own unit.

**Known gap:** SELinux and AppArmor label by path, and those labels ship with the distro's
policy package, not with fprintd — so they cannot be borrowed. `/usr/libexec/fprintd-rs`
does not transition into `fprintd_t` and may be denied `/var/lib/fprint`. The fix is for
`/usr/libexec/fprintd` to become an `update-alternatives` link upstream, which is a
proposal to make there, not a thing to work around here.

---

## Provenance & licensing

The project's own crates are **`MIT OR Apache-2.0`**. libfprint is **LGPL-2.1+**; NBIS
(MINDTCT/BOZORTH3) is **US-Government public domain**. The line that matters is not
"copying vs. not" — it is **whose copyright the source carries**:

- **Matching an interface or wire format is allowed and safe.** Enum values, the FP3 magic
  and GVariant type signature `(issbymsmsia{sv}v)`, D-Bus names and status strings, the
  `/var/lib/fprint` layout — these are *interoperability facts*, not copyrightable
  expression. We read upstream source to *document the format* (see `docs/fp3-format.md`)
  and then write **original** Rust from that spec.
- **Porting public-domain code is allowed and safe.** NBIS carries no copyright at all
  (17 USC §105), so following its arithmetic line for line restricts nothing — not the port,
  not the licence on the result. We do exactly this in `fprint-bozorth3` / `fprint-mindtct`,
  deliberately, because bit-exactness against the stock tools demanded it.
- **Transliterating LGPL implementation code is not.** A line-by-line port of, say,
  `fp-print.c`'s logic would be a **derivative work of LGPL-2.1+** and could not be
  MIT/Apache. We do not do this. When we need behavior compatibility *there*, we implement
  originally from the spec/observed bytes and verify **black-box** (round-trip against real
  libfprint), never by copying its expression. This is why the NBIS ports are taken from
  **stock** NBIS and never from libfprint's patched `nbis/` copy: same algorithm, but that
  copy's changes are LGPL, and porting *those* would be the one move that contaminates.

Consequences:

- **NBIS ports** — realized as **`fprint-bozorth3`** (the BOZORTH3 matcher) and **`fprint-mindtct`**
  (the MINDTCT detector). Both are **faithful ports** of **stock upstream public-domain NBIS** (see
  `docs/bozorth3-algorithm.md`, `docs/mindtct-algorithm.md`), never of libfprint's patched `nbis/`
  (its `g_`-prefixing and patches are LGPL), and verified black-box against the stock C tools.
  Following the reference arithmetic — and, for MINDTCT, its scan/removal *ordering* — closely is
  deliberate: bit-exactness demanded it, and a public-domain source permits it.
  **They carry the workspace's `MIT OR Apache-2.0`, not a public-domain marking.** Public domain is
  not a copyleft: NBIS carries no copyright at all, so it restricts neither the port nor the licence
  we put on the result. It grants without demanding — there is nothing to quarantine against. The
  NBIS lineage is **provenance, recorded in docs — not a licence.** Only NIST's own test data (the
  golden fixtures) stays marked public domain, via `REUSE.toml`.
- **The shim** (`fprint-backend-libfprint`) *dynamically links* libfprint. LGPL explicitly
  permits this from any-licensed code (unlike GPL); LGPL obligations attach only to
  distributing the linked whole, not to our source.
- Any code that genuinely *is* derived from libfprint (e.g. if we ever port a specific
  driver by transliteration) must live in a **separate LGPL crate**, isolated from the
  MIT/Apache crystal.

### Per-crate license map

Canonical licence texts live in `LICENSES/` (REUSE-canonical), mechanically mirrored into
each published crate (via `cargo xtask sync-licenses`) so every tarball is self-describing;
`publish-check` verifies each shipped copy is byte-identical to its source. `reuse lint`
gates every file. The crate-level split:

| crate(s) | SPDX license | note |
|---|---|---|
| **every crate in this workspace** | `MIT OR Apache-2.0` | inline SPDX header on every source file — the NBIS ports included: a public-domain source constrains no licence |
| `fprint-backend-libfprint` (shim) | `MIT OR Apache-2.0` (our source) | *dynamically* links libfprint; **binary** redistribution honors LGPL-2.1 §6 — the system `.so` is replaceable, so it is auto-satisfied |
| NIST golden fixtures (test data) | `LicenseRef-NBIS-PD` (public domain) | not code, and not ours: stock-NBIS reference output and NIST imagery, annotated in `REUSE.toml`; text under `LICENSES/` |
| a genuinely libfprint-derived driver, *if ever* | `LGPL-2.1-or-later` | isolated crate; carries its own SPDX header |

One licence across the tree, because there is only one boundary worth policing: **LGPL**. No crate
occupies that row; the slot exists so such a contribution has an obvious, quarantined home and never
contaminates the permissive core. The NBIS ports need no such home — public domain grants without
demanding, so it cannot contaminate anything.

`MIT OR Apache-2.0` is also the choice the project's own ambition dictates: a system daemon lives or
dies by distro packaging, and the dual licence is the Rust ecosystem's default that every distro
accepts without a second thought (Fedora, for one, [disallows CC0 for code](https://lwn.net/Articles/902410/)
because it waives no patent rights — Apache-2.0 grants them). Permissive code can also flow *into*
the GPL projects around us; GPL code could not flow back. Coexistence points the same way beauty does.

*(Not legal advice; the maintainers confirm specifics before release.)*

## Crate map

| crate | role | platform | deps of note |
|---|---|---|---|
| `fprint-core` | domain model + `Backend`/`Device` traits | any | **none** |
| `fprint-fp3` | FP3 print (de)serialization codec (edge translator) | any | **`fprint-core` only** — GVariant hand-rolled |
| `fprint-bozorth3` | BOZORTH3 minutiae matcher (original port from public-domain NBIS) | any | **none** — self-contained arithmetic kernel |
| `fprint-mindtct` | MINDTCT minutiae detector (original port from public-domain NBIS) | any | **none** — self-contained image-processing kernel |
| `fprint-pipeline` | host-image glue (image→minutiae→template→match); the published front door for matching | any | `fprint-core`, `fprint-mindtct`, `fprint-bozorth3` — owns the boundary conversions |
| `fprint-backend-libfprint` | shim owning the C libfprint FFI directly via `libfprint-sys` | Linux | libfprint-2, `!Send` |
| `fprint-backend-native` | virtual device + host-image `Device` (matching via `fprint-pipeline`); an **experimental** USB capture seam behind the `usb` feature | any | `fprint-pipeline`, `fprint-mindtct`; `nusb` *(optional, experimental)* — async is hand-rolled, no runtime dep |
| *integration* (`fprint-integration`) | `CompositeBackend` / `CompositeDevice` | any (native-only off Linux) | both backends, hand-written `match` delegation |
| `fprintd` | `net.reactivated.Fprint` daemon | Linux | `zbus` (+ its `zvariant`), `tokio`, PolicyKit |

Wire formats live only in edge modules, never in `fprint-core`. **FP3** (de)serialization is a
**hand-rolled GVariant codec** in `fprint-fp3` — no serialization crate, so that edge crystal
depends on `fprint-core` alone (its correctness is pinned by frozen golden-byte fixtures). Only the
**D-Bus** side uses `zvariant`, and it comes transitively through `zbus`; the daemon has no
direct `zvariant` dependency.

## Non-goals

- **Reimplementing libfprint's driver estate in Rust.** Reaching parity with its ~28
  hardware drivers is an unbounded, device-dependent axis, and chasing it would turn
  coexistence into a rewrite race. Native drivers are welcome *contributions* through the
  capture seam (`docs/adding-a-driver.md`), never a yardstick for the project.
- **Extending the `net.reactivated.Fprint` contract.** We implement it; we do not add to
  it. The borrowed D-Bus policy enforces this — see §Coexistence.
- **Our own PAM module, D-Bus policy or PolicyKit actions.** They come from the fprintd
  package (§Coexistence).
- A dlopen/plugin ABI for third-party drivers — when a native driver *is* contributed it
  goes in-tree (libfprint's compiled-in model is fine); we don't add a plugin boundary.
- C-ABI drop-in compatibility with `libfprint.so`.
- Windows/macOS runtime support (the daemon targets Linux; `fprint-core` merely compiles
  anywhere).
