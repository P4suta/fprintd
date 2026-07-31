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
    /// The tag or branch to check out, when the tree feeds something committed. `None` takes the
    /// remote's default branch, which is right for a tree we only read.
    rev: Option<&'static str>,
    why: &'static str,
}

/// The libfprint release the device database is generated from.
///
/// Pinned because `cargo xtask device-db` turns this tree's driver id-tables into a **committed**
/// file, so an unpinned clone makes that file's content depend on the day it was regenerated:
/// measured on 2026-07-31, `v1.94.10` yields 237 rows and the then-current `main` yields 259.
/// The module doc on `device_db.rs` promises regeneration leaves no diff on a clean tree, and only
/// a pin can keep that promise. Bump this deliberately, regenerate, and review the diff.
///
/// Deliberately *not* tied to `docker/Dockerfile`'s `LIBFPRINT_REF`: that pin exists to keep the
/// shim's bindgen output deterministic against the library we link, which is a different question
/// from which devices libfprint has learned about.
const LIBFPRINT_REV: &str = "v1.94.100";

/// The C we speak to, and the FFI binding the shim depends on.
const UPSTREAM: &[Reference] = &[
    Reference {
        dir: "reference/libfprint",
        // Only freedesktop's GitLab, and deliberately no fallback. No GitHub mirror carries the
        // pinned tag, and a stale one would be worse than none: this tree is what `device-db`
        // reads id-tables out of, so a fallback quietly supplying an older driver set would
        // regenerate a plausible-looking table missing dozens of devices. Failing the clone is the
        // safe outcome.
        urls: &["https://gitlab.freedesktop.org/libfprint/libfprint.git"],
        rev: Some(LIBFPRINT_REV),
        why: "the C library the shim links, and the drivers we do not race",
    },
    Reference {
        dir: "reference/fprintd",
        urls: &["https://gitlab.freedesktop.org/libfprint/fprintd.git"],
        // Unpinned: read by humans comparing our D-Bus surface against upstream's, and it feeds
        // nothing generated. The newest description of the contract is the useful one.
        rev: None,
        why: "the daemon whose D-Bus contract we implement",
    },
    Reference {
        dir: "reference/libfprint-rs-binding",
        urls: &["https://github.com/AlvaroParker/libfprint-rs.git"],
        rev: None,
        why: "the FFI binding the shim depends on (see docs/known-issues.md)",
    },
];

/// Stock, public-domain NIST NBIS: the spec our ports were written from, and the oracle they are
/// checked against.
const NBIS: &[Reference] = &[Reference {
    dir: "reference/nbis-stock",
    urls: &["https://github.com/lessandro/nbis.git"],
    // Unpinned: a snapshot of a long-frozen NIST release, not a moving target.
    rev: None,
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
            // "Already present" must not quietly mean "at some other revision": a stale checkout
            // is exactly how an unpinned tree used to leak into a committed file. Say which one it
            // is, and complain if a pinned reference is not sitting on its pin.
            match (r.rev, describe(&dir)) {
                (Some(want), Some(have)) if have != want => {
                    return Err(format!(
                        "{} is checked out at {have}, but this tree is pinned to {want}.\n\
                         Delete it and re-run to get the pinned revision:\n  rm -rf {}",
                        r.dir,
                        dir.display()
                    ));
                }
                (Some(want), _) => println!("xtask: {} already present at {want}, skipping", r.dir),
                (None, _) => println!("xtask: {} already present, skipping", r.dir),
            }
            continue;
        }
        match r.rev {
            Some(rev) => println!("xtask: cloning {} at {rev} — {}", r.dir, r.why),
            None => println!("xtask: cloning {} — {}", r.dir, r.why),
        }

        let mut failures = Vec::new();
        let mut cloned = false;
        for url in r.urls {
            match try_clone(url, &dir, r.rev) {
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

/// The tag a checkout sits on, if it sits exactly on one.
fn describe(dir: &Path) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["describe", "--tags", "--exact-match"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// A shallow clone: none of these are read for their history. `rev` is a tag or branch; `git
/// clone --branch` takes either, and with `--depth 1` fetches only that commit.
fn try_clone(url: &str, dir: &Path, rev: Option<&str>) -> Result<(), String> {
    let mut cmd = Command::new("git");
    cmd.args(["clone", "--depth", "1"]);
    if let Some(rev) = rev {
        cmd.args(["--branch", rev]);
    }
    let out = cmd
        .arg(url)
        .arg(dir)
        .output()
        .map_err(|e| format!("spawn git: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
}
