# SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Deterministic MINDTCT test-corpus generator.
#
# Writes a set of headerless raw 8-bit grayscale "fingerprint" images plus, per image, a
# `<name>.manifest` sidecar ("width height ppi") and a corpus-level `manifest.txt` index listing
# one image basename per line. The stock NBIS `mindtct` oracle (Docker, see oracle.c/run.sh) reads
# these exact bytes and freezes its minutiae (`.xyt`) and intermediate maps into the fixtures, so the
# outputs become a permanent cross-implementation oracle for the pure-Rust port.
#
# The images are synthetic: a sinusoidal ridge grating with gentle curvature and (optionally) a
# whorl/core singularity, plus additive LCG noise. No RNG library — a tiny LCG (same idiom as the
# bozorth3 corpus generator) keeps every byte reproducible on any machine. The set deliberately spans
# several sizes/seeds and the edge cases MINDTCT's maps hinge on: a uniform field (all low-contrast),
# a pure linear gradient (single deterministic direction, no ridge flow), a small image (padding /
# block-boundary behaviour), and a high-curvature whorl.

import math
import os
import sys

OUT = sys.argv[1] if len(sys.argv) > 1 else "corpus"
os.makedirs(OUT, exist_ok=True)

PPI = 500  # sensor resolution recorded in every sidecar (mindtct maps pixels -> mm via this)


class Lcg:
    def __init__(self, seed):
        self.s = (seed * 0x9E3779B97F4A7C15 + 1) & 0xFFFFFFFFFFFFFFFF

    def next(self):
        self.s = (self.s * 6364136223846793005 + 1442695040888963407) & 0xFFFFFFFFFFFFFFFF
        return self.s >> 33


def clamp8(v):
    """Round-to-nearest into the 8-bit range (v is non-negative in practice)."""
    iv = int(v + 0.5)
    if iv < 0:
        return 0
    if iv > 255:
        return 255
    return iv


def dislocations(w, h, seed, period, count):
    """Pick `count` ridge-dislocation dipoles inside the image. Each dipole is a +1/-1 pair of phase
    singularities spaced a few pixels apart; overlaid on a grating (see `grating`) a dipole makes one
    extra ridge begin and end locally — i.e. it plants a ridge-ending / bifurcation minutia in an
    otherwise coherent ridge flow, which is exactly what MINDTCT is built to find. Positions come from
    the LCG so the placement is byte-reproducible."""
    r = Lcg(seed ^ 0x0D15104A)
    margin = int(2 * period) + 10
    sings = []
    if w <= 2 * margin or h <= 2 * margin:
        return sings
    # A dipole separation of about one ridge period inserts a single resolvable half-ridge (a clean
    # ending/bifurcation); much tighter and the +/- windings cancel before the detector can see them.
    sep = max(4, int(round(period)))
    for _ in range(count):
        sx = margin + r.next() % (w - 2 * margin)
        sy = margin + r.next() % (h - 2 * margin)
        ox = sep + r.next() % 3
        oy = (r.next() % 3) - 1
        sings.append((sx, sy, 1.0))
        sings.append((sx + ox, sy + oy, -1.0))
    return sings


def grating(w, h, seed, period, angle_deg, curve, whorl_k, noise, amp=95.0, dc=128.0, disloc=0):
    """A ridge field: parallel sinusoidal ridges at `angle_deg`, bent by a quadratic `curve`, with an
    optional `whorl_k`-turn spiral singularity at the image centre and `disloc` scattered ridge
    dislocations, plus additive LCG noise.

    period    - ridge spacing in pixels (fingerprint ridges are ~9 px at 500 ppi)
    curve     - quadratic bend of the ridge normal (0 = perfectly straight ridges)
    whorl_k   - spiral winding number about the centre (0 = none; >0 = high-curvature core)
    disloc    - number of ridge-dislocation dipoles to scatter (seeds ridge-ending/bifurcation minutiae)
    noise     - additive noise half-range in gray levels
    """
    r = Lcg(seed)
    cx, cy = w / 2.0, h / 2.0
    ca, sa = math.cos(math.radians(angle_deg)), math.sin(math.radians(angle_deg))
    kf = 2.0 * math.pi / period
    sings = dislocations(w, h, seed, period, disloc) if disloc else []
    data = bytearray(w * h)
    idx = 0
    for y in range(h):
        for x in range(w):
            dx = x - cx
            dy = y - cy
            # Coordinate along the ridge normal, bent by the quadratic curvature term.
            u = dx * ca + dy * sa
            v = -dx * sa + dy * ca
            u += curve * v * v
            phase = kf * u
            if whorl_k:
                phase += whorl_k * math.atan2(dy, dx)
            for (sx, sy, ch) in sings:
                phase += ch * math.atan2(y - sy, x - sx)
            val = dc + amp * math.cos(phase)
            if noise:
                val += (r.next() % (2 * noise + 1)) - noise
            data[idx] = clamp8(val)
            idx += 1
    return data


def uniform(w, h, level=128):
    """A flat field: every block reads as low-contrast (no ridge structure at all)."""
    return bytearray([level & 0xFF]) * (w * h)


def gradient(w, h):
    """A pure horizontal ramp: a single, globally deterministic ridge-flow direction and no
    oscillation — probes the low-flow / direction machinery at an extreme."""
    data = bytearray(w * h)
    idx = 0
    for _y in range(h):
        for x in range(w):
            data[idx] = clamp8(x * 255.0 / (w - 1))
            idx += 1
    return data


def write_image(name, w, h, data):
    assert len(data) == w * h, (name, len(data), w, h)
    with open(os.path.join(OUT, f"{name}.raw"), "wb") as f:
        f.write(bytes(data))
    with open(os.path.join(OUT, f"{name}.manifest"), "w", newline="\n") as f:
        f.write(f"{w} {h} {PPI}\n")
    return name


names = []


def emit(name, w, h, data):
    names.append(write_image(name, w, h, data))


# Assorted plain ridge fields: several sizes, two seeds each, differing ridge angle / spacing /
# gentle curvature. These are the "typical" prints that yield a healthy minutiae set.
for (w, h) in ((160, 160), (192, 224), (256, 256)):
    for si, seed in enumerate((0x1111, 0x2222)):
        emit(
            f"grating_{w}x{h}_s{si + 1}",
            w, h,
            grating(w, h, seed, period=9 + si, angle_deg=20 + 35 * si,
                    curve=0.0018 * (si + 1), whorl_k=0, noise=18, disloc=10 + 4 * si),
        )

# Loop-ish print: stronger curvature bends the ridges into an arch/loop (moderate curvature map).
emit("loop_200x240", 200, 240, grating(200, 240, 0x33AA, period=10, angle_deg=8,
                                        curve=0.0060, whorl_k=0, noise=16, disloc=14))

# High-curvature whorls: a multi-turn spiral core drives the high-curvature map hard. Two seeds.
emit("whorl_208x208_s1", 208, 208, grating(208, 208, 0x5151, period=9, angle_deg=0,
                                            curve=0.0, whorl_k=3, noise=16, disloc=8))
emit("whorl_208x208_s2", 208, 208, grating(208, 208, 0x6262, period=11, angle_deg=0,
                                            curve=0.0, whorl_k=5, noise=20, disloc=8))

# --- Edge cases -------------------------------------------------------------------------------
emit("uniform_128x128", 128, 128, uniform(128, 128, 128))   # all low-contrast
emit("gradient_128x128", 128, 128, gradient(128, 128))      # single deterministic direction
emit("small_56x56", 56, 56, grating(56, 56, 0x7777, period=8, angle_deg=45,
                                     curve=0.0, whorl_k=0, noise=14))  # padding / block boundary
emit("small_72x64", 72, 64, grating(72, 64, 0x8888, period=9, angle_deg=63,
                                     curve=0.0030, whorl_k=0, noise=16, disloc=3))

with open(os.path.join(OUT, "manifest.txt"), "w", newline="\n") as f:
    for name in names:
        f.write(f"{name}\n")

print(f"wrote {len(names)} images (+ manifests) to {OUT}/")
