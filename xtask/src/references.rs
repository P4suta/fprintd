// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Cloning the upstream sources we read and measure against.
//!
//! All of it is git-ignored and none of it is built: `reference/` is for reading the C we speak
//! to, and for the stock NBIS the golden oracles are compiled from.
//!
//! "Already cloned" and "the network is down" are separate outcomes here, because only one of
//! them means the oracles will later fail to find their sources.

use std::path::Path;
use std::process::Command;

/// An upstream checkout, and where to get it.
struct Reference {
    dir: &'static str,
    /// Tried in order, first success wins. A second URL means the first is known to be flaky.
    urls: &'static [&'static str],
    why: &'static str,
}

/// The C we speak to, and the FFI binding the shim depends on.
const UPSTREAM: &[Reference] = &[
    Reference {
        dir: "reference/libfprint",
        // freedesktop's GitLab is often unreachable; 3v1n0's is a maintainer mirror.
        urls: &[
            "https://gitlab.freedesktop.org/libfprint/libfprint.git",
            "https://github.com/3v1n0/libfprint.git",
        ],
        why: "the C library the shim links, and the drivers we do not race",
    },
    Reference {
        dir: "reference/fprintd",
        urls: &["https://gitlab.freedesktop.org/libfprint/fprintd.git"],
        why: "the daemon whose D-Bus contract we implement",
    },
    Reference {
        dir: "reference/libfprint-rs-binding",
        urls: &["https://github.com/AlvaroParker/libfprint-rs.git"],
        why: "the FFI binding the shim depends on (see docs/known-issues.md)",
    },
];

/// Stock, public-domain NIST NBIS: the spec our ports were written from, and the oracle they are
/// checked against.
const NBIS: &[Reference] = &[Reference {
    dir: "reference/nbis-stock",
    urls: &["https://github.com/lessandro/nbis.git"],
    why: "the stock BOZORTH3/MINDTCT the golden corpora come from",
}];

pub fn clone_upstream(root: &Path) -> Result<(), String> {
    clone_all(root, UPSTREAM)
}

pub fn clone_nbis(root: &Path) -> Result<(), String> {
    clone_all(root, NBIS)
}

fn clone_all(root: &Path, refs: &[Reference]) -> Result<(), String> {
    for r in refs {
        let dir = root.join(r.dir);
        if dir.is_dir() {
            println!("xtask: {} already present, skipping", r.dir);
            continue;
        }
        println!("xtask: cloning {} — {}", r.dir, r.why);

        let mut failures = Vec::new();
        let mut cloned = false;
        for url in r.urls {
            match try_clone(url, &dir) {
                Ok(()) => {
                    cloned = true;
                    break;
                }
                Err(e) => failures.push(format!("  {url}\n    {e}")),
            }
        }
        if !cloned {
            return Err(format!(
                "could not clone {}:\n{}",
                r.dir,
                failures.join("\n")
            ));
        }
    }
    Ok(())
}

/// A shallow clone: none of these are read for their history.
fn try_clone(url: &str, dir: &Path) -> Result<(), String> {
    let out = Command::new("git")
        .args(["clone", "--depth", "1", url])
        .arg(dir)
        .output()
        .map_err(|e| format!("spawn git: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
}
