// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Developer tasks that are programs rather than shell.
//!
//! `mise.toml` holds tasks that are genuinely one command. Anything longer lives here: shell
//! quoted inside TOML is read by no compiler, linter or formatter, and it runs under whatever
//! shell the task runner picked — `cmd.exe` on a Windows dev box, `sh` in CI — which are not the
//! same language.
//!
//! Run with `cargo xtask <task>` (see `.cargo/config.toml` for the alias).

#![forbid(unsafe_code)]

mod deps;
mod docker;
mod fuzz;
mod lint;
mod mutants;
mod oracle;
mod publish;
mod references;
mod sloc;
mod unit;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use oracle::Oracle;

/// Default fuzzing budget, in seconds. Long enough to be a campaign, short enough to be a command
/// someone runs while waiting.
const FUZZ_SECONDS: u64 = 60;

fn main() -> ExitCode {
    let root = repo_root();
    let mut args = std::env::args().skip(1);
    let task = args.next();

    let result = match task.as_deref() {
        Some("lint") => lint::check(&root),
        Some("publish-check") => publish::check(&root),
        Some("unit-verify") => unit::verify(&root),
        Some("sloc") => sloc::measure(&root),
        Some("clone-ref") => references::clone_upstream(&root),
        Some("clone-ref-nbis") => references::clone_nbis(&root),
        Some("bozorth3-oracle") => oracle::regenerate(&root, Oracle::Bozorth3),
        Some("mindtct-oracle") => oracle::regenerate(&root, Oracle::Mindtct),
        Some("fuzz") => fuzz_task(&root, args),
        Some("mutants") => mutants::run(&root, args.next()),
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

/// `fuzz <target> [seconds]`: the one task that takes arguments.
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

fn usage() -> String {
    [
        "usage: cargo xtask <task>",
        "",
        "tasks:",
        "  lint               repository rules a compiler does not enforce",
        "  publish-check      check the published crates against the registry's rules",
        "  unit-verify        check the systemd unit parses, and that Alias= takes the seat",
        "  sloc               M0: measure upstream libfprint by subsystem",
        "  clone-ref          clone the upstream C we read (libfprint, fprintd, the binding)",
        "  clone-ref-nbis     clone stock NIST NBIS, for the golden oracles",
        "  bozorth3-oracle    regenerate the BOZORTH3 goldens from stock NBIS (DELIBERATE)",
        "  mindtct-oracle     regenerate the MINDTCT goldens from stock NBIS (DELIBERATE)",
        "  fuzz <target> [s]  fuzz one target in the nightly container (DELIBERATE; default 60s)",
        "  mutants [base]     which lines the tests do not defend (DELIBERATE; [base] = that diff only)",
    ]
    .join("\n")
}

/// The repository root: this crate's directory, minus the crate.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask/ always has a parent")
        .to_path_buf()
}
