# Bringing up a sensor with `fpdev`

`fpdev` (the `fprint-driverkit` crate, `publish = false`) is a workbench for writing a
native driver for a fingerprint sensor libfprint does not support. It covers identifying the
device, scaffolding a driver, decoding its frames, matching them, and opening a pull request.
Every step runs offline and deterministic on recorded bytes except `record` and a live
`shell`, which touch a physical sensor. A maintainer with no hardware reproduces a
contributor's work from the recording.

The seam a driver plugs into — the `FrameSource` trait and the layers on top of it — is in
[Adding a native driver](adding-a-driver.md). The capture formats `import` reads are in
[Re-capturing USB traffic](re-capture-formats.md).

Native drivers are a non-goal; see [ADR 0004](adr/0004-coexistence-shim-first.md). The C
libfprint shim stays the main line for real hardware. `fpdev` turns one recorded session into
a hardware-free regression test.

Each subcommand takes `--help`. Hex ids accept a `0x` prefix or bare digits.

## Pipeline

```
probe → new-driver → shell → import / record → replay → frame → match → doctor → ship
```

## 1. Probe: classify the device

```console
$ fpdev probe --vid 2808 --pid 9338
```

`probe` classifies a `(vid, pid)` against a vendored device database with no device
attached. It reports what libfprint knows and whether the host-image capture seam can reach
the device:

```
fpdev probe · 2808:9338

  device     2808:9338
  driver     (unknown to libfprint)
  family     UNKNOWN
  reach  host-image if it streams grayscale frames,
         otherwise unknown

  next  `fpdev shell --vid 2808 --pid 9338` to poke it
```

The `family` verdict is one of `HOST-IMAGE`, `MATCH-ON-CHIP`, `OTHER`, or `UNKNOWN`, and it
decides `reach`:

- **HOST-IMAGE** — the sensor streams pixels to the host, so `ImageDevice<UsbFrameSource<_>>`
  drives it. Write a `FrameSource` and the rest of the stack is built. `next` points at
  `fpdev new-driver`.
- **MATCH-ON-CHIP** — the sensor matches internally and returns no frame, so the host-image
  seam cannot reach it. Bring-up is a different path; `probe` sends you back to
  [Adding a native driver](adding-a-driver.md).
- **UNKNOWN** — no libfprint driver claims the id. This is the common start for an
  unsupported sensor: probe it with `shell` to learn whether it streams frames.

`--all` lists the whole known database grouped by family; `--json` emits the same verdict as
a structured object.

## 2. New-driver: a working skeleton

```console
$ fpdev new-driver --name acme --vid 2808 --pid 9338 --from vfs5011
```

This scaffolds a five-file host-image driver tree — `mod.rs`, `proto.rs`, `source.rs`,
`<name>.rs`, and `mock_tests.rs` — modeled on the `vfs5011` worked example. The scaffold's
`mock_tests.rs` scripts a reference finger through a scripted transport and drives an
`ImageDevice` to enroll and self-verify, so a freshly generated driver passes its tests from
the first build.

Every device value the scaffold emits — VID/PID, endpoints, frame geometry, init sequences —
carries a `// HW-verified: required` marker: the generated code states interoperability facts
and asserts no byte hardware has not confirmed. Resolving those markers against a physical
sensor is the main bring-up work.

- `--from <driver>` records which worked example the scaffold is modeled on (default
  `vfs5011`).
- `--out <dir>` writes the tree to a directory of your choice instead of the driver location
  under `fprint-backend-native`.
- `--check` re-renders in memory and diffs against the committed golden fixture, writing
  nothing — a template that drifts from its golden fails loudly.
- `--family match-on-chip` is refused: the host-image seam does not reach a match-on-chip
  sensor, so nothing is scaffolded.

## 3. Shell: poke the device

```console
$ fpdev shell --vid 2808 --pid 9338     # live, needs --features usb + hardware
$ fpdev shell --replay session.hex       # offline dry-run over recorded bytes
$ fpdev shell                            # offline dry-run over an empty transport
```

`shell` is a line-oriented REPL for control and bulk transfers over the same transport seam
the drivers speak. Its commands are `control <reqType> <req> <wValue> <wIndex> [hex]`,
`bulkout <ep> <hex>`, `bulkin <ep> <len>`, `help`, and `quit`. Numbers take `0x` hex or
decimal; hex payloads ignore spaces and `:`.

A `bulkin` read is auto-annotated. The bytes are sniffed for the driver's frame-header
layout, hex-dumped, and scored for entropy and printability, so an unknown stream tells you
at a glance whether it looks like an image header or opaque payload:

```
bulkin ep=0x81 len=6
  read 6 bytes
  0000  01 fe 04 00 02 00                                 |......|
  frame header? yes  width=4 height=2  (valid geometry)
```

The sniff is a heuristic that mirrors the backend's documented header shape, not the
authoritative parser. The `--replay` file is a small format of its own: one device-to-host
bulk-in payload as hex per line, replayed in order by successive `bulkin` reads. It is
distinct from a `.cassette` (below). Without the `usb` feature the live path prints that it
is not wired rather than fake a capture.

## 4. Import or record: get a `.cassette`

A `.cassette` is a replayable session of USB transfers. There are two ways to make one.

**Import an existing trace** captured from the vendor stack or libfprint:

```console
$ fpdev import capture.pcapng --vid 2808 --pid 9338
```

`import` reads a pcapng, a classic pcap, a Linux usbmon log (text or binary), or a Windows
USBPcap trace, and folds one device's traffic into a `.cassette`. `--vid`/`--pid` isolate a
device when the trace carries its descriptor; `--bus`/`--addr` select it directly; `-o` sets
the output path; `--format` overrides the `auto` magic sniff. The capture-format layouts are
documented in [Re-capturing USB traffic](re-capture-formats.md).

**Record a live session** from a sensor you hold:

```console
$ fpdev record --vid 2808 --pid 9338 -o acme.cassette
```

`record` opens the device over USB, drives a capture through the same path `replay` reads,
and saves the exchange. It needs the `usb` feature and hardware. This step must run against a
physical sensor. A contributor with the hardware records it; the resulting `.cassette` is what
CI replays.

## 5. Replay: run the real driver, no hardware

```console
$ fpdev replay acme.cassette
```

`replay` lifts the cassette's device-to-host bytes into a scripted transport and runs the
genuine driver over them. Each frame is re-assembled by the driver's own framing, then
measured: MINDTCT reports how many minutiae the frame carries and how reliable they are, and
BOZORTH3 scores the frame against itself.

```
replay: acme.cassette
device: 2808:9338
handshake: replayed
frame 0  256x256  minutiae 17  mean-quality 97.8  self-score 52 (PASS >= 40)
frames: 1 assembled, framing reproduced
```

A clean replay confirms the driver's framing reproduces the recorded pixels. A mismatch names
the frame whose transfer diverged (a truncated payload, a wrong header). The `PASS`/`WEAK`
verdict is the self-score against a threshold of 40.
`--json` emits the per-frame report as a structured object.

## 6. Frame: see the pixels

```console
$ fpdev frame acme.cassette --out frame.png
```

`frame` re-assembles a cassette's frames and writes one PNG each. When the framing is not yet
known, decode a headerless raw buffer with an explicit geometry, or let the detector find it:

```console
$ fpdev frame --raw dump.bin --guess-width --out guess.png
```

`--guess-width` finds the geometry. It enumerates the divisors of the pixel count, assembles a
frame at each candidate width, runs MINDTCT over it, and ranks the candidates by the total
mass of reliable minutiae. The true geometry aligns the ridges into fewer, more reliable
minutiae; a sheared width breaks them into many low-quality false ones:

```
 rank  width  height  minutiae  mean-quality  quality-sum
    1    256     256        17          97.8         1662
    2    512     128        19          77.7         1476
    3    128     512         9          80.1          721
best guess: 256x256 (17 minutiae, quality-sum 1662)
```

With `--out`, it also writes a thumbnail for the top-ranked candidates. The other raw knobs
handle a stream whose bytes are laid out unexpectedly: `--width`/`--height` for a fixed
geometry, `--endian le|be` for 16-bit samples, and `--transpose` for a row/column swap.

## 7. Match: score two captures

```console
$ fpdev match --probe probe.cassette --gallery gallery.cassette --verbose
```

`match` detects minutiae in two captures and scores them through BOZORTH3, printing each
print's `xyt` table, the score, the threshold, and the margin that decides accept or reject.
Either input is a `.cassette` or a frame image (PNG, PGM).

```
  BOZORTH3 score  52
  threshold       40
  margin          52 - 40 = 12  (accept)
```

`--threshold` sets the cutoff (default 40), `--out` writes the two minutiae overlays side by
side, `--json` emits the structured result, and `--verbose` adds a correspondence view — the
compatible-edge count BOZORTH3 clusters over, plus a nearest-neighbour pairing that makes a
low score legible. The pairing ignores the rotation and translation the score tolerates, so
it reads only for prints that already sit in the same frame; it is labelled as such.

## 8. Doctor: is the capture good enough?

```console
$ fpdev doctor acme.cassette
```

`doctor` grades one capture's fitness for detection and suggests what to fix. It prints an
`ok`/`warn` verdict for each metric, then concrete hints:

```
  geometry             256x256  ok   (each side >= 25px)
  dynamic range            226  ok   (>= 40)
  contrast (stdev)        68.0  ok   (>= 20.0)
  minutiae                  17  ok   (>= 8)
  mean reliability        97.8  ok   (>= 25.0)
  foreground              1.00  ok   (>= 0.25)
  exposure (mean)        128.1  ok   (32.0..=223.0)
```

`--ppi`, `--transpose`, and `--frame` are the knobs a bring-up sweeps to find the true
resolution and geometry — MINDTCT's thresholds are resolution-relative, and a transposed
buffer shears ridges into noise. `--tui` drives those same knobs in a full-screen dashboard,
`--json` emits the report, and `--out-overlay` writes the minutiae overlay.

## 9. Ship: open a pull request

```console
$ fpdev ship --driver acme --check      # dry run: print the plan and a PR-body draft
$ fpdev ship --driver acme              # apply the plan
```

`ship` gathers a bring-up's files into pull-request shape. By default it
integrates the driver into `fprint-backend-native` behind the `FrameSource` seam: it places
the scaffold, registers the module, drops the scaffold's `dead_code` allow, notes the device
DB entry to add, installs the capture fixture, and points at the acceptance gate. It closes by
printing a PR-body draft with the driver's provenance, its remaining HW-verified work, and the
acceptance outcome.

A driver ported from LGPL libfprint code cannot enter the permissive core. For that case:

```console
$ fpdev ship --driver acme --isolated-crate --lgpl
```

`--isolated-crate` emits a standalone `publish = false` crate instead of integrating, and
`--lgpl` stamps its `LGPL-2.1-or-later` provenance and isolates it from the permissive stack,
as [Adding a native driver](adding-a-driver.md) §License discipline requires. `--lgpl` without
`--isolated-crate` is refused. `--out` overrides the scaffold's source directory; `--check`
verifies the packaging without writing.

## Maintainer gates

Three `xtask` tasks track a driver toward merge:

- `cargo xtask hw-checklist [driver]` — the burndown of `HW-verified: required` markers, the
  device values still unconfirmed. `--json` for a machine-readable list.
- `cargo xtask driver-check [driver]` — the PR-ready scorecard: the `unsafe` quarantine,
  black-box verification, REUSE cleanliness, workspace lints, the dependency boundary, and
  (for an isolated crate) the registry's publish rules, run as one command. It is the
  [Adding a native driver](adding-a-driver.md) acceptance criteria, mechanized.
- `cargo xtask capture-golden <driver> <recording>` — freeze a recording (a `.cassette` or a
  `.pgm`) as a permanent regression: it lifts the recording into `fprint-backend-native`'s
  fixtures and generates an ordinary `#[test]` that replays it through the detect → match
  pipeline. Plain CI runs that test with no hardware, no nightly toolchain, and no container.

## Where to next

- [Adding a native driver](adding-a-driver.md) — the `FrameSource` seam, the reference
  template, license discipline, and the acceptance criteria in full.
- [Re-capturing USB traffic](re-capture-formats.md) — the capture formats `fpdev import`
  reads, field by field.
