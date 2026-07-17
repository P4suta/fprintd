// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Turning clippy's findings into inline PR annotations, in Rust rather than a shell pipe.
//!
//! GitHub renders a `::warning file=…,line=…::message` line on the workflow's output as an
//! annotation on the diff. Producing those from clippy usually means piping `--message-format=json`
//! through `jq` or `clippy-sarif`, the shell logic this repo keeps out of its workflows. Here it is
//! a program: `cargo clippy --message-format=json` is parsed with the same `cargo_metadata` message
//! reader the tooling already depends on, and the annotations are printed directly. clippy's exit
//! status is preserved, so the CI step still fails on a warning.

use std::io::BufReader;
use std::path::Path;
use std::process::{Command, Stdio};

use cargo_metadata::diagnostic::DiagnosticLevel;
use cargo_metadata::Message;

/// Run clippy over the workspace and emit a GitHub annotation for every warning and error.
pub fn annotate(root: &Path) -> Result<(), String> {
    let mut child = Command::new("cargo")
        .current_dir(root)
        .args([
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--message-format=json",
            "--",
            "-D",
            "warnings",
        ])
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("run cargo clippy: {e}"))?;

    let reader = BufReader::new(child.stdout.take().expect("stdout was piped"));
    let mut count = 0usize;
    for message in Message::parse_stream(reader) {
        let message = message.map_err(|e| format!("read clippy output: {e}"))?;
        let Message::CompilerMessage(compiler) = message else {
            continue;
        };
        let diagnostic = compiler.message;
        let level = match diagnostic.level {
            DiagnosticLevel::Error => "error",
            DiagnosticLevel::Warning => "warning",
            _ => continue,
        };
        let Some(span) = diagnostic.spans.iter().find(|s| s.is_primary) else {
            continue;
        };
        // The rendered form carries the source excerpt and the lint name; a workflow command must be
        // one line, so newlines become the `%0A` GitHub decodes back.
        let body = diagnostic.message.replace('\n', "%0A");
        println!(
            "::{level} file={},line={},col={}::{body}",
            span.file_name, span.line_start, span.column_start,
        );
        count += 1;
    }

    let status = child
        .wait()
        .map_err(|e| format!("wait for cargo clippy: {e}"))?;
    if !status.success() {
        return Err(format!("clippy reported {count} annotated problem(s)"));
    }
    println!("xtask: clippy is clean ({count} annotations)");
    Ok(())
}
