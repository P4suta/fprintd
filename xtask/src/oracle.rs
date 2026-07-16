// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Regenerating the NBIS golden corpora from the stock C tools.
//!
//! Both oracles follow the same shape: a deterministic Python generator writes the inputs, the
//! stock NBIS implementation is compiled in a gcc container and run over them, and its output is
//! frozen as the fixtures our Rust ports are tested against. Any change here must leave the
//! regenerated fixtures byte-identical, or it has moved the goldens rather than reproduced them.
//!
//! Regeneration is deliberate: it overwrites frozen fixtures that exist to catch drift.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::docker::{container_path, relative_to, Run};

/// gcc image for building the stock C oracles. Pinned: the fixtures are goldens, so the compiler
/// that produces them must not move.
const GCC_IMAGE: &str = "gcc:13-bookworm";
/// Stock NBIS, cloned by `mise run clone-ref-nbis` and git-ignored.
const NBIS_STOCK: &str = "reference/nbis-stock";

/// The two NBIS kernels we port, each with a stock oracle to check against.
#[derive(Clone, Copy)]
pub enum Oracle {
    Bozorth3,
    Mindtct,
}

impl Oracle {
    fn fixtures(self) -> &'static str {
        match self {
            Oracle::Bozorth3 => "crates/fprint-bozorth3/tests/fixtures",
            Oracle::Mindtct => "crates/fprint-mindtct/tests/fixtures",
        }
    }

    fn tooling(self) -> &'static str {
        match self {
            Oracle::Bozorth3 => "docker/bozorth3-oracle",
            Oracle::Mindtct => "docker/mindtct-oracle",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Oracle::Bozorth3 => "bozorth3",
            Oracle::Mindtct => "mindtct",
        }
    }
}

pub fn regenerate(root: &Path, oracle: Oracle) -> Result<(), String> {
    let stock = root.join(NBIS_STOCK);
    if !stock.is_dir() {
        return Err(format!(
            "{} not found — clone stock NBIS first (`mise run clone-ref-nbis`)",
            stock.display()
        ));
    }
    let fixtures = root.join(oracle.fixtures());
    std::fs::create_dir_all(&fixtures)
        .map_err(|e| format!("create {}: {e}", fixtures.display()))?;

    gen_corpus(root, oracle, &fixtures)?;
    let binary = compile(root, oracle, &stock)?;
    match oracle {
        Oracle::Bozorth3 => {
            score_bozorth3(root, &binary, &fixtures)?;
            dump_stages_bozorth3(root, &binary, &fixtures)
        }
        Oracle::Mindtct => detect_mindtct(root, &binary, &fixtures),
    }
}

/// Write the corpus inputs. Deterministic by construction (the generators roll their own LCG), so
/// running this on a clean tree must leave no diff.
fn gen_corpus(root: &Path, oracle: Oracle, fixtures: &Path) -> Result<(), String> {
    let script = root.join(oracle.tooling()).join("gen_corpus.py");
    println!("xtask: generating the {} corpus", oracle.name());
    let status = Command::new("python")
        .arg(&script)
        .arg(fixtures)
        .status()
        .map_err(|e| format!("spawn python: {e} (is python on PATH?)"))?;
    if !status.success() {
        return Err(format!("{} failed ({status})", script.display()));
    }
    Ok(())
}

/// Compile the stock oracle in the gcc container, into a git-ignored path under `target/`.
///
/// The binary lands on the bind mount rather than the container's `/tmp` so that the second
/// container can run it: `docker run --rm` keeps nothing.
fn compile(root: &Path, oracle: Oracle, stock: &Path) -> Result<PathBuf, String> {
    let out_dir = root.join("target/oracle");
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("create {}: {e}", out_dir.display()))?;
    let binary = out_dir.join(format!("{}-oracle", oracle.name()));

    let mut includes: Vec<PathBuf> = Vec::new();
    let mut sources: Vec<PathBuf> = vec![root.join(oracle.tooling()).join("oracle.c")];

    match oracle {
        Oracle::Bozorth3 => {
            let src = stock.join("bozorth3");
            includes.push(src.join("include"));
            includes.push(stock.join("commonnbis/include"));
            for f in [
                "bozorth3.c",
                "bz_io.c",
                "bz_sort.c",
                "bz_alloc.c",
                "bz_gbls.c",
                "bz_drvrs.c",
            ] {
                sources.push(src.join("src/lib/bozorth3").join(f));
            }
        }
        Oracle::Mindtct => {
            // The stock build generates an2k.h from an2k.h.src (a plain copy — the .src has no
            // substitutions the oracle needs). `lfs.h` includes <an2k.h> unconditionally, so
            // every mindtct source needs it present.
            let geninc = root.join("target/oracle/mindtct-geninc");
            std::fs::create_dir_all(&geninc)
                .map_err(|e| format!("create {}: {e}", geninc.display()))?;
            let an2k_src = stock.join("an2k/include/an2k.h.src");
            std::fs::copy(&an2k_src, geninc.join("an2k.h"))
                .map_err(|e| format!("copy {}: {e}", an2k_src.display()))?;

            let src = stock.join("mindtct");
            includes.push(geninc);
            includes.push(src.join("include"));
            includes.push(stock.join("commonnbis/include"));
            sources.extend(mindtct_lib_sources(&src.join("src/lib/mindtct"))?);
        }
    }

    let mut argv: Vec<String> = vec!["gcc".into(), "-O2".into(), "-w".into()];
    for inc in &includes {
        argv.push("-I".into());
        argv.push(container_path(&relative_to(root, inc)?));
    }
    for src in &sources {
        argv.push(container_path(&relative_to(root, src)?));
    }
    argv.push("-lm".into());
    argv.push("-o".into());
    argv.push(container_path(&relative_to(root, &binary)?));

    println!(
        "xtask: compiling the stock NBIS {} oracle ({} sources)",
        oracle.name(),
        sources.len()
    );
    Run::new(GCC_IMAGE).args(argv).output(root)?;
    Ok(binary)
}

/// MINDTCT's library sources, minus the three that only serve the stock CLI driver and drag in the
/// ANSI/NIST (an2k) and Sun-raster image libraries: `to_type9.c` + `update.c` (an2k) and
/// `results.c` (sunrast). The oracle reimplements the two result writers it needs.
///
/// Everything else is required: the stock `get_minutiae()` / `lfs_detect_minutiae_V2` pipeline
/// pulls in the whole detect path, and the oracle's second driver re-calls the same individual
/// functions to emit the per-stage `.brwpre` / `.rmin` goldens.
///
/// Sorted: `read_dir` does not promise an order, and link order is an input to a golden.
fn mindtct_lib_sources(dir: &Path) -> Result<Vec<PathBuf>, String> {
    const CLI_ONLY: [&str; 3] = ["to_type9.c", "update.c", "results.c"];

    let mut sources: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("read {}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "c"))
        .filter(|p| {
            !p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| CLI_ONLY.contains(&n))
        })
        .collect();
    sources.sort();

    if sources.is_empty() {
        return Err(format!("no MINDTCT sources found in {}", dir.display()));
    }
    Ok(sources)
}

/// Score the frozen pairs and freeze the results.
///
/// The sort is here rather than piped through `sort(1)`: shell sort order depends on the
/// container's locale, and this file is a golden.
fn score_bozorth3(root: &Path, binary: &Path, fixtures: &Path) -> Result<(), String> {
    println!("xtask: scoring the corpus");
    let stdout = Run::new(GCC_IMAGE)
        .arg(container_path(&relative_to(root, binary)?))
        .arg(container_path(&relative_to(root, fixtures)?))
        .arg(container_path(&relative_to(
            root,
            &fixtures.join("pairs.txt"),
        )?))
        .output(root)?;

    let mut lines: Vec<&str> = stdout.lines().map(str::trim_end).collect();
    lines.sort_unstable();
    let expected = fixtures.join("expected.tsv");
    let mut body = lines.join("\n");
    body.push('\n');
    std::fs::write(&expected, body).map_err(|e| format!("write {}: {e}", expected.display()))?;

    println!(
        "xtask: wrote {} ({} scores)",
        expected.display(),
        lines.len()
    );
    Ok(())
}

/// Freeze the per-pair stage-1/stage-2 sizes.
///
/// A second run rather than an extra column on [`score_bozorth3`]: that task's stdout *is*
/// `expected.tsv`, so anything more printed there would move a golden. One run, one artifact —
/// which makes `expected.tsv` byte-identical by construction rather than by inspection.
///
/// The triple is what `docs/bozorth3-algorithm.md` claims is bit-identical to the reference on
/// every pair. Frozen here, that claim is checked rather than asserted, and with it the argument
/// that the ±1 score residual is confined to `bz_match_score`.
fn dump_stages_bozorth3(root: &Path, binary: &Path, fixtures: &Path) -> Result<(), String> {
    println!("xtask: dumping the stage-1/stage-2 sizes");
    let stdout = Run::new(GCC_IMAGE)
        .env("BOZORTH3_DUMP_STAGES", "1")
        .arg(container_path(&relative_to(root, binary)?))
        .arg(container_path(&relative_to(root, fixtures)?))
        .arg(container_path(&relative_to(
            root,
            &fixtures.join("pairs.txt"),
        )?))
        .output(root)?;

    let mut lines: Vec<&str> = stdout.lines().map(str::trim_end).collect();
    lines.sort_unstable();
    let stages = fixtures.join("stages.tsv");
    let mut body = lines.join("\n");
    body.push('\n');
    std::fs::write(&stages, body).map_err(|e| format!("write {}: {e}", stages.display()))?;

    println!(
        "xtask: wrote {} ({} triples)",
        stages.display(),
        lines.len()
    );
    Ok(())
}

/// Detect minutiae over the synthetic corpus, dumping the intermediate maps as goldens.
fn detect_mindtct(root: &Path, binary: &Path, fixtures: &Path) -> Result<(), String> {
    println!("xtask: detecting minutiae over the corpus (dumping intermediate maps)");
    Run::new(GCC_IMAGE)
        .env("MINDTCT_DUMP_MAPS", "1")
        .arg(container_path(&relative_to(root, binary)?))
        .arg(container_path(&relative_to(root, fixtures)?))
        .arg(container_path(&relative_to(
            root,
            &fixtures.join("manifest.txt"),
        )?))
        .output(root)?;

    let count = std::fs::read_dir(fixtures)
        .map_err(|e| format!("read {}: {e}", fixtures.display()))?
        .count();
    println!("xtask: {} now holds {count} files", fixtures.display());
    Ok(())
}
