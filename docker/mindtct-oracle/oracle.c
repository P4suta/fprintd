/*
 * SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
 * SPDX-License-Identifier: MIT OR Apache-2.0
 *
 * Minimal MINDTCT oracle driver.
 *
 * Links the stock, public-domain NBIS mindtct library sources (detect.c, getmin.c, maps.c, ...) and
 * runs the REAL detection pipeline on synthetic raw grayscale images. For each image named in a
 * `manifest.txt` index it calls get_minutiae() (getmin.c -> lfs_detect_minutiae_V2, detect.c) and
 * writes:
 *
 *   <name>.xyt   the final minutiae in NIST-internal representation, exactly as the stock
 *                write_minutiae_XYTQ() (results.c) formats them ("x y theta quality" per line).
 *
 * and, when MINDTCT_DUMP_MAPS is set in the environment, the intermediate results the Rust port must
 * reproduce block-for-block:
 *
 *   <name>.dm    direction map        (block integers, stock dump_map() layout)
 *   <name>.lcm   low-contrast map
 *   <name>.lfm   low-flow map
 *   <name>.hcm   high-curvature map
 *   <name>.brw   binarized image, headerless raw bw*bh bytes (bdata from the pipeline)
 *
 * The get_minutiae() pipeline captures its binary image (.brw) only at the very END of
 * lfs_detect_minutiae_V2, i.e. AFTER detect_minutiae_V2 + remove_false_minutia_V2 have run and
 * removal has edited bdata in place. To supply per-STAGE golden for the earlier stages the port must
 * reproduce, a second driver (detect_stage_dump) re-runs the stock individual functions from
 * detect.c's lfs_detect_minutiae_V2 body verbatim (init_dir2rad / init_dftwaves / init_rotgrids /
 * get_max_padding_V2 -> pad_uchar_image -> bits_8to6 -> gen_image_maps -> init_rotgrids(dirbin) ->
 * binarize_V2 -> [dump .brwpre] -> gray2bin(1,1,0) -> detect_minutiae_V2 -> [dump .rmin] ->
 * remove_false_minutia_V2 -> [dump .rmin2]), stopping BEFORE count_minutiae_ridges, and writes:
 *
 *   <name>.brwpre  binarize_V2 output (remove-free), headerless raw iw*ih bytes captured BEFORE the
 *                  gray2bin(1,1,0) collapse — i.e. the stock grayscale binary image 0=ridge(black),
 *                  255=valley(white), at the ORIGINAL image size/origin (bw==iw, bh==ih). This is the
 *                  pure directional-binarization stage, so it equals the port's DebugMaps.binarized
 *                  byte-for-byte on EVERY image (unlike .brw, which removal contaminates).
 *
 *   <name>.rmin    detect_minutiae_V2 output captured BEFORE remove_false_minutia_V2: the raw minutia
 *                  list, one minutia per line preserving list order, each line five space-separated
 *                  ints "x y direction type appearing":
 *                    x, y        - lfs-internal pixel coords, top-left origin (MINUTIA.x / .y)
 *                    direction   - lfs-internal integer direction 0..(2*ndirs-1)=0..31 (MINUTIA.direction)
 *                    type        - stock MINUTIA.type: 0=BIFURCATION, 1=RIDGE_ENDING (lfs.h)
 *                    appearing   - MINUTIA.appearing: 0=DISAPPEARING, 1=APPEARING
 *                  A header line holds the minutia count (minutiae->num) alone.
 *
 *   <name>.rmin2   remove_false_minutia_V2 output captured immediately AFTER the full 10-step
 *                  false-minutia pruning and BEFORE count_minutiae_ridges: the pruned minutia list.
 *                  Identical contract/format to .rmin (header count, then "x y direction type
 *                  appearing" per line, list order preserved). Count is <= .rmin (removal only
 *                  prunes; step 6 may adjust a surviving minutia's position/direction in place).
 *
 * A `<name>.manifest` sidecar supplies "width height ppi" for each image. This is a verification
 * oracle only; it is compiled and run inside Docker (see run.sh), never shipped. Its output is frozen
 * into crates/fprint-mindtct/tests/fixtures by a later phase.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <lfs.h>

/* MM_PER_INCH lives in an2k.h; keep the oracle self-contained instead of pulling that header in. */
#define MM_PER_INCH 25.4

/* Slurp an entire file into a freshly malloc'd buffer; *len receives the byte count. */
static unsigned char *read_file(const char *path, int *len) {
    FILE *fp = fopen(path, "rb");
    if (!fp)
        return NULL;
    fseek(fp, 0, SEEK_END);
    long n = ftell(fp);
    fseek(fp, 0, SEEK_SET);
    if (n < 0) {
        fclose(fp);
        return NULL;
    }
    unsigned char *buf = (unsigned char *) malloc((size_t) n + 1);
    if (!buf) {
        fclose(fp);
        return NULL;
    }
    size_t got = fread(buf, 1, (size_t) n, fp);
    fclose(fp);
    if (got != (size_t) n) {
        free(buf);
        return NULL;
    }
    buf[n] = 0;
    *len = (int) n;
    return buf;
}

/* Dump an int block-map exactly like stock results.c:dump_map() ("%2d " per cell, newline per row). */
static void dump_map_oracle(const char *path, const int *map, int mw, int mh) {
    FILE *fp = fopen(path, "wb");
    if (!fp) {
        fprintf(stderr, "ERROR: cannot write %s\n", path);
        return;
    }
    const int *p = map;
    for (int my = 0; my < mh; my++) {
        for (int mx = 0; mx < mw; mx++)
            fprintf(fp, "%2d ", *p++);
        fprintf(fp, "\n");
    }
    fclose(fp);
}

/* Write the NIST-internal .xyt exactly as stock results.c:write_minutiae_XYTQ() would. */
static void write_xyt(const char *path, const MINUTIAE *minutiae, int iw, int ih) {
    FILE *fp = fopen(path, "wb");
    if (!fp) {
        fprintf(stderr, "ERROR: cannot write %s\n", path);
        return;
    }
    for (int i = 0; i < minutiae->num; i++) {
        MINUTIA *m = minutiae->list[i];
        int ox, oy, ot, oq;
        lfs2nist_minutia_XYT(&ox, &oy, &ot, m, iw, ih);
        oq = sround(m->reliability * 100.0);
        fprintf(fp, "%d %d %d %d\n", ox, oy, ot, oq);
    }
    fclose(fp);
}

/*
 * Re-run the stock lfs_detect_minutiae_V2 body from detect.c using the individual public lfs.h
 * functions, stopping right before remove_false_minutia_V2, to capture the per-STAGE golden the
 * end-to-end get_minutiae() pipeline cannot expose:
 *   .brwpre  binarize_V2 output (remove-free, pre-gray2bin, original image size)
 *   .rmin    detect_minutiae_V2 raw minutia list (pre-removal, list order preserved)
 * Mirrors detect.c:lfs_detect_minutiae_V2 verbatim in call order and arguments (num_directions=16 =>
 * directions 0..31). Returns 0 on success, non-zero on any stock-function error.
 */
static int detect_stage_dump(const char *dir, const char *name,
                             unsigned char *idata, int iw, int ih) {
    char path[1024];
    const LFSPARMS *lfsparms = &lfsparms_V2;

    unsigned char *pdata, *bdata;
    int pw, ph, bw, bh;
    DIR2RAD *dir2rad;
    DFTWAVES *dftwaves;
    ROTGRIDS *dftgrids, *dirbingrids;
    int *direction_map, *low_contrast_map, *low_flow_map, *high_curve_map;
    int mw, mh, ret, maxpad;
    MINUTIAE *minutiae;

    /* INITIALIZATION — same order as detect.c. */
    maxpad = get_max_padding_V2(lfsparms->windowsize, lfsparms->windowoffset,
                                lfsparms->dirbin_grid_w, lfsparms->dirbin_grid_h);
    if ((ret = init_dir2rad(&dir2rad, lfsparms->num_directions)))
        return ret;
    if ((ret = init_dftwaves(&dftwaves, dft_coefs, lfsparms->num_dft_waves,
                             lfsparms->windowsize))) {
        free_dir2rad(dir2rad);
        return ret;
    }
    if ((ret = init_rotgrids(&dftgrids, iw, ih, maxpad,
                             lfsparms->start_dir_angle, lfsparms->num_directions,
                             lfsparms->windowsize, lfsparms->windowsize,
                             RELATIVE2ORIGIN))) {
        free_dir2rad(dir2rad);
        free_dftwaves(dftwaves);
        return ret;
    }

    /* Pad input image based on max padding. */
    if (maxpad > 0) {
        if ((ret = pad_uchar_image(&pdata, &pw, &ph, idata, iw, ih,
                                   maxpad, lfsparms->pad_value))) {
            free_dir2rad(dir2rad);
            free_dftwaves(dftwaves);
            free_rotgrids(dftgrids);
            return ret;
        }
    } else {
        pdata = (unsigned char *) malloc((size_t) iw * ih);
        if (!pdata) {
            free_dir2rad(dir2rad);
            free_dftwaves(dftwaves);
            free_rotgrids(dftgrids);
            return -580;
        }
        memcpy(pdata, idata, (size_t) iw * ih);
        pw = iw;
        ph = ih;
    }

    /* Scale input image to 6 bits [0..63]. */
    bits_8to6(pdata, pw, ph);

    /* MAPS. */
    if ((ret = gen_image_maps(&direction_map, &low_contrast_map,
                              &low_flow_map, &high_curve_map, &mw, &mh,
                              pdata, pw, ph, dir2rad, dftwaves, dftgrids, lfsparms))) {
        free_dir2rad(dir2rad);
        free_dftwaves(dftwaves);
        free_rotgrids(dftgrids);
        free(pdata);
        return ret;
    }
    free_dir2rad(dir2rad);
    free_dftwaves(dftwaves);
    free_rotgrids(dftgrids);

    /* BINARIZATION. */
    if ((ret = init_rotgrids(&dirbingrids, iw, ih, maxpad,
                             lfsparms->start_dir_angle, lfsparms->num_directions,
                             lfsparms->dirbin_grid_w, lfsparms->dirbin_grid_h,
                             RELATIVE2CENTER))) {
        free(pdata);
        free(direction_map);
        free(low_contrast_map);
        free(low_flow_map);
        free(high_curve_map);
        return ret;
    }
    if ((ret = binarize_V2(&bdata, &bw, &bh, pdata, pw, ph, direction_map, mw, mh,
                           dirbingrids, lfsparms))) {
        free(pdata);
        free(direction_map);
        free(low_contrast_map);
        free(low_flow_map);
        free(high_curve_map);
        free_rotgrids(dirbingrids);
        return ret;
    }
    free_rotgrids(dirbingrids);

    if ((iw != bw) || (ih != bh)) {
        fprintf(stderr, "ERROR: detect_stage_dump: binary image bad dims %d,%d\n", bw, bh);
        free(pdata);
        free(direction_map);
        free(low_contrast_map);
        free(low_flow_map);
        free(high_curve_map);
        free(bdata);
        return -581;
    }

    /* .brwpre: binarize_V2 output BEFORE gray2bin — grayscale binary (0=ridge, 255=valley),
       original image size (bw==iw, bh==ih). Full-corpus exact match to port's DebugMaps.binarized. */
    snprintf(path, sizeof path, "%s/%s.brwpre", dir, name);
    FILE *bf = fopen(path, "wb");
    if (bf) {
        fwrite(bdata, 1, (size_t) bw * bh, bf);
        fclose(bf);
    } else {
        fprintf(stderr, "ERROR: cannot write %s\n", path);
    }

    /* DETECTION. */
    gray2bin(1, 1, 0, bdata, iw, ih);
    if ((ret = alloc_minutiae(&minutiae, MAX_MINUTIAE))) {
        free(pdata);
        free(direction_map);
        free(low_contrast_map);
        free(low_flow_map);
        free(high_curve_map);
        free(bdata);
        return ret;
    }
    if ((ret = detect_minutiae_V2(minutiae, bdata, iw, ih,
                                  direction_map, low_flow_map, high_curve_map,
                                  mw, mh, lfsparms))) {
        free(pdata);
        free(direction_map);
        free(low_contrast_map);
        free(low_flow_map);
        free(high_curve_map);
        free(bdata);
        free_minutiae(minutiae);
        return ret;
    }

    /* .rmin: raw minutia list captured BEFORE remove_false_minutia_V2, list order preserved.
       Header = count; each line "x y direction type appearing" in lfs-internal representation. */
    snprintf(path, sizeof path, "%s/%s.rmin", dir, name);
    FILE *rf = fopen(path, "wb");
    if (rf) {
        fprintf(rf, "%d\n", minutiae->num);
        for (int i = 0; i < minutiae->num; i++) {
            MINUTIA *m = minutiae->list[i];
            fprintf(rf, "%d %d %d %d %d\n",
                    m->x, m->y, m->direction, m->type, m->appearing);
        }
        fclose(rf);
    } else {
        fprintf(stderr, "ERROR: cannot write %s\n", path);
    }

    /* REMOVAL — stock remove_false_minutia_V2 (remove.c), same call/args as detect.c's
       lfs_detect_minutiae_V2 body: (minutiae, bdata, iw, ih, direction_map, low_flow_map,
       high_curve_map, mw, mh, lfsparms). Runs the full 10-step false-minutia pruning in place,
       stopping BEFORE count_minutiae_ridges. */
    if ((ret = remove_false_minutia_V2(minutiae, bdata, iw, ih,
                                       direction_map, low_flow_map, high_curve_map,
                                       mw, mh, lfsparms))) {
        free(pdata);
        free(direction_map);
        free(low_contrast_map);
        free(low_flow_map);
        free(high_curve_map);
        free(bdata);
        free_minutiae(minutiae);
        return ret;
    }

    /* .rmin2: minutia list captured immediately AFTER remove_false_minutia_V2 and BEFORE
       count_minutiae_ridges. Same contract/format as .rmin (header = count; each line
       "x y direction type appearing" in lfs-internal representation), list order preserved.
       The count is <= .rmin (removal only prunes; step 6 may adjust position/direction). */
    snprintf(path, sizeof path, "%s/%s.rmin2", dir, name);
    FILE *rf2 = fopen(path, "wb");
    if (rf2) {
        fprintf(rf2, "%d\n", minutiae->num);
        for (int i = 0; i < minutiae->num; i++) {
            MINUTIA *m = minutiae->list[i];
            fprintf(rf2, "%d %d %d %d %d\n",
                    m->x, m->y, m->direction, m->type, m->appearing);
        }
        fclose(rf2);
    } else {
        fprintf(stderr, "ERROR: cannot write %s\n", path);
    }

    free_minutiae(minutiae);
    free(pdata);
    free(direction_map);
    free(low_contrast_map);
    free(low_flow_map);
    free(high_curve_map);
    free(bdata);
    return 0;
}

static int process(const char *dir, const char *name, int dump_maps) {
    char path[1024];

    /* Manifest sidecar: "width height ppi". */
    snprintf(path, sizeof path, "%s/%s.manifest", dir, name);
    FILE *mf = fopen(path, "r");
    if (!mf) {
        fprintf(stderr, "ERROR: missing manifest %s\n", path);
        return -1;
    }
    int iw, ih, ippi;
    int nread = fscanf(mf, "%d %d %d", &iw, &ih, &ippi);
    fclose(mf);
    if (nread != 3) {
        fprintf(stderr, "ERROR: bad manifest %s\n", path);
        return -1;
    }

    /* Headerless raw 8-bit grayscale image. */
    snprintf(path, sizeof path, "%s/%s.raw", dir, name);
    int len = 0;
    unsigned char *idata = read_file(path, &len);
    if (!idata) {
        fprintf(stderr, "ERROR: cannot read %s\n", path);
        return -1;
    }
    if (len != iw * ih) {
        fprintf(stderr, "ERROR: %s: %d bytes != %dx%d\n", path, len, iw, ih);
        free(idata);
        return -1;
    }

    double ippmm = ippi / (double) MM_PER_INCH;

    MINUTIAE *minutiae;
    int *quality_map, *direction_map, *low_contrast_map, *low_flow_map, *high_curve_map;
    int map_w, map_h;
    unsigned char *bdata;
    int bw, bh, bd;

    int ret = get_minutiae(&minutiae, &quality_map, &direction_map,
                           &low_contrast_map, &low_flow_map, &high_curve_map,
                           &map_w, &map_h, &bdata, &bw, &bh, &bd,
                           idata, iw, ih, 8, ippmm, &lfsparms_V2);
    if (ret) {
        free(idata);
        fprintf(stderr, "ERROR: get_minutiae(%s) returned %d\n", name, ret);
        return ret;
    }

    /* Per-stage golden (binarize/detect, pre-removal) — needs the untouched idata, so run it while
       idata is still alive and before it is freed below. Same env gate as the map dumps. */
    if (dump_maps) {
        int sret = detect_stage_dump(dir, name, idata, iw, ih);
        if (sret)
            fprintf(stderr, "ERROR: detect_stage_dump(%s) returned %d\n", name, sret);
    }
    free(idata);

    snprintf(path, sizeof path, "%s/%s.xyt", dir, name);
    write_xyt(path, minutiae, iw, ih);

    if (dump_maps) {
        snprintf(path, sizeof path, "%s/%s.dm", dir, name);
        dump_map_oracle(path, direction_map, map_w, map_h);
        snprintf(path, sizeof path, "%s/%s.lcm", dir, name);
        dump_map_oracle(path, low_contrast_map, map_w, map_h);
        snprintf(path, sizeof path, "%s/%s.lfm", dir, name);
        dump_map_oracle(path, low_flow_map, map_w, map_h);
        snprintf(path, sizeof path, "%s/%s.hcm", dir, name);
        dump_map_oracle(path, high_curve_map, map_w, map_h);

        snprintf(path, sizeof path, "%s/%s.brw", dir, name);
        FILE *bf = fopen(path, "wb");
        if (bf) {
            fwrite(bdata, 1, (size_t) bw * bh, bf);
            fclose(bf);
        }
        /* Also record the binary-image dimensions so a headerless .brw is interpretable. */
        snprintf(path, sizeof path, "%s/%s.brwdim", dir, name);
        FILE *df = fopen(path, "w");
        if (df) {
            fprintf(df, "%d %d %d %d\n", bw, bh, map_w, map_h);
            fclose(df);
        }
    }

    printf("%s\t%d minutiae\t%dx%d map\t%dx%d bin\n",
           name, minutiae->num, map_w, map_h, bw, bh);

    free_minutiae(minutiae);
    free(quality_map);
    free(direction_map);
    free(low_contrast_map);
    free(low_flow_map);
    free(high_curve_map);
    free(bdata);
    return 0;
}

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: %s <corpus_dir> [manifest.txt]\n", argv[0]);
        return 2;
    }
    const char *dir = argv[1];
    char mpath[1024];
    if (argc >= 3)
        snprintf(mpath, sizeof mpath, "%s", argv[2]);
    else
        snprintf(mpath, sizeof mpath, "%s/manifest.txt", dir);

    FILE *fp = fopen(mpath, "r");
    if (!fp) {
        fprintf(stderr, "cannot open manifest index %s\n", mpath);
        return 2;
    }

    int dump_maps = getenv("MINDTCT_DUMP_MAPS") != NULL;
    int failures = 0;
    char line[512];
    while (fgets(line, sizeof line, fp)) {
        char name[256];
        if (sscanf(line, "%255s", name) != 1)
            continue;
        if (process(dir, name, dump_maps) != 0)
            failures++;
    }
    fclose(fp);
    return failures ? 1 : 0;
}
