// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Checking that the crates we publish can be published.
//!
//! `reuse lint` and crates.io are different oracles: REUSE accepts a custom `LicenseRef-*`
//! identifier and crates.io rejects it, so a green lint says nothing about whether a crate can
//! leave the workspace. `cargo publish --dry-run` is the oracle that answers that, and it answers
//! several questions at once — the licence expression is one the registry takes, every dependency
//! resolves without a bare `path`, and the packaged tarball builds on its own.
//!
//! It runs over the whole workspace in one invocation rather than crate by crate. That is not a
//! shortcut: none of these crates is on the registry yet, so a per-crate run cannot resolve
//! `fprint-core` for `fprint-fp3` and fails on a chicken-and-egg rather than on anything real.
//! `--workspace` resolves the members against each other, skips every `publish = false` member on
//! its own, and is the operation a release actually performs.
//!
//! Two further facts are checked against the packaged output rather than the source tree, because
//! the source tree does not state them:
//!
//! * **What Cargo strips.** A dev-dependency reaches the published manifest unless its entry omits
//!   `version`. That is a Cargo behaviour, not something any manifest here declares, and the whole
//!   "invisible to the published crates" argument rests on it.
//! * **What the fixtures promise.** `crates/fprint-bozorth3/REUSE.toml` and its `fprint-mindtct`
//!   sibling say the goldens ship "so the golden suite is runnable straight from the published
//!   crate". `cargo package`'s include/exclude rules decide whether that is true.

use std::path::Path;
use std::process::Command;

/// Crates whose `REUSE.toml` promises the golden fixtures travel with the tarball, and the file
/// each one's golden suite reads first. A tarball missing these still compiles and still passes
/// `cargo publish`; only the promise breaks.
const SHIPS_FIXTURES: &[(&str, &str)] = &[
    ("fprint-bozorth3", "tests/fixtures/expected.tsv"),
    ("fprint-mindtct", "tests/fixtures/manifest.txt"),
];

/// Crates that must never appear in a published manifest. Both are `publish = false` and both are
/// dev-dependencies of a published crate; they are stripped only because their workspace entry
/// omits `version`.
const NEVER_PUBLISHED: &[&str] = &["fprint-backend-native", "fprint-testkit"];

/// Check the published crates against the registry's own rules.
pub fn check(root: &Path) -> Result<(), String> {
    println!("xtask: packaging the workspace as the registry would ...");
    dry_run(root)?;
    stripped_deps_are_absent(root)?;

    for (krate, witness) in SHIPS_FIXTURES {
        let listing = package_list(root, krate)?;
        if !listing.lines().any(|f| f.trim() == *witness) {
            return Err(format!(
                "{krate}'s package does not contain {witness}, so its golden suite cannot run \
                 from the published crate — which is what its REUSE.toml says the fixtures are \
                 shipped for. Check the `include`/`exclude` keys in crates/{krate}/Cargo.toml."
            ));
        }
    }

    println!("xtask: every published crate packages, strips and ships what it claims");
    Ok(())
}

/// Package and verify every publishable member exactly as `cargo publish` would, without
/// uploading.
///
/// `--locked` so the check is against the graph the lockfile pins, matching the `msrv` job.
/// `--allow-dirty` because this runs on a working tree, and it is the packaged content that is
/// under test, not the git status.
fn dry_run(root: &Path) -> Result<(), String> {
    let out = Command::new("cargo")
        .current_dir(root)
        .args([
            "publish",
            "--dry-run",
            "--locked",
            "--allow-dirty",
            "--workspace",
        ])
        .output()
        .map_err(|e| format!("run cargo publish: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "the workspace cannot be published:\n{}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// The first [`NEVER_PUBLISHED`] crate `manifest` declares as a dependency, if any.
///
/// Reads dependency table keys rather than searching the text, so a crate merely *named* in a
/// string — a `description`, a `keywords` entry — is not a finding. Cargo's generated manifest
/// gives each dependency its own `[…dependencies.<name>]` table, which is what this matches.
fn names_unpublished(manifest: &str) -> Option<&'static str> {
    manifest
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix('[')?.strip_suffix(']'))
        .filter_map(|header| {
            let (table, name) = header.rsplit_once('.')?;
            table.ends_with("dependencies").then_some(name)
        })
        .find_map(|name| NEVER_PUBLISHED.iter().copied().find(|d| *d == name))
}

/// No packaged manifest may name a `publish = false` crate.
///
/// Reads what [`dry_run`] just generated under `target/package`, which is the manifest a consumer
/// resolves against — not the one in the source tree. Every packaged crate is checked by walking
/// the directory, so nothing here has to be kept in step with the workspace membership.
fn stripped_deps_are_absent(root: &Path) -> Result<(), String> {
    let dir = root.join("target/package");
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
    let mut checked = 0usize;
    for entry in entries.flatten() {
        let manifest = entry.path().join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&manifest)
            .map_err(|e| format!("read {}: {e}", manifest.display()))?;
        if let Some(dep) = names_unpublished(&text) {
            return Err(format!(
                "a published manifest names `{dep}`, which is `publish = false`. Its workspace \
                 entry in the root Cargo.toml must have a `path` and no `version` — that omission \
                 is what makes Cargo strip it.\n  see {}",
                manifest.display()
            ));
        }
        checked += 1;
    }
    if checked == 0 {
        return Err(format!(
            "no packaged manifests under {} — `cargo publish --dry-run` should have left some \
             there",
            dir.display()
        ));
    }
    println!("xtask: {checked} packaged manifests name no unpublished crate");
    Ok(())
}

/// The files `krate`'s tarball would contain, one per line, relative to the crate root.
fn package_list(root: &Path, krate: &str) -> Result<String, String> {
    let out = Command::new("cargo")
        .current_dir(root)
        .args(["package", "--list", "--allow-dirty", "-p"])
        .arg(krate)
        .output()
        .map_err(|e| format!("run cargo package --list: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo package --list -p {krate} failed:\n{}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape Cargo generates: one table per dependency, name in the header, comments dropped.
    const STRIPPED: &str = "\
[package]
name = \"fprint-fp3\"
description = \"An FP3 codec, tested against fprint-backend-native\"

[dependencies.fprint-core]
version = \"0.1.0\"

[dev-dependencies.fprint-bozorth3]
version = \"0.1.0\"
";

    #[test]
    fn a_stripped_manifest_names_nothing_unpublished() {
        assert_eq!(names_unpublished(STRIPPED), None);
    }

    #[test]
    fn a_crate_named_only_in_prose_is_not_a_dependency() {
        // `description` above says "fprint-backend-native". Searching the text would fire here;
        // reading table headers must not.
        assert!(STRIPPED.contains("fprint-backend-native"));
        assert_eq!(names_unpublished(STRIPPED), None);
    }

    #[test]
    fn an_unstripped_dev_dependency_is_found() {
        let manifest =
            format!("{STRIPPED}\n[dev-dependencies.fprint-backend-native]\nversion = \"0.1.0\"\n");
        assert_eq!(names_unpublished(&manifest), Some("fprint-backend-native"));
    }

    #[test]
    fn every_dependency_table_kind_is_read() {
        for table in [
            "dependencies",
            "dev-dependencies",
            "build-dependencies",
            "target.'cfg(target_os = \"linux\")'.dev-dependencies",
        ] {
            let manifest = format!("[{table}.fprint-testkit]\nversion = \"0.1.0\"\n");
            assert_eq!(
                names_unpublished(&manifest),
                Some("fprint-testkit"),
                "table `{table}` was not read as a dependency table"
            );
        }
    }

    #[test]
    fn a_non_dependency_table_is_not_read() {
        // `[lints.clippy.all]` ends in a name, not a dependency; `[package]` has no dot at all.
        let manifest = "[lints.clippy.all]\nlevel = \"deny\"\n\n[package]\nname = \"x\"\n";
        assert_eq!(names_unpublished(manifest), None);
    }
}
