// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Golden cross-implementation oracle for the **maps** stage: our [`fprint_mindtct::debug_maps`]
//! reproduces the stock C **MINDTCT** intermediate block maps, verified block-for-block against the
//! frozen corpus.
//!
//! The corpus (`tests/fixtures/<name>.raw` + `<name>.manifest`) and the reference maps
//! (`<name>.dm` direction, `<name>.lcm` low-contrast, `<name>.lfm` low-flow, `<name>.hcm`
//! high-curvature) are produced by the stock NBIS `get_minutiae` / `lfs_detect_minutiae_V2` pipeline
//! in Docker (`mise run mindtct-oracle`, `MINDTCT_DUMP_MAPS=1`) and committed as a permanent oracle —
//! regenerate them only deliberately. This test needs no Docker: it reads the same raw bytes the C
//! read, runs the pure-Rust front-end, and compares.
//!
//! Each map dump is `stock results.c:dump_map()` layout: `"%2d "` per block cell, one row of
//! `map_w` cells per line, `map_h` lines — i.e. whitespace-separated block integers in row-major
//! order. `map_w`/`map_h` are recovered from the dump's own geometry and cross-checked against the
//! port's reported dimensions.
//!
//! Comparison is **exact** (every block must match to the integer). Each map stage is a separate
//! `#[test]` so a divergence localizes to the offending stage; failure messages name the image, the
//! `(bx, by)` block, and the `got`/`want` pair, capped so a wholesale divergence stays readable.

use std::path::{Path, PathBuf};

use fprint_mindtct::{
    debug_maps, debug_raw_minutiae, debug_removed_minutiae, detect_minutiae, DebugMaps, GrayImage,
    Minutia, RawMinutia,
};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The corpus image basenames, in `manifest.txt` order.
fn corpus_names() -> Vec<String> {
    let dir = fixtures_dir();
    let text = std::fs::read_to_string(dir.join("manifest.txt"))
        .expect("manifest.txt missing — run `mise run mindtct-oracle`");
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

/// A parsed `<name>.manifest` sidecar: "width height ppi".
struct Manifest {
    width: usize,
    height: usize,
    ppi: u16,
}

fn load_manifest(path: &Path) -> Manifest {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    let mut it = text.split_whitespace();
    let width = it.next().expect("manifest: width").parse().expect("width");
    let height = it
        .next()
        .expect("manifest: height")
        .parse()
        .expect("height");
    let ppi = it.next().expect("manifest: ppi").parse().expect("ppi");
    Manifest { width, height, ppi }
}

/// A block-integer map parsed from a stock `dump_map()` file: the flat row-major values plus the
/// geometry recovered from the dump (`w` = cells on the first row, `h` = number of rows).
struct GoldenMap {
    vals: Vec<i32>,
    w: usize,
    h: usize,
}

/// Parse a stock `dump_map()` dump ("%2d " per cell, one map row per text line) into a flat
/// row-major `Vec<i32>` with its geometry. Every non-empty line must hold the same cell count.
fn load_dump_map(path: &Path) -> GoldenMap {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    let mut vals = Vec::new();
    let mut w = 0usize;
    let mut h = 0usize;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let row: Vec<i32> = line
            .split_whitespace()
            .map(|t| {
                t.parse()
                    .unwrap_or_else(|e| panic!("{path:?}: bad cell {t:?}: {e}"))
            })
            .collect();
        if h == 0 {
            w = row.len();
        } else {
            assert_eq!(
                row.len(),
                w,
                "{path:?}: ragged dump — row {h} has {} cells, expected {w}",
                row.len()
            );
        }
        vals.extend_from_slice(&row);
        h += 1;
    }
    GoldenMap { vals, w, h }
}

/// Load a corpus image and run the pure-Rust front-end, returning the port's maps alongside the
/// image geometry.
fn run_port(name: &str) -> DebugMaps {
    let dir = fixtures_dir();
    let man = load_manifest(&dir.join(format!("{name}.manifest")));
    let data = std::fs::read(dir.join(format!("{name}.raw")))
        .unwrap_or_else(|e| panic!("read {name}.raw: {e}"));
    assert_eq!(
        data.len(),
        man.width * man.height,
        "{name}.raw: {} bytes != {}x{}",
        data.len(),
        man.width,
        man.height
    );
    let img = GrayImage {
        data: &data,
        width: man.width,
        height: man.height,
        ppi: man.ppi,
    };
    debug_maps(img)
}

/// Compare one flat block map against its golden dump, exact. Returns a list of human-readable
/// mismatch descriptions (`(bx,by): got G want W`), capped at `MAX_REPORT` entries plus a count.
fn diff_map(got: &[i32], gold: &GoldenMap) -> Vec<String> {
    const MAX_REPORT: usize = 12;
    let mut out = Vec::new();
    if got.len() != gold.vals.len() {
        out.push(format!(
            "length mismatch: port has {} blocks, golden has {} ({}x{})",
            got.len(),
            gold.vals.len(),
            gold.w,
            gold.h
        ));
        return out;
    }
    let mut n = 0usize;
    for (i, (&g, &w)) in got.iter().zip(gold.vals.iter()).enumerate() {
        if g != w {
            n += 1;
            if out.len() < MAX_REPORT {
                let bx = i % gold.w;
                let by = i / gold.w;
                out.push(format!("({bx},{by}): got {g} want {w}"));
            }
        }
    }
    if n > out.len() {
        out.push(format!(
            "... and {} more divergent blocks (of {})",
            n - MAX_REPORT,
            got.len()
        ));
    }
    out
}

/// Selector for the four block maps, so one comparison body drives all four stage tests.
enum Which {
    Direction,
    LowContrast,
    LowFlow,
    HighCurve,
}

impl Which {
    fn ext(&self) -> &'static str {
        match self {
            Which::Direction => "dm",
            Which::LowContrast => "lcm",
            Which::LowFlow => "lfm",
            Which::HighCurve => "hcm",
        }
    }
    fn field<'a>(&self, m: &'a DebugMaps) -> &'a [i32] {
        match self {
            Which::Direction => &m.direction_map,
            Which::LowContrast => &m.low_contrast_map,
            Which::LowFlow => &m.low_flow_map,
            Which::HighCurve => &m.high_curve_map,
        }
    }
}

/// Drive one map stage across the whole corpus, asserting an exact block-for-block match.
fn assert_map_matches_stock(which: Which) {
    let dir = fixtures_dir();
    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for name in corpus_names() {
        let maps = run_port(&name);
        let gold = load_dump_map(&dir.join(format!("{name}.{}", which.ext())));
        // The port's reported geometry must agree with the dump geometry before per-block diffing
        // is meaningful; a mismatch here is reported by this stage too (it gates the comparison).
        if maps.map_w != gold.w || maps.map_h != gold.h {
            failures.push(format!(
                "{name}: port map is {}x{}, golden {}.{} is {}x{}",
                maps.map_w,
                maps.map_h,
                name,
                which.ext(),
                gold.w,
                gold.h
            ));
            checked += 1;
            continue;
        }
        let diffs = diff_map(which.field(&maps), &gold);
        if !diffs.is_empty() {
            failures.push(format!(
                "{name} [{}]:\n    {}",
                which.ext(),
                diffs.join("\n    ")
            ));
        }
        checked += 1;
    }
    assert!(checked > 0, "no images checked — corpus missing?");
    assert!(
        failures.is_empty(),
        "{}/{} images diverged from the stock {} map:\n  {}",
        failures.len(),
        checked,
        which.ext(),
        failures.join("\n  ")
    );
}

#[test]
fn map_dimensions_match_stock() {
    let dir = fixtures_dir();
    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for name in corpus_names() {
        let maps = run_port(&name);
        // All four dumps share the block geometry; the direction map's dump is the witness.
        let gold = load_dump_map(&dir.join(format!("{name}.dm")));
        if maps.map_w != gold.w || maps.map_h != gold.h {
            failures.push(format!(
                "{name}: port map is {}x{}, stock is {}x{}",
                maps.map_w, maps.map_h, gold.w, gold.h
            ));
        }
        // Every emitted map vector must be exactly map_w*map_h long (row-major block order).
        let expect = maps.map_w * maps.map_h;
        for (label, v) in [
            ("direction_map", &maps.direction_map),
            ("low_contrast_map", &maps.low_contrast_map),
            ("low_flow_map", &maps.low_flow_map),
            ("high_curve_map", &maps.high_curve_map),
        ] {
            if v.len() != expect {
                failures.push(format!(
                    "{name}: {label} has {} blocks, expected {}x{}={}",
                    v.len(),
                    maps.map_w,
                    maps.map_h,
                    expect
                ));
            }
        }
        checked += 1;
    }
    assert!(checked > 0, "no images checked — corpus missing?");
    assert!(
        failures.is_empty(),
        "{}/{} images had map-geometry divergence:\n  {}",
        failures.len(),
        checked,
        failures.join("\n  ")
    );
}

/// The binarized-image dimensions recorded by the oracle in a `<name>.brwdim` sidecar:
/// "bw bh map_w map_h". `bw`/`bh` are the (unpadded) binary-image size; the map dims are carried
/// so a headerless `.brw` is fully interpretable.
struct BrwDim {
    bw: usize,
    bh: usize,
}

fn load_brwdim(path: &Path) -> BrwDim {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    let mut it = text.split_whitespace();
    let bw = it.next().expect("brwdim: bw").parse().expect("bw");
    let bh = it.next().expect("brwdim: bh").parse().expect("bh");
    BrwDim { bw, bh }
}

/// The binarized image the port emits must have the oracle's binary-image geometry: `bw * bh`
/// bytes, at the *original* image size (`bw == iw`, `bh == ih`), matching `<name>.brwdim` and the
/// headerless `<name>.brw` byte length. This is the dimension/origin contract of the binarization
/// stage — the dir-bin grid pad stripped, no residual padding — and holds for the whole corpus.
#[test]
fn binarized_dims_match_stock() {
    let dir = fixtures_dir();
    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for name in corpus_names() {
        let maps = run_port(&name);
        let man = load_manifest(&dir.join(format!("{name}.manifest")));
        let bd = load_brwdim(&dir.join(format!("{name}.brwdim")));
        let brw_len = std::fs::metadata(dir.join(format!("{name}.brw")))
            .unwrap_or_else(|e| panic!("stat {name}.brw: {e}"))
            .len() as usize;
        // The oracle's binary image is the original image size, and its headerless byte count agrees.
        if bd.bw != man.width || bd.bh != man.height || brw_len != bd.bw * bd.bh {
            failures.push(format!(
                "{name}: brwdim {}x{} / .brw {} bytes vs image {}x{}",
                bd.bw, bd.bh, brw_len, man.width, man.height
            ));
        }
        if maps.binarized.len() != man.width * man.height {
            failures.push(format!(
                "{name}: port binarized {} bytes, expected {}x{}={}",
                maps.binarized.len(),
                man.width,
                man.height,
                man.width * man.height
            ));
        }
        checked += 1;
    }
    assert!(checked > 0, "no images checked — corpus missing?");
    assert!(
        failures.is_empty(),
        "{}/{} images had binarized-geometry divergence:\n  {}",
        failures.len(),
        checked,
        failures.join("\n  ")
    );
}

/// Images whose stock `<name>.brw` is *also* the pure directional-binarization output — i.e. the
/// downstream false-minutia removal (`remove_false_minutia_V2`) drew nothing on `bdata` for them.
///
/// The oracle's `.brw` is captured at the very end of `lfs_detect_minutiae_V2`, **after**
/// `detect_minutiae_V2` and `remove_false_minutia_V2` have run — and removal edits the binary image
/// in place (joining minutiae, filling pores; see stock `detect.c` L639 / `remove.c`). So for images
/// carrying minutiae, `.brw` diverges from the binarization stage's output by exactly those edits,
/// localized around the affected ridges. On these low-/no-minutia images removal is a no-op, so the
/// binarization output equals `.brw` **byte-for-byte** — a clean bit-exactness oracle for this stage.
/// (Full-corpus `.brw` equality must wait until the removal stage is ported.)
const BRW_EXACT_IMAGES: &[&str] = &[
    "uniform_128x128",
    "gradient_128x128",
    "grating_160x160_s1",
    "whorl_208x208_s2",
];

/// On the removal-untouched images, the port's binarization output (`binarize_V2` →
/// `binarize_image_V2` / `dirbinarize` + three `fill_holes` passes) equals the stock `.brw`
/// byte-for-byte — an exact cross-implementation check of the directional-binarization stage.
#[test]
fn binarized_image_matches_stock_where_removal_is_noop() {
    let dir = fixtures_dir();
    let corpus = corpus_names();
    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for &name in BRW_EXACT_IMAGES {
        assert!(
            corpus.iter().any(|n| n == name),
            "{name} not in corpus — update BRW_EXACT_IMAGES"
        );
        let maps = run_port(name);
        let bd = load_brwdim(&dir.join(format!("{name}.brwdim")));
        let want = std::fs::read(dir.join(format!("{name}.brw")))
            .unwrap_or_else(|e| panic!("read {name}.brw: {e}"));

        const MAX_REPORT: usize = 12;
        let mut diffs: Vec<String> = Vec::new();
        let mut n = 0usize;
        for (i, (&g, &w)) in maps.binarized.iter().zip(want.iter()).enumerate() {
            if g != w {
                n += 1;
                if diffs.len() < MAX_REPORT {
                    diffs.push(format!("({},{}): got {g} want {w}", i % bd.bw, i / bd.bw));
                }
            }
        }
        if n > diffs.len() {
            diffs.push(format!("... and {} more divergent pixels", n - MAX_REPORT));
        }
        if !diffs.is_empty() {
            failures.push(format!("{name} [brw]:\n    {}", diffs.join("\n    ")));
        }
        checked += 1;
    }
    assert!(checked > 0, "no exact-brw images checked");
    assert!(
        failures.is_empty(),
        "{}/{} removal-untouched images diverged from the stock binarized image:\n  {}",
        failures.len(),
        checked,
        failures.join("\n  ")
    );
}

/// Full-corpus, byte-exact binarization oracle against the **remove-free** stage golden.
///
/// The end-to-end `.brw` is captured after `remove_false_minutia_V2` edits `bdata` in place, so it
/// only matches the pure binarization output on the handful of removal-noop images
/// (`binarized_image_matches_stock_where_removal_is_noop`). The oracle's `detect_stage_dump` re-runs
/// the stock `lfs_detect_minutiae_V2` body up to (and including) `binarize_V2` and dumps its output
/// **before** `gray2bin(1,1,0)` as `<name>.brwpre`: the stock grayscale binary image (0=ridge,
/// 255=valley) at the original image size — exactly what `binarize_V2` produced, with no removal
/// contamination. So the port's `DebugMaps.binarized` must equal `.brwpre` byte-for-byte on **every**
/// image in the corpus. This is the full binarization-stage bit-exactness check (the removal-noop
/// variant above is the strict subset that also happens to survive in `.brw`).
#[test]
fn binarized_matches_stock() {
    let dir = fixtures_dir();
    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for name in corpus_names() {
        let maps = run_port(&name);
        let man = load_manifest(&dir.join(format!("{name}.manifest")));
        let want = std::fs::read(dir.join(format!("{name}.brwpre")))
            .unwrap_or_else(|e| panic!("read {name}.brwpre — run `mise run mindtct-oracle`: {e}"));
        if want.len() != man.width * man.height {
            failures.push(format!(
                "{name}: .brwpre {} bytes != image {}x{}",
                want.len(),
                man.width,
                man.height
            ));
            checked += 1;
            continue;
        }
        if maps.binarized.len() != want.len() {
            failures.push(format!(
                "{name}: port binarized {} bytes, .brwpre {} bytes",
                maps.binarized.len(),
                want.len()
            ));
            checked += 1;
            continue;
        }
        const MAX_REPORT: usize = 12;
        let mut diffs: Vec<String> = Vec::new();
        let mut n = 0usize;
        for (i, (&g, &w)) in maps.binarized.iter().zip(want.iter()).enumerate() {
            if g != w {
                n += 1;
                if diffs.len() < MAX_REPORT {
                    diffs.push(format!(
                        "({},{}): got {g} want {w}",
                        i % man.width,
                        i / man.width
                    ));
                }
            }
        }
        if n > diffs.len() {
            diffs.push(format!("... and {} more divergent pixels", n - MAX_REPORT));
        }
        if !diffs.is_empty() {
            failures.push(format!("{name} [brwpre]:\n    {}", diffs.join("\n    ")));
        }
        checked += 1;
    }
    assert!(checked > 0, "no images checked — corpus missing?");
    assert!(
        failures.is_empty(),
        "{}/{} images diverged from the stock pre-removal binarized image (.brwpre):\n  {}",
        failures.len(),
        checked,
        failures.join("\n  ")
    );
}

#[test]
fn direction_map_matches_stock() {
    assert_map_matches_stock(Which::Direction);
}

#[test]
fn low_contrast_map_matches_stock() {
    assert_map_matches_stock(Which::LowContrast);
}

#[test]
fn low_flow_map_matches_stock() {
    assert_map_matches_stock(Which::LowFlow);
}

#[test]
fn high_curve_map_matches_stock() {
    assert_map_matches_stock(Which::HighCurve);
}

/// Parse a stock `.rmin` dump: a header count line, then one `"x y direction type appearing"` row per
/// raw minutia (list order preserved). Returns the rows as [`RawMinutia`], asserting the count header.
fn load_rmin(path: &Path) -> Vec<RawMinutia> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let count: usize = lines
        .next()
        .expect("rmin: header count")
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("{path:?}: bad header: {e}"));
    let mut out = Vec::with_capacity(count);
    for line in lines {
        let v: Vec<i32> = line
            .split_whitespace()
            .map(|t| {
                t.parse()
                    .unwrap_or_else(|e| panic!("{path:?}: bad field {t:?}: {e}"))
            })
            .collect();
        assert_eq!(v.len(), 5, "{path:?}: row {line:?} is not 5 fields");
        out.push(RawMinutia {
            x: v[0],
            y: v[1],
            direction: v[2],
            kind: v[3],
            appearing: v[4],
        });
    }
    assert_eq!(
        out.len(),
        count,
        "{path:?}: header says {count} minutiae, found {}",
        out.len()
    );
    out
}

/// The port's `debug_raw_minutiae` reproduces the stock `detect_minutiae_V2` output exactly — same
/// count, same fields, same list order — for every image in the corpus, verified against the frozen
/// `.rmin` oracle (captured before `remove_false_minutia_V2`).
#[test]
fn raw_minutiae_match_stock() {
    const MAX_REPORT: usize = 12;
    let dir = fixtures_dir();
    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for name in corpus_names() {
        let man = load_manifest(&dir.join(format!("{name}.manifest")));
        let data = std::fs::read(dir.join(format!("{name}.raw")))
            .unwrap_or_else(|e| panic!("read {name}.raw: {e}"));
        let img = GrayImage {
            data: &data,
            width: man.width,
            height: man.height,
            ppi: man.ppi,
        };
        let got = debug_raw_minutiae(img);
        let want = load_rmin(&dir.join(format!("{name}.rmin")));

        let mut diffs: Vec<String> = Vec::new();
        if got.len() != want.len() {
            diffs.push(format!("count: port {} want {}", got.len(), want.len()));
        }
        let mut n = 0usize;
        for (i, (g, w)) in got.iter().zip(want.iter()).enumerate() {
            if g != w {
                n += 1;
                if diffs.len() < MAX_REPORT {
                    diffs.push(format!("[{i}] port {g:?} want {w:?}"));
                }
            }
        }
        if n > MAX_REPORT {
            diffs.push(format!(
                "... and {} more divergent minutiae",
                n - MAX_REPORT
            ));
        }
        if !diffs.is_empty() {
            failures.push(format!("{name} [rmin]:\n    {}", diffs.join("\n    ")));
        }
        checked += 1;
    }
    assert!(checked > 0, "no images checked — corpus missing?");
    assert!(
        failures.is_empty(),
        "{}/{} images diverged from the stock raw minutiae (.rmin):\n  {}",
        failures.len(),
        checked,
        failures.join("\n  ")
    );
}

/// The port's `debug_removed_minutiae` reproduces the stock `remove_false_minutia_V2` output exactly
/// — same count, same fields, same list order — for every image in the corpus, verified against the
/// frozen `.rmin2` oracle (captured after the ten false-minutia removal stages).
#[test]
fn removed_minutiae_match_stock() {
    const MAX_REPORT: usize = 12;
    let dir = fixtures_dir();
    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for name in corpus_names() {
        let man = load_manifest(&dir.join(format!("{name}.manifest")));
        let data = std::fs::read(dir.join(format!("{name}.raw")))
            .unwrap_or_else(|e| panic!("read {name}.raw: {e}"));
        let img = GrayImage {
            data: &data,
            width: man.width,
            height: man.height,
            ppi: man.ppi,
        };
        let got = debug_removed_minutiae(img);
        let want = load_rmin(&dir.join(format!("{name}.rmin2")));

        let mut diffs: Vec<String> = Vec::new();
        if got.len() != want.len() {
            diffs.push(format!("count: port {} want {}", got.len(), want.len()));
        }
        let mut n = 0usize;
        for (i, (g, w)) in got.iter().zip(want.iter()).enumerate() {
            if g != w {
                n += 1;
                if diffs.len() < MAX_REPORT {
                    diffs.push(format!("[{i}] port {g:?} want {w:?}"));
                }
            }
        }
        if n > MAX_REPORT {
            diffs.push(format!(
                "... and {} more divergent minutiae",
                n - MAX_REPORT
            ));
        }
        if !diffs.is_empty() {
            failures.push(format!("{name} [rmin2]:\n    {}", diffs.join("\n    ")));
        }
        checked += 1;
    }
    assert!(checked > 0, "no images checked — corpus missing?");
    assert!(
        failures.is_empty(),
        "{}/{} images diverged from the stock removed minutiae (.rmin2):\n  {}",
        failures.len(),
        checked,
        failures.join("\n  ")
    );
}

/// Parse a stock `.xyt` dump: one `"x y theta q"` row per final minutia, list order preserved (the
/// stock `results.c:write_minutiae_XYTQ` layout). An empty file is a legal zero-minutia oracle. Each
/// row maps onto a [`Minutia`] for a field-for-field comparison against [`detect_minutiae`].
fn load_xyt(path: &Path) -> Vec<Minutia> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let v: Vec<i32> = line
            .split_whitespace()
            .map(|t| {
                t.parse()
                    .unwrap_or_else(|e| panic!("{path:?}: bad field {t:?}: {e}"))
            })
            .collect();
        assert_eq!(
            v.len(),
            4,
            "{path:?}: row {line:?} is not 4 fields (x y theta q)"
        );
        out.push(Minutia {
            x: v[0],
            y: v[1],
            theta: v[2],
            quality: v[3],
        });
    }
    out
}

/// The full-pipeline final gate: the port's [`detect_minutiae`] reproduces the stock `get_minutiae`
/// output exactly — same count, same `x y theta q`, same list order — for every image in the corpus,
/// verified against the frozen `.xyt` oracle (the `results.c:write_minutiae_XYTQ` final output, after
/// ridge counting, false-minutia removal, and integrated quality). This is the end-to-end MINDTCT
/// bit-exactness check: everything upstream (`.dm`/`.lcm`/`.lfm`/`.hcm`/`.brwpre`/`.rmin`/`.rmin2`)
/// feeds the `xyt` conversion here.
#[test]
fn minutiae_match_stock() {
    const MAX_REPORT: usize = 12;
    let dir = fixtures_dir();
    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for name in corpus_names() {
        let man = load_manifest(&dir.join(format!("{name}.manifest")));
        let data = std::fs::read(dir.join(format!("{name}.raw")))
            .unwrap_or_else(|e| panic!("read {name}.raw: {e}"));
        let img = GrayImage {
            data: &data,
            width: man.width,
            height: man.height,
            ppi: man.ppi,
        };
        let got = detect_minutiae(img);
        let want = load_xyt(&dir.join(format!("{name}.xyt")));

        let mut diffs: Vec<String> = Vec::new();
        if got.len() != want.len() {
            diffs.push(format!("count: port {} want {}", got.len(), want.len()));
        }
        let mut n = 0usize;
        for (i, (g, w)) in got.iter().zip(want.iter()).enumerate() {
            if g != w {
                n += 1;
                if diffs.len() < MAX_REPORT {
                    diffs.push(format!(
                        "[{i}] port ({},{},{},{}) want ({},{},{},{})",
                        g.x, g.y, g.theta, g.quality, w.x, w.y, w.theta, w.quality
                    ));
                }
            }
        }
        if n > MAX_REPORT {
            diffs.push(format!(
                "... and {} more divergent minutiae",
                n - MAX_REPORT
            ));
        }
        if !diffs.is_empty() {
            failures.push(format!("{name} [xyt]:\n    {}", diffs.join("\n    ")));
        }
        checked += 1;
    }
    assert!(checked > 0, "no images checked — corpus missing?");
    assert!(
        failures.is_empty(),
        "{}/{} images diverged from the stock final minutiae (.xyt):\n  {}",
        failures.len(),
        checked,
        failures.join("\n  ")
    );
}
