# SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Deterministic BOZORTH3 test-corpus generator.
#
# Writes a set of `.xyt` minutiae files (3-column "x y theta", canonical theta in 0..359, unique
# (x,y) per print, <= 150 minutiae) plus a `pairs.txt` manifest of "name probe.xyt gallery.xyt".
# The stock NBIS `bozorth3` oracle (Docker) and our Rust matcher both read these exact files, so the
# frozen scores become a permanent cross-implementation oracle. No RNG library: a tiny LCG keeps the
# corpus byte-reproducible on any machine.

import math
import os
import sys

OUT = sys.argv[1] if len(sys.argv) > 1 else "corpus"
os.makedirs(OUT, exist_ok=True)


class Lcg:
    def __init__(self, seed):
        self.s = (seed * 0x9E3779B97F4A7C15 + 1) & 0xFFFFFFFFFFFFFFFF

    def next(self):
        self.s = (self.s * 6364136223846793005 + 1442695040888963407) & 0xFFFFFFFFFFFFFFFF
        return self.s >> 33


def make_print(n, seed):
    """A print of n minutiae with unique (x,y), coords in [40,460), theta in 0..359."""
    r = Lcg(seed)
    pts = []
    used = set()
    guard = 0
    while len(pts) < n and guard < n * 50:
        guard += 1
        x = 40 + r.next() % 420
        y = 40 + r.next() % 420
        if (x, y) in used:
            continue
        used.add((x, y))
        t = r.next() % 360
        pts.append((x, y, t))
    return pts


def jitter(pts, seed, dpos, dtheta):
    """Small per-minutia noise (simulates recapture of the same finger)."""
    r = Lcg(seed)
    out = []
    used = set()
    for (x, y, t) in pts:
        nx = x + (r.next() % (2 * dpos + 1)) - dpos
        ny = y + (r.next() % (2 * dpos + 1)) - dpos
        nt = (t + (r.next() % (2 * dtheta + 1)) - dtheta) % 360
        if (nx, ny) in used:
            nx += 1
        used.add((nx, ny))
        out.append((nx, ny, nt))
    return out


def rigid(pts, deg, tx, ty):
    """Rotate about the centroid by `deg` degrees and translate — a genuine rigid re-placement.

    theta advances by `deg` too; BOZORTH3 is rotation/translation invariant so a rigid copy should
    still match strongly."""
    if not pts:
        return []
    cx = sum(p[0] for p in pts) / len(pts)
    cy = sum(p[1] for p in pts) / len(pts)
    rad = math.radians(deg)
    c, s = math.cos(rad), math.sin(rad)
    out = []
    used = set()
    for (x, y, t) in pts:
        rx = (x - cx) * c - (y - cy) * s + cx + tx
        ry = (x - cx) * s + (y - cy) * c + cy + ty
        nx, ny = int(round(rx)), int(round(ry))
        while (nx, ny) in used:
            nx += 1
        used.add((nx, ny))
        nt = (t + deg) % 360
        out.append((nx, ny, nt))
    return out


def drop_some(pts, seed, k):
    """Remove k minutiae (simulates a partial capture)."""
    r = Lcg(seed)
    pts = list(pts)
    for _ in range(min(k, max(0, len(pts) - 10))):
        if pts:
            del pts[r.next() % len(pts)]
    return pts


def write_xyt(name, pts):
    path = os.path.join(OUT, name)
    with open(path, "w", newline="\n") as f:
        for (x, y, t) in pts:
            f.write(f"{x} {y} {t}\n")
    return name


pairs = []


def emit(tag, a, b):
    fa = write_xyt(f"{tag}_a.xyt", a)
    fb = write_xyt(f"{tag}_b.xyt", b)
    pairs.append((tag, fa, fb))


# Base prints of assorted sizes, with two independent seeds each to broaden coverage.
bases = {}
for n in (10, 12, 20, 35, 60, 100, 150):
    for si, seedtag in enumerate(("s1", "s2")):
        bases[f"{n}{seedtag}"] = (n, make_print(n, 1000 * n + 7777 * si + 13))

# Odd rotation angles stress the f32 atanf path across all quadrants — the single most
# bit-exactness-sensitive operation. Boundary jitter probes the ±11°/~5% tolerance edges.
for key, (n, p) in bases.items():
    emit(f"self_{key}", p, p)
    emit(f"jit_{key}", p, jitter(p, 7 * n, 2, 2))
    emit(f"jitedge_{key}", p, jitter(p, 3 * n, 4, 9))       # near the tolerance boundary
    emit(f"partial_{key}", p, drop_some(jitter(p, n, 2, 2), n, n // 4))
    for deg in (15, 37, 90, 123, 200, 271, 359):
        emit(f"rot{deg}_{key}", p, rigid(p, deg, 12, -8))

# Cross (unrelated) pairs across differing sizes and seeds.
keys = list(bases.keys())
for i in range(len(keys)):
    (na, pa) = bases[keys[i]]
    (nb, pb) = bases[keys[(i + 5) % len(keys)]]
    emit(f"cross_{keys[i]}_{keys[(i + 5) % len(keys)]}", pa, pb)

# Degenerate / edge cases.
emit("tiny_5", make_print(5, 3), make_print(5, 3))        # < 10 → 0
emit("tiny_9_vs_20", make_print(9, 11), make_print(20, 11))  # one side < 10 → 0
emit("empty", [], [])                                      # empty → 0

with open(os.path.join(OUT, "pairs.txt"), "w", newline="\n") as f:
    for (tag, fa, fb) in pairs:
        f.write(f"{tag} {fa} {fb}\n")

print(f"wrote {len(pairs)} pairs to {OUT}/")
