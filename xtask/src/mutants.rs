// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Mutation testing: which lines the tests do not defend.
//!
//! `cargo test` green says the goldens pass. It does not say a golden would catch a stage that
//! stopped doing its work. `crates/fprint-mindtct/tests/corpus_adequacy.rs` makes that argument for
//! one file by hand; cargo-mutants makes it for every line by deleting the line and watching for a
//! test failure. Scope and build settings are in `.cargo/mutants.toml`; this file holds what the
//! command line cannot state declaratively.
//!
//! Two runs, one task:
//!
//! * no argument — every mutant in the published crates. DELIBERATE: 5,784 of them, hours.
//! * a base ref  — only mutants on lines the branch touched, which is what CI runs on a pull
//!   request. A `git diff` plus a `cargo mutants` is two commands and a file between them, so it is
//!   a program rather than a line of `mise.toml`.
//!
//! ## What a finding means
//!
//! A surviving mutant is untested code, not broken code. Nothing here gates a merge; this is a
//! report. The task still exits non-zero when mutants survive, so it does not misreport its result.
//! CI takes the decision not to gate, in `.github/workflows/ci.yml`.

use std::path::Path;
use std::process::Command;

/// Where the branch diff is handed to `--in-diff`. Under `target/`, which is git-ignored, and a
/// file rather than a pipe because a redirect would put a shell between the two commands.
const DIFF: &str = "target/mutants-diff.patch";

/// Jobs per core. cargo-mutants runs one `cargo test` per job and defaults to one job, which leaves
/// a 16-core box testing 5,784 mutants on one core. Half the cores: each job's `cargo test` spawns
/// its own threads, and cargo-mutants' jobserver caps the total at the core count, so the remaining
/// cores run those threads.
const JOBS_PER_CORE: usize = 2;

/// Mutation-test the published crates; `base` restricts it to the lines changed since that ref.
pub fn run(root: &Path, base: Option<String>) -> Result<(), String> {
    let jobs = jobs();
    let mut cmd = Command::new("cargo");
    cmd.arg("mutants").arg("-j").arg(jobs.to_string());
    cmd.current_dir(root);

    match &base {
        None => println!("xtask: mutating the published crates on {jobs} jobs (this takes hours)"),
        Some(base) => {
            let path = write_diff(root, base)?;
            cmd.arg("--in-diff").arg(&path);
            println!("xtask: mutating what changed since {base}, on {jobs} jobs");
        }
    }

    let status = cmd
        .status()
        .map_err(|e| format!("spawn cargo-mutants: {e} (cargo install cargo-mutants)"))?;
    if !status.success() {
        // cargo-mutants has already printed the survivors; no need to repeat them.
        return Err(format!("mutants survived, or the run failed ({status})"));
    }
    Ok(())
}

/// Write the diff from `base`'s merge base to the working tree into [`DIFF`], and return the path.
///
/// From the **merge base**, not from `base` itself: the base branch moves while a pull request
/// sits, and a plain two-dot diff would hand cargo-mutants every line the base gained in the
/// meantime as if this branch had touched it.
///
/// To the **working tree**, not to `HEAD`: cargo-mutants applies the diff's line numbers to the
/// source on disk and refuses the run when the two disagree, so a diff that stops at `HEAD` is a
/// diff that breaks the moment a line is uncommitted. In CI the tree is clean and the two are the
/// same thing; on a dev box only one of them is.
fn write_diff(root: &Path, base: &str) -> Result<String, String> {
    let merge_base = git(root, &["merge-base", base, "HEAD"])?;
    let merge_base = merge_base.trim();
    let diff = git(root, &["diff", merge_base])?;
    if diff.trim().is_empty() {
        return Err(format!("nothing changed since {base} — no mutants to test"));
    }

    let path = root.join(DIFF);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    std::fs::write(&path, &diff).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path.display().to_string())
}

/// Run `git` with `args` and return its stdout.
fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| format!("spawn git: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("git {}: {}", args.join(" "), err.trim()));
    }
    String::from_utf8(out.stdout).map_err(|e| format!("git {}: {e}", args.join(" ")))
}

/// Jobs to run in parallel, from the machine this is on.
fn jobs() -> usize {
    std::thread::available_parallelism()
        .map(|n| (n.get() / JOBS_PER_CORE).max(1))
        .unwrap_or(1)
}
