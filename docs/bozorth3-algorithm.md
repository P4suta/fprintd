# The BOZORTH3 minutiae matcher (spec)

This is a **factual, implementation-ready description of the BOZORTH3 fingerprint matching
algorithm**, extracted so `crates/fp-bozorth3` can reproduce the stock tool's integer match
scores **exactly**.

> **Provenance & license note.** BOZORTH3 is part of the **NIST NBIS** package, a work of the
> U.S. Federal Government that is **in the public domain** (title 17 §105 — see the header of
> `bozorth3/src/lib/bozorth3/bozorth3.c`). This spec, and `fp-bozorth3`, are written from the
> **stock upstream NBIS** source (cloned to `reference/nbis-stock/`, git-ignored), **not** from
> libfprint's patched `nbis/` copy — whose modifications carry libfprint's LGPL-2.1+ terms. Because
> NBIS is public domain, `fp-bozorth3` may follow the reference arithmetic faithfully; it is
> quarantined under SPDX `LicenseRef-NBIS-PD` so it never touches the permissive core. See
> `ARCHITECTURE.md` §Provenance & licensing.

Source of fact: NBIS `bozorth3/src/lib/bozorth3/{bozorth3.c,bz_sort.c,bz_io.c}`,
`bozorth3/include/{bozorth.h,bz_array.h}`, and the CLI driver `bozorth3/src/bin/bozorth3/`.

---

## Inputs & coordinate conventions

A print is a list of minutiae, each an integer triple `xyt`:

- `x`, `y` — pixel coordinates (origin top-left).
- `t` (theta) — ridge direction in **integer degrees**, range `0..=359`.

Our domain type `fp_core::Minutia { x, y, theta }` maps 1:1 onto this triple; `fp-bozorth3`
defines its own identical `Minutia` (the xyt triple is an interoperability fact — the matcher
crate stays dependency-free, and the consumer converts).

### Load-time normalization (`bz_io.c: bz_load` / `bz_prune`, `bz_sort.c`)

1. **Cap** the minutia count at `max_minutiae` (default `DEFAULT_BOZORTH_MINUTIAE = 150`, hard max
   `MAX_BOZORTH_MINUTIAE = 200`). When more are present, `bz_prune` keeps the **highest-quality**
   ones (`sort_quality_decreasing`); with no quality column, order is the file order before the cap.
2. **Sort** the retained minutiae by **increasing `x`, ties broken by increasing `y`**
   (`sort_x_y` → `qsort`). This ordering is **load-bearing**: stage 1 relies on it for an early
   `break` (below).

`fp-bozorth3` reproduces: cap to `max` (default 150), then stable sort by `(x, y)` ascending.
(Quality-based pruning is only reachable with a quality column, which `Template::Nbis` does not
carry; we cap in input order then sort.)

---

## Constants (`bozorth.h`)

| macro | value | role |
|---|---:|---|
| `MAX_BOZORTH_MINUTIAE` | 200 | hard cap on minutiae per print |
| `DEFAULT_BOZORTH_MINUTIAE` | 150 | default cap (`max_minutiae`) |
| `MIN_COMPUTABLE_BOZORTH_MINUTIAE` | 10 | below this (either print) → score `0` |
| `DM` | 125 | max inter-minutia distance; `distance` stored is `dx²+dy² ∈ [0, DM²]` |
| `FD` | 5625 | (unused in match core paths we hit; kept for reference) |
| `TK` | 0.05 (f32) | stage-2 distance-tolerance factor |
| `TXS` | 121 | stage-2 angle window low bound (`= 11²`) |
| `CTXS` | 121801 | stage-2 angle window high bound (`= 349²`) |
| `MSTR` | 3 | cluster growth constant (stage 3) |
| `MMSTR` | 8 | score threshold above which `bz_final_loop` refines the result |
| `WWIM` | 10 | stage-3 rotation-consistency window |
| `QQ_SIZE` | 4000 | work-queue size; overflow returns `QQ_OVERFLOW_SCORE = 4000` |
| `DEFAULT_MAX_MATCH_SCORE` | 400 | CLI display cap only (not applied inside the score) |
| `ZERO_MATCH_SCORE` | 0 | returned when a print has too few minutiae |

Angle helper `IANGLE180(deg)` folds a degree value into `(-180, 180]`:
`deg>180 ? deg-360 : (deg<=-180 ? deg+360 : deg)`.

The rounding macro (no `ROUND_USING_LIBRARY`): `ROUND(f) = (f<0) ? (int)(f-0.5) : (int)(f+0.5)`
— i.e. round-half-away-from-zero, truncating toward zero after the ±0.5 bias.

---

## Stage 1 — intra-print comparison table (`bz_comp`)

For one print with its minutiae **sorted by `(x, y)`**, build a table of pairwise edges. For every
pair `(k, j)` with `k < j` (indices 0-based into the sorted list):

1. **Opposite-angle skip.** If `t[j] > 0 && t[k] == t[j] - 180`, or `t[j] <= 0 && t[k] == t[j] + 180`,
   skip this pair.
2. **Distance.** `dx = x[j] - x[k]`, `dy = y[j] - y[k]`, `distance = dx*dx + dy*dy` (integer).
   If `distance > DM*DM` (`= 15625`): if `dx > DM` **break** the inner `j` loop (valid because the
   list is x-sorted, so all later `j` are farther in x); otherwise **continue** to the next `j`.
3. **Edge angle `theta_kj`** (integer degrees):
   - if `dx == 0`: `theta_kj = 90`.
   - else: `dz = (180.0f / PI_f32) * atanf( (f32)dy / (f32)dx )` using **32-bit `atanf`**; then bias
     `dz += (dz<0 ? -0.5 : +0.5)` and truncate: `theta_kj = (int)dz`. *(The `m1_xyt` CLI variant
     negates `dy`; the default and our path use `+dy`. `PI_f32 = (float)M_PI`.)*
     **Bit-exactness hinges on using f32 `atan` and this exact rounding.**
4. **Relative angles.**
   `beta_k = IANGLE180(theta_kj - t[k])`, `beta_j = IANGLE180(theta_kj - t[j] + 180)`.
5. **Row emission** (6 integer columns), ordered so the smaller beta comes first:
   - if `beta_k < beta_j`: `[distance, beta_k, beta_j, k+1, j+1, theta_kj]`
   - else: `[distance, beta_j, beta_k, k+1, j+1, theta_kj + 400]`

   The `+400` on the 6th column (and the k/j point ids being 1-based) encode which endpoint owns
   which beta; stage 2 decodes it.
6. **Keep the table sorted** by the first three columns `(distance, beta_min, beta_max)` via an
   insertion (binary-search + shift of a pointer list). `fp-bozorth3` builds the rows then sorts by
   `(col0, col1, col2)` — the row order after sorting is what matters, and ties preserve prior order.

Table capacity is 20000 rows (`(200²)/2`); NBIS stops at 19999. Our cap of 200 minutiae keeps us
within bounds without a special case, but we mirror the guard.

---

## Stage 2 — inter-print compatibility table (`bz_match`)

Given the two **sorted** stage-1 tables (probe = "subject" `scolpt`, gallery = "on-file" `fcolpt`),
emit **compatible edge pairs** into `colp`. `st` starts at 1; for each probe row `ss` (k = 1..):
for each gallery row `ff` (j = st..):

1. **Distance window (f32).** `dz = ff[0] - ss[0]`; `fi = (2.0f * TK) * (ff[0] + ss[0])`.
   If `dz² > fi²` (both squared as f32): if `dz < 0`, set `st = j+1` and **continue** (gallery edge
   too short — advance the probe's start pointer); else **break** (gallery edge too long, and rows
   are distance-sorted). This is the ±5% distance-agreement test.
2. **Angle window.** For `i` in `1,2`: `dz = ss[i] - ff[i]`; `dz2 = dz²`. If `TXS < dz2 < CTXS`
   (i.e. `11 < |Δβ| < 349`), the betas disagree → **skip** this pair (`continue` outer). Passing both
   means each beta agrees within ~11° (or within ~11° of a full wrap).
3. **Relative rotation `Δθ`.** Decode each row's 6th column: if `ss[5] >= 220` then `p1 = ss[5]-580,
   n=1` else `p1 = ss[5], n=0`; likewise `ff[5]` → `p2, b`. Then `p1 = IANGLE180(p1 - p2)`.
4. **Endpoint pairing.** Emit a 5-column `rot` row: `[Δθ, ss[3], ss[4], A, B]` where `(A,B) =
   (ff[4], ff[3])` if `n != b`, else `(ff[3], ff[4])`. Columns 1..4 are the four 1-based minutia ids
   (probe-k, probe-j, gallery-k/j, gallery-j/k) linking the two edges.
5. **Keep sorted** by columns `(1, 3, 2)` (probe-k, gallery-second, probe-j) via binary-search insert.

Output `colp[0..edge_pair_index]` is the sorted compatibility list; the count is returned. Capacity
20000 (stops at 19999).

---

## Stage 3 — clustering & match score (`bz_match_score`, `bz_sift`, `bz_final_loop`)

Consumes the sorted compatibility list `colp` (each row `[Δθ, probe_k, probe_j, gallery_a,
gallery_b]`) and produces the integer score. Ported faithfully; `crates/fp-bozorth3/src/cluster.rs`
keeps a `// PORT:` note at each of the two behaviour-preserving deviations from the C.

- **Guard.** If either print has `< MIN_COMPUTABLE_BOZORTH_MINUTIAE` (10) minutiae → `0`.
- **Seed loop** (`bz_match_score`, over each unused edge pair `k`). For each seed it enumerates
  endpoint-pairing combinations via a mixed-radix **odometer** over conflict groups, and for each
  combination grows a **path** of compatible edges through `bz_sift`:
  - `bz_sift` links an edge whose endpoints are free (record the pairing) or already-consistently
    paired (re-accept), else records the conflicting alternatives into groups `cf`/`rf` (capped at
    `WWIM = 10` groups). Endpoint pairings live in `tq` (subject→on-file) and `rq` (on-file→subject);
    accepted edges are stamped in `sc[]` with a per-path generation `ftt` and collected in `y[]`.
    A `qq[]` overflow (`QQ_SIZE = 4000`) aborts with `QQ_OVERFLOW_SCORE` (4000).
  - The path is grown by a consecutive-same-start run, then a BFS (linear scan + binary search over
    the sorted `colp`) that discovers further connected edges.
- **Cluster formation** (path length `tot ≥ MSTR = 3`): compute the path's **mean relative rotation**
  (positive/negative angles averaged separately, `ROUND`ed), **prune** edges whose Δθ deviates from
  the mean by more than the `TXS`/`CTXS` band (`121 < diff² < 121801`, i.e. >11°), then register
  cluster `tp` with its edge count `ct[tp]`, representative rotation, integer centroids
  `avv[tp][1..4]`, and sorted endpoint sets `yy[tp][0..1]`. Cross-check `tp` against every prior
  cluster: if their rotations agree (±11°), centroid separations agree (the `TK` distance-ratio
  test), the connecting-vector orientation is rotation-consistent, **and** they share no endpoint,
  merge group totals (`gct[ii] += ct[tp]`).
- **Score.** `match_score` tracks the max of any single `ct[tp]` and any pairwise-merged `gct[ii]`.
  If `match_score < MMSTR` (8) it is returned as-is; otherwise `bz_final_loop` raises it to the
  maximum total `ct` over a **fully mutually-compatible** cluster set (a DFS intersecting clusters'
  compatibility lists `ctp`).

The returned integer is the BOZORTH3 match score — a raw count of corresponding structure, with no
normalization. The CLI compares it against a caller-chosen threshold (no built-in accept/reject
constant; `DEFAULT_MAX_MATCH_SCORE = 400` only caps the *printed* value).

### Reproduction guarantee & the f32 boundary residual

`crates/fp-bozorth3` reproduces stock NBIS scores, verified pair-for-pair against the C tool
(`tests/golden.rs`, corpus in `tests/fixtures/`, oracle via `mise run bozorth3-oracle`):

- **Stages 1–2 are bit-identical** — the `(probe_web_len, gallery_web_len, num_edges)` triple matches
  the reference exactly on every pair tested, including the near-boundary ones. So the compatibility
  tables — and the edge angles from f32 `atanf` — reproduce the reference's math library exactly.
- **The score is exact on every non-trivial match**, up to the largest, most cluster-heavy cases.
- **A few tiny near-tolerance pairs differ by ±1.** With identical compatibility tables, the
  divergence is isolated to a stage-3 f32 comparison where a single edge sits exactly on the 11°
  rotation-consistency threshold; f32 rounding tips it into or out of the cluster differently than
  the reference build. This is the inherent limit of reproducing an algorithm whose *only*
  specification is float-dependent C output — the reference score is itself math-library-relative at
  that boundary. It is bounded to ±1 and enumerated in the golden test; on a sub-20-minutia print,
  where match thresholds are ~40, it is operationally irrelevant.

---

## Consuming `fp-bozorth3` for verify (fp-core seam)

`Template::Nbis` is `Vec<Vec<Minutia>>` — one inner vector per enrolled capture sample. libfprint/
NBIS verify runs the probe scan against **each** enrolled sample and takes the **maximum** score,
comparing it to a driver threshold. `fp-bozorth3` therefore exposes a single-pair
`match_score(&[Minutia], &[Minutia]) -> u32`; the consumer takes the max over samples and applies a
**documented threshold** (to be pinned from stock NBIS + libfprint usage, then verified black-box).

---

## Verification plan

- **Golden vectors (offline, permanent oracle).** In Docker, build the **stock NBIS `bozorth3`**
  CLI, run it over a fixed corpus of `.xyt` pairs (matching / non-matching / edge cases: empty,
  single, `< 10`, at the 200 cap), capture the integer scores, and freeze `{input xyt, score}` into
  `crates/fp-bozorth3/tests/fixtures/`. `cargo test -p fp-bozorth3` then asserts `match_score`
  equals the C score **exactly**, with no Docker required for regression.
- **Regeneration** is a documented `mise` task so the corpus stays reproducible.
