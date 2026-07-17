// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Checking the published crates against the registry's rules.
//!
//! `reuse lint` and crates.io differ: REUSE accepts a custom `LicenseRef-*` identifier and
//! crates.io rejects it, so a green lint does not prove a crate can be published. `cargo publish
//! --dry-run` checks the registry's rules at once: the licence expression is one the registry
//! takes, every dependency resolves without a bare `path`, and the packaged tarball builds on its
//! own.
//!
//! The check runs over the whole workspace in one invocation. None of these crates is on the
//! registry yet, so a per-crate run cannot resolve `fprint-core` for `fprint-fp3`. `--workspace`
//! resolves the members against each other, skips every `publish = false` member, and matches the
//! operation a release performs.
//!
//! Two facts are checked against the packaged output rather than the source tree:
//!
//! * **What Cargo strips.** A dev-dependency reaches the published manifest unless its entry omits
//!   `version`. This is Cargo behaviour, not declared in any manifest here.
//! * **What the fixtures promise.** `crates/fprint-bozorth3/REUSE.toml` and its `fprint-mindtct`
//!   sibling ship the goldens so the golden suite runs from the published crate. `cargo package`'s
//!   include/exclude rules decide whether that holds.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use cargo_metadata::MetadataCommand;

/// Crates whose `REUSE.toml` ships the golden fixtures in the tarball, and the file each one's
/// golden suite reads first. A tarball missing these still compiles and passes `cargo publish`.
const SHIPS_FIXTURES: &[(&str, &str)] = &[
    ("fprint-bozorth3", "tests/fixtures/expected.tsv"),
    ("fprint-mindtct", "tests/fixtures/manifest.txt"),
];

/// Check the published crates against the registry's own rules.
pub fn check(root: &Path) -> Result<(), String> {
    println!("xtask: packaging the workspace as the registry would ...");
    let never_published = publish_false_members(root)?;
    release_parity(root, &never_published)?;
    dry_run(root)?;
    stripped_deps_are_absent(root, &never_published)?;
    ships_license_texts(root)?;

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

/// Package and verify every publishable member as `cargo publish` would, without uploading.
///
/// `--locked` checks against the graph the lockfile pins, matching the `msrv` job. `--allow-dirty`
/// because this runs on a working tree; the packaged content is under test, not the git status.
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

/// The workspace members carrying `publish = false`, from the resolved metadata rather than a
/// hardcoded list, so a new `publish = false` crate is covered automatically.
///
/// `cargo_metadata` reports `publish = false` as `Some(vec![])` — an empty allow-list of registries.
fn publish_false_members(root: &Path) -> Result<Vec<String>, String> {
    let metadata = MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .no_deps()
        .exec()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    Ok(metadata
        .workspace_packages()
        .iter()
        .filter(|p| p.publish.as_ref().is_some_and(Vec::is_empty))
        .map(|p| p.name.to_string())
        .collect())
}

/// release-plz must mark exactly the `publish = false` members `release = false`, and no others.
///
/// The two configs state which crates leave the workspace in different files. If they disagree, a
/// release skips a publishable crate or tries to publish one that cannot go. This checks they agree.
fn release_parity(root: &Path, publish_false: &[String]) -> Result<(), String> {
    let path = root.join("release-plz.toml");
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let value: toml::Value =
        toml::from_str(&text).map_err(|e| format!("parse {}: {e}", path.display()))?;

    let mut marked = BTreeSet::new();
    if let Some(packages) = value.get("package").and_then(toml::Value::as_array) {
        for pkg in packages {
            let name = pkg.get("name").and_then(toml::Value::as_str);
            let released = pkg.get("release").and_then(toml::Value::as_bool);
            if let (Some(name), Some(false)) = (name, released) {
                marked.insert(name.to_string());
            }
        }
    }

    let expected: BTreeSet<String> = publish_false.iter().cloned().collect();
    if marked != expected {
        let missing: Vec<_> = expected.difference(&marked).cloned().collect();
        let extra: Vec<_> = marked.difference(&expected).cloned().collect();
        return Err(format!(
            "release-plz.toml and the `publish = false` set disagree.\n  \
             publish = false but not `release = false` in release-plz.toml: {missing:?}\n  \
             `release = false` in release-plz.toml but publishable: {extra:?}"
        ));
    }
    println!("xtask: release-plz.toml holds back exactly the unpublishable crates");
    Ok(())
}

/// The first crate in `never` that `manifest` declares as a dependency, if any.
///
/// Reads dependency table headers rather than searching the text, so a crate named only in a
/// string (a `description`, a `keywords` entry) is not a finding. Cargo's generated manifest gives
/// each dependency its own `[…dependencies.<name>]` table, which this matches.
fn names_unpublished(manifest: &str, never: &[String]) -> Option<String> {
    manifest
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix('[')?.strip_suffix(']'))
        .filter_map(|header| {
            let (table, name) = header.rsplit_once('.')?;
            table.ends_with("dependencies").then_some(name)
        })
        .find_map(|name| never.iter().find(|d| *d == name).cloned())
}

/// No packaged manifest may name a `publish = false` crate.
///
/// Reads what [`dry_run`] generated under `target/package`, the manifest a consumer resolves
/// against, not the source tree. Every packaged crate is checked by walking the directory, so this
/// stays in step with the workspace membership.
fn stripped_deps_are_absent(root: &Path, never: &[String]) -> Result<(), String> {
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
        if let Some(dep) = names_unpublished(&text, never) {
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

/// Every published crate's tarball must carry the licence texts its REUSE.toml relies on, and each
/// shipped copy must be byte-identical to the canonical `LICENSES/` source.
///
/// The tarball ships a crate's `LICENSES/` mirror verbatim, so a shipped file's bytes are the
/// mirror's bytes on disk; the byte check compares that mirror against the workspace-root source.
/// [`sync_licenses`](crate::sync_licenses) writes the mirrors; this fails the build if one is
/// missing from the package or has drifted.
fn ships_license_texts(root: &Path) -> Result<(), String> {
    let canonical = root.join("LICENSES");
    for (krate, ids) in crate::sync_licenses::MIRRORS {
        let listing = package_list(root, krate)?;
        if let Some(id) = missing_license_text(&listing, ids) {
            return Err(format!(
                "{krate}'s package does not contain LICENSES/{id}.txt, so the extracted tarball \
                 cannot satisfy `reuse lint` on its own. Run `cargo xtask sync-licenses`."
            ));
        }
        for id in *ids {
            let name = format!("{id}.txt");
            let source = canonical.join(&name);
            let mirror = root.join("crates").join(krate).join("LICENSES").join(&name);
            let want =
                std::fs::read(&source).map_err(|e| format!("read {}: {e}", source.display()))?;
            let got =
                std::fs::read(&mirror).map_err(|e| format!("read {}: {e}", mirror.display()))?;
            if want != got {
                return Err(format!(
                    "{krate}'s LICENSES/{name} differs from the canonical LICENSES/{name}. Run \
                     `cargo xtask sync-licenses`."
                ));
            }
        }
    }
    println!("xtask: every published crate ships its licence texts, byte-identical to LICENSES/");
    Ok(())
}

/// The first SPDX id in `ids` whose `LICENSES/<id>.txt` the tarball `listing` omits, if any.
fn missing_license_text<'a>(listing: &str, ids: &'a [&str]) -> Option<&'a str> {
    ids.iter().copied().find(|id| {
        let want = format!("LICENSES/{id}.txt");
        !listing.lines().any(|f| f.trim() == want)
    })
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

    /// The `publish = false` names, as [`publish_false_members`] would return them.
    fn never() -> Vec<String> {
        vec![
            "fprint-backend-native".to_string(),
            "fprint-testkit".to_string(),
        ]
    }

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
        assert_eq!(names_unpublished(STRIPPED, &never()), None);
    }

    #[test]
    fn a_crate_named_only_in_prose_is_not_a_dependency() {
        // `description` above says "fprint-backend-native". Searching the text would fire here;
        // reading table headers must not.
        assert!(STRIPPED.contains("fprint-backend-native"));
        assert_eq!(names_unpublished(STRIPPED, &never()), None);
    }

    #[test]
    fn an_unstripped_dev_dependency_is_found() {
        let manifest =
            format!("{STRIPPED}\n[dev-dependencies.fprint-backend-native]\nversion = \"0.1.0\"\n");
        assert_eq!(
            names_unpublished(&manifest, &never()).as_deref(),
            Some("fprint-backend-native")
        );
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
                names_unpublished(&manifest, &never()).as_deref(),
                Some("fprint-testkit"),
                "table `{table}` was not read as a dependency table"
            );
        }
    }

    /// A `cargo package --list` listing carrying both dual-licence texts.
    const WITH_LICENSES: &str = "\
Cargo.toml
LICENSES/Apache-2.0.txt
LICENSES/MIT.txt
src/lib.rs
";

    #[test]
    fn a_listing_with_every_licence_text_is_complete() {
        assert_eq!(
            missing_license_text(WITH_LICENSES, &["MIT", "Apache-2.0"]),
            None
        );
    }

    #[test]
    fn a_missing_licence_text_is_named() {
        assert_eq!(
            missing_license_text(WITH_LICENSES, &["MIT", "Apache-2.0", "LicenseRef-NBIS-PD"]),
            Some("LicenseRef-NBIS-PD")
        );
    }

    #[test]
    fn a_licence_named_only_as_a_path_substring_does_not_count() {
        // A file whose path merely contains the text name must not satisfy the check: the listing
        // entry has to be exactly `LICENSES/<id>.txt`.
        let listing = "src/MIT.txt.rs\ndocs/LICENSES/MIT.txt.bak\n";
        assert_eq!(missing_license_text(listing, &["MIT"]), Some("MIT"));
    }

    #[test]
    fn a_non_dependency_table_is_not_read() {
        // `[lints.clippy.all]` ends in a name, not a dependency; `[package]` has no dot at all.
        let manifest = "[lints.clippy.all]\nlevel = \"deny\"\n\n[package]\nname = \"x\"\n";
        assert_eq!(names_unpublished(manifest, &never()), None);
    }
}
