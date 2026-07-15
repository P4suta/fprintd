# The FP3 print serialization format (spec)

This is a **factual description of the on-disk fingerprint-template format** libfprint
reads and writes, extracted so we can interoperate with existing `/var/lib/fprint` stores
and any `FpPrint` serialized by libfprint/fprintd.

> **Provenance & license note.** These are *interoperability facts* — a byte layout and a
> GVariant type signature you must match to be compatible — not copyrightable expression.
> The facts were read from upstream `libfprint/fp-print.c` (`fp_print_serialize` /
> `fp_print_deserialize`, LGPL-2.1+) purely to document the format. Our codec
> (`crates/fprint-fp3`) is written **originally from this spec**, not transliterated from the C
> source. See `ARCHITECTURE.md` §Provenance & Licensing.

## Container

```
 byte 0..3   ASCII magic  "FP3"           (0x46 0x50 0x33)
 byte 3..N   GVariant-serialized value, type (issbymsmsia{sv}v),
             in GVariant *normal form*, little-endian.
```

- On big-endian hosts libfprint byteswaps to little-endian before storing, and byteswaps
  back on load — so **the on-disk encoding is always little-endian normal-form GVariant**.
  (`fp-print.c`: `g_variant_byteswap` under `G_BIG_ENDIAN`; load path uses
  `g_variant_get_normal_form` on LE.)

## Top-level tuple `(issbymsmsia{sv}v)`

| # | GVariant | field | meaning |
|---|---|---|---|
| 0 | `i` | type | `FpiPrintType`: **0=UNDEFINED, 1=RAW, 2=NBIS** |
| 1 | `s` | driver | driver id the template is bound to |
| 2 | `s` | device_id | device id |
| 3 | `b` | device_stored | true ⇒ this print is only a handle to an on-sensor template (MOC) |
| 4 | `y` | finger | `FpFinger` byte (see `fprint-core::Finger`: 0=unknown, 1..=10) |
| 5 | `ms` | username | maybe-string |
| 6 | `ms` | description | maybe-string |
| 7 | `i` | enroll_date | GLib **Julian day** (`g_date_get_julian`), or `G_MININT32` (= `i32::MIN`) if unset/invalid |
| 8 | `a{sv}` | reserved | always the **empty** vardict; reserved for future expansion |
| 9 | `v` | payload | biometric payload, structure depends on `type` (below) |

## Payload variant (`v`, field 9)

**NBIS (type = 2).** The variant holds type `(a(aiaiai))` — a tuple with a single field,
an array of samples; each sample is `(aiaiai)` = three **equal-length** `int32` arrays
`(xcol, ycol, thetacol)`, one sample per enrolled capture. Maps to
`fprint-core::Template::Nbis(Vec<Vec<Minutia>>)` where each inner `Vec<Minutia>` is one sample
and a `Minutia { x, y, theta }` is one index across the three arrays.

**RAW (type = 1).** The variant holds the driver's own opaque GVariant (`print->data`) —
an arbitrary type chosen per driver. For match-on-chip prints this is typically a small
handle. Byte-compatible round-tripping must preserve the exact inner variant (type
signature + data), so `fprint-core::Template::Raw` carries it as the opaque serialized variant.

**UNDEFINED (type = 0).** Not serialized in practice (a print before enrollment).

## Related: MOC user-id string (not FP3)

Separately, devices that can only store a short id string alongside an on-chip template use
`fpi_print_generate_user_id`:

```
FP1-YYYYMMDD-<finger-hex>-<random32-hex>-<username>
```
(`fp-print.c` / `fpi-print.c`). This is a different, ASCII, format — handled by its own
edge helper, not the FP3 codec.

## Verification plan

- **Self-consistency (offline, now):** `Print → encode → decode → Print` round-trips in
  `crates/fprint-fp3` tests.
- **Byte-compatibility (needs Linux/libfprint):** capture real `FP3` blobs (enroll via the
  `virtual-image` driver, or read `/var/lib/fprint` fixtures), then assert our decode
  matches and our re-encode is byte-identical. Tracked as an M2 fixture task.

## Source-of-fact references (upstream libfprint, for the format only)

- `libfprint/fp-print.c` — `FPI_PRINT_VARIANT_TYPE` (`(issbymsmsia{sv}v)`), `fp_print_serialize`,
  `fp_print_deserialize`, `"FP3"` magic, `G_MININT32` sentinel, `(a(aiaiai))` NBIS payload.
- `libfprint/fpi-print.h` — `FpiPrintType { UNDEFINED=0, RAW, NBIS }`.
