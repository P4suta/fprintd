/*
 * SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
 * SPDX-License-Identifier: MIT OR Apache-2.0
 *
 * Minimal BOZORTH3 oracle driver.
 *
 * Links the stock, public-domain NBIS bozorth3 library sources (bozorth3.c, bz_io.c, bz_sort.c,
 * bz_alloc.c, bz_gbls.c, bz_drvrs.c) and exposes just enough of the CLI's globals to run the REAL
 * loader (bz_load → bz_prune: theta transform, quality trim, (x,y) sort) and the REAL matcher
 * (bozorth_main). It reads a pairs manifest ("tag probe.xyt gallery.xyt" per line) and prints
 * "tag<TAB>score" so the score is produced by exactly the same code path the stock tool uses.
 *
 * This is a verification oracle only; it is compiled and run inside Docker (see run.sh), never
 * shipped. Its output is frozen into crates/fprint-bozorth3/tests/fixtures/expected.tsv.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <bozorth.h>

/* Globals the bozorth3 library declares `extern` (normally defined in the CLI driver). */
int m1_xyt = 0;
int max_minutiae = DEFAULT_BOZORTH_MINUTIAE;      /* 150 */
int min_computable_minutiae = MIN_COMPUTABLE_BOZORTH_MINUTIAE; /* 10 */
int verbose_main = 0;
int verbose_load = 0;
int verbose_bozorth = 0;
int verbose_threshold = 0;
FILE *errorfp;

static int score_pair(const char *probe_path, const char *gallery_path) {
    struct xyt_struct *p = bz_load(probe_path);
    struct xyt_struct *g = bz_load(gallery_path);
    /* bz_load returns NULL for an empty/degenerate file; the CLI scores such a pair 0. */
    if (p == XYT_NULL || g == XYT_NULL) {
        if (p) free(p);
        if (g) free(g);
        return 0;
    }
    set_probe_filename((char *) probe_path);
    set_gallery_filename((char *) gallery_path);
    int s = bozorth_main(p, g);
    free(p);
    free(g);
    return s;
}

int main(int argc, char **argv) {
    errorfp = stderr;
    set_progname(0, "bozorth3-oracle", getpid());

    if (argc < 3) {
        fprintf(stderr, "usage: %s <corpus_dir> <pairs.txt>\n", argv[0]);
        return 2;
    }
    const char *dir = argv[1];
    FILE *fp = fopen(argv[2], "r");
    if (!fp) {
        fprintf(stderr, "cannot open pairs file %s\n", argv[2]);
        return 2;
    }

    char line[1024];
    while (fgets(line, sizeof line, fp)) {
        char tag[256], a[256], b[256];
        if (sscanf(line, "%255s %255s %255s", tag, a, b) != 3)
            continue;
        char pa[600], pb[600];
        snprintf(pa, sizeof pa, "%s/%s", dir, a);
        snprintf(pb, sizeof pb, "%s/%s", dir, b);
        printf("%s\t%d\n", tag, score_pair(pa, pb));
    }
    fclose(fp);
    return 0;
}
