// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Developer tasks written as Rust programs.
//!
//! `mise.toml` holds single-command tasks. Longer tasks live here: shell quoted inside TOML is
//! unchecked by any compiler, linter or formatter, and runs under whatever shell the task runner
//! selects (`cmd.exe` on Windows, `sh` in CI).
//!
//! Run with `cargo xtask <task>` (alias in `.cargo/config.toml`).

#![forbid(unsafe_code)]

mod capture_golden;
mod ci;
mod deps;
mod device_db;
mod docker;
mod driver_check;
mod fuzz;
mod hw_checklist;
mod lint;
mod mutants;
mod oracle;
mod publish;
mod references;
mod sloc;
mod sync_licenses;
mod unit;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use oracle::Oracle;

/// Default fuzzing budget, in seconds.
const FUZZ_SECONDS: u64 = 60;

fn main() -> ExitCode {
    let root = repo_root();
    let mut args = std::env::args().skip(1);
    let task = args.next();

    let result = match task.as_deref() {
        Some("lint") => lint::check(&root),
        Some("ci-annotate") => ci::annotate(&root),
        Some("publish-check") => publish::check(&root),
        Some("sync-licenses") => sync_licenses::run(&root),
        Some("unit-verify") => unit::verify(&root),
        Some("sloc") => sloc::measure(&root),
        Some("clone-ref") => references::clone_upstream(&root),
        Some("clone-ref-nbis") => references::clone_nbis(&root),
        Some("bozorth3-oracle") => oracle::regenerate(&root, Oracle::Bozorth3),
        Some("mindtct-oracle") => oracle::regenerate(&root, Oracle::Mindtct),
        Some("device-db") => device_db::regenerate(&root),
        Some("fuzz") => fuzz_task(&root, args),
        Some("mutants") => mutants::run(&root, args.next()),
        Some("hw-checklist") => hw_checklist_task(&root, args),
        Some("driver-check") => driver_check::run(&root, driver_arg(args)),
        Some("capture-golden") => capture_golden_task(&root, args),
        Some(other) => Err(format!("unknown task `{other}`\n\n{}", usage())),
        None => Err(usage()),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `fuzz <target> [seconds]`.
fn fuzz_task(root: &Path, mut args: impl Iterator<Item = String>) -> Result<(), String> {
    let target = args
        .next()
        .ok_or_else(|| format!("fuzz: which target?\n\n{}", usage()))?;
    let seconds = match args.next() {
        None => FUZZ_SECONDS,
        Some(s) => s
            .parse()
            .map_err(|_| format!("fuzz: `{s}` is not a number of seconds"))?,
    };
    fuzz::run(root, &target, seconds)
}

/// `hw-checklist [driver] [--json]`: an optional driver filter and a `--json` flag, in any order.
fn hw_checklist_task(root: &Path, args: impl Iterator<Item = String>) -> Result<(), String> {
    let mut driver = None;
    let mut json = false;
    for arg in args {
        match arg.as_str() {
            "--json" => json = true,
            other => driver = Some(other.to_string()),
        }
    }
    hw_checklist::run(root, driver, json)
}

/// `capture-golden <driver> <recording-path>`: both arguments are required.
fn capture_golden_task(root: &Path, mut args: impl Iterator<Item = String>) -> Result<(), String> {
    let driver = args
        .next()
        .ok_or_else(|| format!("capture-golden: which driver?\n\n{}", usage()))?;
    let recording = args
        .next()
        .ok_or_else(|| format!("capture-golden: which recording?\n\n{}", usage()))?;
    capture_golden::run(root, &driver, Path::new(&recording))
}

/// The optional positional `[driver]` argument.
fn driver_arg(mut args: impl Iterator<Item = String>) -> Option<String> {
    args.next()
}

fn usage() -> String {
    [
        "usage: cargo xtask <task>",
        "",
        "tasks:",
        "  lint               repository rules a compiler does not enforce",
        "  ci-annotate        run clippy and emit GitHub PR annotations for each finding",
        "  publish-check      check the published crates against the registry's rules",
        "  sync-licenses      mirror LICENSES/ into each published crate (self-describing tarballs)",
        "  unit-verify        check the systemd unit parses, and that Alias= takes the seat",
        "  sloc               M0: measure upstream libfprint by subsystem",
        "  clone-ref          clone the upstream C we read (libfprint, fprintd, the binding)",
        "  clone-ref-nbis     clone stock NIST NBIS, for the golden oracles",
        "  bozorth3-oracle    regenerate the BOZORTH3 goldens from stock NBIS (DELIBERATE)",
        "  mindtct-oracle     regenerate the MINDTCT goldens from stock NBIS (DELIBERATE)",
        "  device-db          regenerate the native device DB from libfprint id-tables (DELIBERATE)",
        "  fuzz <target> [s]  fuzz one target in the nightly container (DELIBERATE; default 60s)",
        "  mutants [base]     which lines the tests do not defend (DELIBERATE; [base] = that diff only)",
        "  hw-checklist       list the pending HW-verified markers ([driver] filters; --json)",
        "  driver-check       run a native driver's acceptance checks ([driver] scopes)",
        "  capture-golden     freeze a driver's recording as a golden fixture (<driver> <recording>)",
    ]
    .join("\n")
}

/// The repository root: this crate's parent directory.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask/ always has a parent")
        .to_path_buf()
}
