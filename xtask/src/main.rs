// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Developer tasks that are programs rather than shell.
//!
//! `mise.toml` is the right home for a task that is genuinely one command. It is the wrong home
//! for anything else: shell quoted inside TOML is read by no compiler, linter or formatter, and
//! it is run by whatever shell the task runner picked — `cmd.exe` on this project's Windows dev
//! box, `sh` in CI — which are not the same language.
//!
//! Run with `cargo xtask <task>` (see `.cargo/config.toml` for the alias).

mod docker;
mod oracle;
mod references;
mod sloc;
mod unit;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use oracle::Oracle;

fn main() -> ExitCode {
    let root = repo_root();
    let task = std::env::args().nth(1);

    let result = match task.as_deref() {
        Some("unit-verify") => unit::verify(&root),
        Some("sloc") => sloc::measure(&root),
        Some("clone-ref") => references::clone_upstream(&root),
        Some("clone-ref-nbis") => references::clone_nbis(&root),
        Some("bozorth3-oracle") => oracle::regenerate(&root, Oracle::Bozorth3),
        Some("mindtct-oracle") => oracle::regenerate(&root, Oracle::Mindtct),
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

fn usage() -> String {
    [
        "usage: cargo xtask <task>",
        "",
        "tasks:",
        "  unit-verify        check the systemd unit parses, and that Alias= takes the seat",
        "  sloc               M0: measure upstream libfprint by subsystem",
        "  clone-ref          clone the upstream C we read (libfprint, fprintd, the binding)",
        "  clone-ref-nbis     clone stock NIST NBIS, for the golden oracles",
        "  bozorth3-oracle    regenerate the BOZORTH3 goldens from stock NBIS (DELIBERATE)",
        "  mindtct-oracle     regenerate the MINDTCT goldens from stock NBIS (DELIBERATE)",
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
