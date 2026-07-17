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

/// Rhetoric that documentation and comments do without: slogans, metaphor, and defensive asides.
/// Prose states what is true; design rationale goes in an ADR (`docs/adr/`). Each pattern matches
/// nothing in the tree, so a hit is a style regression, not a false positive.
const RHETORIC: [&str; 10] = [
    "an open invitation",
    "not scaffolding",
    "grants without demanding",
    "north star",
    "prime directive",
    "coexistence, not",
    "rewrite war",
    "rewrite race",
    "not a gap",
    "by design, not",
];

/// Shell constructs whose meaning changes with the shell that runs them: command substitution,
/// bash-only `set -e`, and naming a shell that may be absent. Logic that branches or captures
/// output belongs in this crate, where a compiler reads it — never in a task runner or a workflow.
const SHELL_SPECIFIC: [&str; 3] = ["$(", "set -e", "bash -c"];

/// Pure sequencing. `&&`/`||` mean the same in every shell a runner might pick, so a workflow step
/// may chain two single commands with them; `mise.toml`, which has no shell-selection story, may not.
const TASK_RUNNER_ONLY: [&str; 2] = ["&&", "||"];

pub(crate) struct Finding {
    pub(crate) file: PathBuf,
    pub(crate) line: usize,
    pub(crate) rule: &'static str,
    pub(crate) text: String,
}

pub fn check(root: &Path) -> Result<(), String> {
    let mut findings = Vec::new();
    no_shell_scripts(root, &mut findings)?;
    no_shell_in_tasks(root, &mut findings)?;
    no_narration(root, &mut findings)?;
    devcontainer_matches_ci(root, &mut findings)?;
    crate::deps::check(root, &mut findings)?;

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

/// No shell logic in the task runner or any workflow.
///
/// `mise.toml` takes the full ban: a task is one command, and anything longer belongs in this crate.
/// A workflow may chain single commands with `&&`/`||` — those read the same under every shell — but
/// still may not branch, loop, or capture output, so the shell-specific constructs are banned there
/// too. Every workflow is scanned, not just `ci.yml`, so a new one inherits the rule.
fn no_shell_in_tasks(root: &Path, findings: &mut Vec<Finding>) -> Result<(), String> {
    scan_shell(&root.join("mise.toml"), true, findings);

    let workflows = root.join(".github").join("workflows");
    if let Ok(entries) = std::fs::read_dir(&workflows) {
        let mut paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "yml" || x == "yaml"))
            .collect();
        paths.sort();
        for path in paths {
            scan_shell(&path, false, findings);
        }
    }
    Ok(())
}

/// Scan one file for banned shell. `task_runner` adds the `&&`/`||` ban that only `mise.toml` carries.
fn scan_shell(path: &Path, task_runner: bool, findings: &mut Vec<Finding>) {
    let Ok(body) = std::fs::read_to_string(path) else {
        return;
    };
    for (i, line) in body.lines().enumerate() {
        if line.trim_start().starts_with('#') {
            continue;
        }
        let banned = SHELL_SPECIFIC
            .iter()
            .chain(TASK_RUNNER_ONLY.iter().filter(|_| task_runner));
        for pat in banned {
            if line.contains(pat) {
                findings.push(Finding {
                    file: path.to_path_buf(),
                    line: i + 1,
                    rule: "shell in a task or workflow — logic that branches or captures output \
                           goes in xtask; a workflow may only chain single commands with && or ||",
                    text: line.to_string(),
                });
            }
        }
    }
}

/// The devcontainer builds the same image CI does, rather than forking a second one: it must go
/// through `docker/docker-compose.yml`, the base compose the `linux` CI job also builds from.
fn devcontainer_matches_ci(root: &Path, findings: &mut Vec<Finding>) -> Result<(), String> {
    let path = root.join(".devcontainer").join("devcontainer.json");
    let Ok(body) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    if !body.contains("docker/docker-compose.yml") {
        findings.push(Finding {
            file: path,
            line: 1,
            rule: "the devcontainer must reuse docker/docker-compose.yml (the image the linux CI \
                   job builds), not fork a second image",
            text: "docker/docker-compose.yml is not referenced".to_string(),
        });
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
        // A changelog is git's history rendered as prose — the one place dating a line against the
        // past is the point, not a smell. git-cliff generates it, so it cannot be hand-narration.
        if path.file_name().is_some_and(|n| n == "CHANGELOG.md") {
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
            for pat in RHETORIC {
                if lower.contains(pat) {
                    findings.push(Finding {
                        file: path.clone(),
                        line: i + 1,
                        rule: "rhetoric — state what is true; put rationale in an ADR (docs/adr/)",
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
