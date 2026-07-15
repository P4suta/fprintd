# The MINDTCT minutiae detector (spec)

This is a **factual, implementation-ready description of the MINDTCT fingerprint minutiae-detection
algorithm**, extracted so `crates/fprint-mindtct` can reproduce the stock tool's `xyt` output
**exactly**. It is a skeleton: the top-level pipeline and the bit-exactness rules are pinned here;
the per-stage detail is filled in alongside the code.

> **Provenance & license note.** MINDTCT is part of the **NIST NBIS** package, a work of the U.S.
> Federal Government that is **in the public domain** (title 17 §105 — see the header of every
> `mindtct/src/lib/mindtct/*.c`). This spec, and `fprint-mindtct`, are written from the **stock upstream
> NBIS** source (`reference/nbis-stock/mindtct/`, git-ignored), **not** from libfprint's patched
> `nbis/mindtct/` copy — whose modifications carry libfprint's LGPL-2.1+ terms. Because NBIS is
> public domain, `fprint-mindtct` **does** follow the reference arithmetic — and its scan/removal
> *ordering* — faithfully, a deliberate choice since bit-exact xyt output demanded it, and it carries
> the workspace's `MIT OR Apache-2.0`: a public-domain source carries no copyright, so it restricts
> neither the port nor the licence on the result. It grants without demanding; there is nothing to
> quarantine against. The NBIS lineage is provenance — this note — not a licence. See
> `ARCHITECTURE.md` §Provenance & licensing.

Source of fact: NBIS `mindtct/src/lib/mindtct/{detect.c,maps.c,dft.c,binar.c,detect,contour.c,
remove.c,ridges.c,quality.c,xytreps.c,imgutil.c,init.c,globals.c}` and `mindtct/include/lfs.h`. The
top-level entry point is `detect.c:lfs_detect_minutiae_V2` (L426).

---

## Inputs & coordinate conventions

The input is an 8-bit grayscale image (row-major, one byte per pixel, `0` = black, `255` = white),
its width and height in pixels, and its scan resolution in ppi. `fprint-mindtct` wraps this as
[`GrayImage`]; several thresholds are resolution-relative.

The output is a list of minutiae in NIST `xyt` form, each an integer 4-tuple:

- `x`, `y` — pixel coordinates, origin **bottom-left** (`x` rightward, `y` upward).
- `theta` — ridge orientation in **integer degrees** on `0..=359`, `0` pointing east, increasing
  counter-clockwise (the `lfs2nist` representation; the M1 variant differs by a 180° offset).
- `quality` — a reliability estimate (higher is better).

Our domain type `fprint_core::Minutia` maps onto this tuple; `fprint-mindtct` defines its own identical
[`Minutia`] (the xyt triple is an interoperability fact — the detector crate stays dependency-free,
and the consumer converts).

---

## Top-level pipeline (`lfs_detect_minutiae_V2`)

The stock `_V2` entry point runs these steps in order (arguments elided):

1. **Initialize lookup tables** — `init_dir2rad`, `init_dftwaves(dft_coefs)`, `init_rotgrids`
   (DFT grids, `RELATIVE2ORIGIN`). `get_max_padding_V2` fixes the pad amount from the window and
   dir-bin grid sizes.
2. **Pad** the input with `pad_uchar_image` (fill value `PAD_VALUE = 128`), producing `pdata` at
   `pw × ph`. If no padding is needed, `pdata` is a plain copy.
3. **6-bit rescale** — `bits_8to6(pdata)`: every pixel `>>= 2`, mapping `[0,256)` → `[0,64)`. The DFT
   power accumulation is tuned for this 6-bit range.
4. **Block maps** — `gen_image_maps` → `direction_map`, `low_contrast_map`, `low_flow_map`,
   `high_curve_map`, each `mw × mh` blocks. (DFT direction analysis, contrast, ridge-flow, curvature,
   plus interpolation and smoothing.)
5. **Binarize** — `init_rotgrids` again for the dir-bin grids (`RELATIVE2CENTER`), then `binarize_V2`
   → `bdata` (`bw × bh`, `0` = ridge / `255` = valley). The binary image must match the *input*
   dimensions (`iw × ih`), else error `-581`.
6. **Detect** — `gray2bin(1,1,0,…)` maps the image to `{0,1}`; `alloc_minutiae(MAX_MINUTIAE)`;
   `detect_minutiae_V2` scans contours and pattern-matches ridge endings / bifurcations.
7. **Remove false minutiae** — `remove_false_minutia_V2` (hooks, islands, loops, overlaps,
   malformations, pores).
8. **Ridge counts** — `count_minutiae_ridges` fills each minutia's neighbour ridge counts.
9. **Wrap up** — `gray2bin(1,255,0,…)` restores the binary image to `{0,255}`; the maps, binary
   image, dimensions, and minutiae list are returned.

`fprint-mindtct::detect_minutiae` reproduces steps 2–9 and then applies the `xyt` conversion below;
`debug_maps` exposes the step-4/5 intermediates for verification.

---

## Bit-exactness

Reproducing the stock `xyt` output to the integer requires reproducing the reference's rounding,
precision, and ordering **verbatim**. The rules (see `crates/fprint-mindtct/src/num.rs`):

- **`sround` = round-half-away-from-zero.** `lfs.h`: `sround(x) = (int)((x<0) ? x-0.5 : x+0.5)`,
  i.e. bias by `copysign(0.5, x)` then truncate toward zero: `(x + copysign(0.5, x)).trunc() as i32`.
- **`trunc_dbl_precision(x, 16384.0)` = 1/16384 quantization.** `lfs.h`: for `scale = TRUNC_SCALE =
  16384.0`, `(x<0) ? (int)(x*scale - 0.5)/scale : (int)(x*scale + 0.5)/scale`. Used to strip
  low-order float noise so comparisons are stable across platforms.
- **`f64` throughout.** All arithmetic is double precision. The **sole `f32` exception** is
  `xytreps`'s `degrees_per_unit = 180 / (float)NUM_DIRECTIONS` — it must stay `f32` to match.
- **6-bit pixels.** `bits_8to6` is exactly `>> 2` (integer divide by 4), not a scale-and-round.
- **Stable bubble sort.** Every sort the stock relies on is an in-place bubble sort with a strict
  `<` comparison and **no swap on equality** — order-preserving on ties. Reproduce the comparator
  and the tie behaviour, not just the final key order; several stages are **order-dependent**.
- **DFT `cos`/`sin` (a libm risk).** `dft.c` fills the waveform tables with `cos`/`sin` **without**
  `trunc_dbl_precision`. Different libm implementations can differ in the last ULP, which can flip a
  downstream `sround`. This is the main cross-platform hazard; the golden oracle pins one libm and
  any divergence is characterized there (mirroring the `fprint-bozorth3` ±1 discipline).

---

## Parameters (`lfsparms_V2`, from `lfs.h` / `globals.c`)

`globals.c` assembles `lfsparms_V2` from the following `#define`s (skeleton — the full table is
filled in with the code):

| macro | value | role |
|---|---:|---|
| `PAD_VALUE` | 128 | fill value for image padding (medium gray) |
| `JOIN_LINE_RADIUS` | 1 | join-line radius |
| `MAP_BLOCKSIZE_V2` | 8 | block size (px) for the image maps |
| `MAP_WINDOWSIZE_V2` | 24 | analysis window size (px) |
| `MAP_WINDOWOFFSET_V2` | 8 | window offset (px) |
| `NUM_DIRECTIONS` | 16 | number of quantized ridge directions |
| `START_DIR_ANGLE` | `M_PI/2` | starting direction angle (90°) |
| `NUM_DFT_WAVES` | 4 | DFT waveforms used in direction analysis |
| `POWMAX_MIN` | 100000.0 | min max-power for a valid ridge-flow block |
| `POWNORM_MIN` | 3.8 | min normalized power |
| `RMV_VALID_NBR_MIN` | 3 | min valid neighbours to keep a block direction |
| `DIRBIN_GRID_W` | 7 | dir-bin rotated grid width |
| `DIRBIN_GRID_H` | 9 | dir-bin rotated grid height |
| `MAX_MINUTIA_DELTA` | 10 | max minutia direction delta |
| `MAX_HIGH_CURVE_THETA` | `M_PI/3` | high-curvature angle threshold (60°) |
| `HIGH_CURVE_HALF_CONTOUR` | 14 | half-contour length for curvature test |
| `MIN_LOOP_LEN` | 20 | min loop length |
| `MAXTRANS` | 2 | max transitions (overlap removal) |
| `MAX_NBRS` | 5 | neighbours per minutia for ridge counting |
| `MAX_RIDGE_STEPS` | 10 | max ridge-count trace steps |
| `MAX_MINUTIAE` | 1000 | detection-list capacity (`lfs.h`) |
| `TRUNC_SCALE` | 16384.0 | quantization scale for `trunc_dbl_precision` |

---

## Coordinate / theta conversion to `xyt` (`xytreps.c`)

The detector works in an "LFS native" representation (origin top-left, direction as a `0..NUM_DIRECTIONS`
index); the `xyt` output is produced by `lfs2nist_minutia_XYT` (with an M1 variant):

```
degrees_per_unit = 180 / (float)NUM_DIRECTIONS;          // f32 — the one f32 in the port
x = minutia->x;
y = ih - minutia->y;                                     // flip origin to bottom-left
t = (270 - sround(minutia->direction * degrees_per_unit)) % 360;
if (t < 0) t += 360;                                     // fold into 0..=359
```

The **M1 mode** (`lfs2m1_minutia_XYT`) is identical except `t = (90 - sround(…)) % 360` — a 180°
offset (M1 points *into* the ridge, NIST points *out*). `fprint-mindtct` emits the `lfs2nist` form by
default; the M1 offset is a documented switch at the `xyt` seam.

---

## Verification plan

- **Golden vectors (offline, permanent oracle).** In Docker, build the **stock NBIS `mindtct`** CLI,
  run it over a fixed corpus of grayscale images, capture the `.xyt` output (and, with `-m1`/map
  dumps, the intermediate maps), and freeze `{input image, xyt, maps}` into
  `crates/fprint-mindtct/tests/fixtures/`. `cargo test -p fprint-mindtct` then asserts `detect_minutiae`
  (and `debug_maps`) reproduce the C output; the stage-by-stage map dumps localize any divergence.
- **Regeneration** is a documented `mise` task so the corpus stays reproducible.
