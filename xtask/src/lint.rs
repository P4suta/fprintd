// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Repository rules that no compiler enforces.
//!
//! `cargo fmt`, `clippy` and `reuse` cover the rest; these are the norms specific to this
//! project, checked here so a pre-commit hook can refuse them rather than a reviewer having to
//! notice. See `CONTRIBUTING.md`.
//!
//! Every pattern below matches nothing in the tree as it stands. A check that fires on correct
//! code gets switched off, so precision matters more than coverage: `M1`/`M2` milestone markers
//! are not searched for, because `m1`/`m2` are ordinary variable names in the NBIS ports.

use std::path::{Path, PathBuf};

/// Directories that are not ours to lint.
const SKIP_DIRS: [&str; 4] = ["target", "reference", ".git", "node_modules"];

/// Phrases that date a comment against something the reader cannot see: a past that is gone, or
/// a future nobody promised. Git holds the first; the second is wrong the moment it arrives.
const NARRATION: [&str; 10] = [
    "for now",
    "will grow",
    "used to be",
    "first-cut",
    "we tried",
    "as of the",
    "for the time being",
    "in future",
    "at one point",
    "no longer needed",
];

/// Shell metacharacters. A task runner is not a shell script: `mise.toml` holds one-command
/// tasks, anything longer belongs in this crate, where a compiler reads it.
const SHELL_IN_TASKS: [&str; 5] = ["&&", "||", "bash -c", "set -e", "$("];

struct Finding {
    file: PathBuf,
    line: usize,
    rule: &'static str,
    text: String,
}

pub fn check(root: &Path) -> Result<(), String> {
    let mut findings = Vec::new();
    no_shell_scripts(root, &mut findings)?;
    no_shell_in_tasks(root, &mut findings)?;
    no_narration(root, &mut findings)?;

    if findings.is_empty() {
        println!("xtask: repository rules ok");
        return Ok(());
    }

    let mut msg = format!("{} violation(s):\n", findings.len());
    for f in &findings {
        let path = f.file.strip_prefix(root).unwrap_or(&f.file);
        msg.push_str(&format!(
            "\n  {}:{}\n    {}\n    {}\n",
            path.display().to_string().replace('\\', "/"),
            f.line,
            f.rule,
            f.text.trim()
        ));
    }
    Err(msg)
}

/// No shell scripts. They are read by no compiler and run under whichever shell found them.
fn no_shell_scripts(root: &Path, findings: &mut Vec<Finding>) -> Result<(), String> {
    for path in walk(root)? {
        if path.extension().is_some_and(|e| e == "sh") {
            findings.push(Finding {
                file: path,
                line: 0,
                rule: "shell script — put the logic in xtask/ (CONTRIBUTING.md)",
                text: String::new(),
            });
        }
    }
    Ok(())
}

/// No shell in the task runner or the workflow.
fn no_shell_in_tasks(root: &Path, findings: &mut Vec<Finding>) -> Result<(), String> {
    for rel in ["mise.toml", ".github/workflows/ci.yml"] {
        let path = root.join(rel);
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (i, line) in body.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with('#') {
                continue;
            }
            for pat in SHELL_IN_TASKS {
                if line.contains(pat) {
                    findings.push(Finding {
                        file: path.clone(),
                        line: i + 1,
                        rule: "shell in a task — one command, or put it in xtask/",
                        text: line.to_string(),
                    });
                }
            }
        }
    }
    Ok(())
}

/// No narration about a past or a future the reader cannot check.
fn no_narration(root: &Path, findings: &mut Vec<Finding>) -> Result<(), String> {
    for path in walk(root)? {
        let is_rust = path.extension().is_some_and(|e| e == "rs");
        let is_doc = path.extension().is_some_and(|e| e == "md");
        if !is_rust && !is_doc {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (i, line) in body.lines().enumerate() {
            // In Rust, only comments; a string literal may legitimately say anything.
            if is_rust && !line.trim_start().starts_with("//") {
                continue;
            }
            let lower = line.to_lowercase();
            for pat in NARRATION {
                if lower.contains(pat) {
                    findings.push(Finding {
                        file: path.clone(),
                        line: i + 1,
                        rule: "narration — say what is true now; git holds the history",
                        text: line.to_string(),
                    });
                }
            }
        }
    }
    Ok(())
}

/// Every file under `root`, skipping [`SKIP_DIRS`].
fn walk(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries =
            std::fs::read_dir(&dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if !SKIP_DIRS.contains(&name.as_ref()) {
                    stack.push(path);
                }
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}
