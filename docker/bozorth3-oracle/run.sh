#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Compile the stock-NBIS BOZORTH3 oracle and score the frozen corpus, writing expected.tsv.
# Intended to run inside a gcc container with the repo mounted at /work (see `mise run
# bozorth3-oracle`). Requires reference/nbis-stock (populate via `mise run clone-ref-nbis`).
set -euo pipefail

ROOT="${1:-/work}"
SRC="$ROOT/reference/nbis-stock/bozorth3"
FIX="$ROOT/crates/fprint-bozorth3/tests/fixtures"
BIN="/tmp/bozorth3-oracle"

if [ ! -d "$SRC" ]; then
    echo "error: $SRC not found — clone stock NBIS first (mise run clone-ref-nbis)" >&2
    exit 1
fi

echo "== compiling stock NBIS bozorth3 oracle =="
gcc -O2 -w -I "$SRC/include" -I "$ROOT/reference/nbis-stock/commonnbis/include" \
    "$ROOT/docker/bozorth3-oracle/oracle.c" \
    "$SRC/src/lib/bozorth3/bozorth3.c" \
    "$SRC/src/lib/bozorth3/bz_io.c" \
    "$SRC/src/lib/bozorth3/bz_sort.c" \
    "$SRC/src/lib/bozorth3/bz_alloc.c" \
    "$SRC/src/lib/bozorth3/bz_gbls.c" \
    "$SRC/src/lib/bozorth3/bz_drvrs.c" \
    -lm -o "$BIN"

echo "== scoring corpus =="
"$BIN" "$FIX" "$FIX/pairs.txt" | sort > "$FIX/expected.tsv"

echo "== expected.tsv =="
cat "$FIX/expected.tsv"
echo "== wrote $FIX/expected.tsv =="
