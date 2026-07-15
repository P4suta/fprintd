#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Compile the stock-NBIS MINDTCT oracle and score the synthetic corpus, writing per-image `.xyt`
# minutiae and intermediate maps into the fixtures. Intended to run inside a gcc container with the
# repo mounted at /work (see `mise run mindtct-oracle`). Requires reference/nbis-stock (populate via
# `mise run clone-ref-nbis`).
set -euo pipefail

ROOT="${1:-/work}"
STOCK="$ROOT/reference/nbis-stock"
SRC="$STOCK/mindtct"
FIX="$ROOT/crates/fprint-mindtct/tests/fixtures"
BIN="/tmp/mindtct-oracle"
GENINC="/tmp/mindtct-geninc"

if [ ! -d "$SRC" ]; then
    echo "error: $SRC not found — clone stock NBIS first (mise run clone-ref-nbis)" >&2
    exit 1
fi

mkdir -p "$FIX" "$GENINC"

# The stock build generates an2k.h from an2k.h.src (a plain copy — the .src has no substitutions the
# oracle needs). lfs.h #includes <an2k.h> unconditionally, so every mindtct source needs it present.
cp "$STOCK/an2k/include/an2k.h.src" "$GENINC/an2k.h"

# MINDTCT library sources, minus the three that are only used by the CLI driver and drag in the ANSI/
# NIST (an2k) and Sun-raster image libraries: to_type9.c + update.c (an2k), results.c (sunrast). The
# oracle reimplements the two result writers it needs (dump_map, write_minutiae_XYTQ) inline.
#
# This glob already links the whole detect path — binar.c, detect.c, minutia.c, matchpat.c,
# contour.c, chaincod.c, loop.c, line.c, remove.c, ridges.c, maps.c, init.c, dft.c, ... — because the
# stock get_minutiae()/lfs_detect_minutiae_V2 pipeline pulls in all of them. The oracle's second
# driver (detect_stage_dump) re-calls the same individual functions (binarize_V2, detect_minutiae_V2,
# ...) to emit the per-stage .brwpre / .rmin golden, so no extra sources are needed; the same
# exclusion policy (no to_type9/update/results/an2k) still holds.
LIBSRC=()
for f in "$SRC"/src/lib/mindtct/*.c; do
    base="$(basename "$f")"
    case "$base" in
        to_type9.c|update.c|results.c) continue ;;
    esac
    LIBSRC+=("$f")
done

echo "== compiling stock NBIS mindtct oracle (${#LIBSRC[@]} lib sources) =="
gcc -O2 -w \
    -I "$GENINC" \
    -I "$SRC/include" \
    -I "$STOCK/commonnbis/include" \
    "$ROOT/docker/mindtct-oracle/oracle.c" \
    "${LIBSRC[@]}" \
    -lm -o "$BIN"

echo "== detecting minutiae over corpus (dumping intermediate maps + per-stage .brwpre/.rmin) =="
MINDTCT_DUMP_MAPS=1 "$BIN" "$FIX" "$FIX/manifest.txt"

echo "== fixtures written to $FIX =="
ls -1 "$FIX" | sed 's/^/  /'
