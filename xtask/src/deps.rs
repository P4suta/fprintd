// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The one rule, and the three crate-level invariants around it, checked against the manifests.
//!
//! ARCHITECTURE.md opens with "**dependencies flow only toward the leaves**" and CONTRIBUTING.md
//! calls it "the core norm for every change". `cargo fmt` and `clippy` cannot see it, and until now
//! neither could anything else — the project's supreme rule was held by review alone, while its
//! lesser ones (no shell, no narration) were machine-enforced.
//!
//! ## Why a line scan is enough
//!
//! This reads `[dependencies]` table keys and nothing else. No TOML parser and no `cargo metadata`
//! JSON: hand-writing a parser to avoid a parser dependency is the cleverness this repo rejects,
//! and a parser bug fires on correct code, which [`crate::lint`] predicts the fate of.
//!
//! A line scan is not weaker than a full-graph check, because [`ALLOWED`] is **transitively
//! closed** and **every** crate's manifest is read. Any transitive upward edge `A ⇝ B` decomposes
//! into declared edges, so some `C → B` lies on the path. Either `C`'s row permits `B` — and then
//! transitive closure means `A`'s row permits it too, so there was nothing to catch — or it does
//! not, and the scan fires on `C`'s manifest. Workspace crates are reachable only by `path`, so no
//! external crate can smuggle an arrow back in.
//!
//! Anything unparsed is silence, not an alarm: precision over coverage.

use std::path::{Path, PathBuf};

use crate::lint::Finding;

/// Which workspace crates each crate may name. **Transitively closed** — see the module docs.
///
/// The rows are the shipped graph, which is the graph the rule is about. `fprintd` names the shim
/// directly and does not consume `fprint-integration`; that is what the code does, and
/// ARCHITECTURE.md's diagram says so.
const ALLOWED: &[(&str, &[&str])] = &[
    ("fprint-core", &[]),
    ("fprint-testkit", &[]),
    ("fprint-bozorth3", &[]),
    ("fprint-mindtct", &[]),
    ("xtask", &[]),
    ("fprint-fp3", &["fprint-core"]),
    (
        "fprint-backend-native",
        &["fprint-core", "fprint-bozorth3", "fprint-mindtct"],
    ),
    ("fprint-backend-libfprint", &["fprint-core", "fprint-fp3"]),
    (
        "fprint-integration",
        &[
            "fprint-core",
            "fprint-fp3",
            "fprint-bozorth3",
            "fprint-mindtct",
            "fprint-backend-native",
            "fprint-backend-libfprint",
        ],
    ),
    (
        "fprintd",
        &["fprint-core", "fprint-fp3", "fprint-backend-libfprint"],
    ),
];

/// Crates whose dependency-freedom is architecture rather than circumstance.
///
/// `fprint-core` is ARCHITECTURE.md principle 2. The two kernels take their input as an
/// interoperability fact so they need no domain model. `xtask` is in the lockfile every published
/// crate resolves against. `fprint-testkit` must stay free so it cannot cycle with the tests of the
/// crates it feeds.
const ZERO_DEP: &[&str] = &[
    "fprint-core",
    "fprint-bozorth3",
    "fprint-mindtct",
    "fprint-testkit",
    "xtask",
];

/// The one crate that may omit `#![forbid(unsafe_code)]`: it is the FFI quarantine
/// (ARCHITECTURE.md principle 6).
const UNSAFE_QUARANTINE: &[&str] = &["fprint-backend-libfprint"];

/// Crates that may be named only from a dev-dependency table.
const DEV_ONLY: &[&str] = &["fprint-testkit"];

/// Which table a manifest line sits under.
#[derive(Clone, Copy, PartialEq)]
enum Table {
    /// `[dependencies]` or `[build-dependencies]`, plain or under a `[target.'…']`.
    Normal,
    /// `[dev-dependencies]`, plain or under a `[target.'…']`.
    Dev,
    /// Anything else: `[package]`, `[lints]`, `[features]`, …
    Other,
}

/// Classify a `[…]` header. `[target.'cfg(…)'.dev-dependencies]` is a dev table; the quoted cfg may
/// itself contain dots and brackets, so the suffix decides rather than a split.
fn table_of(header: &str) -> Table {
    if header.ends_with("dev-dependencies") {
        Table::Dev
    } else if header.ends_with("dependencies") {
        Table::Normal
    } else {
        Table::Other
    }
}

/// The dependency names a manifest declares, as `(name, table)`.
///
/// Reads both shapes Cargo accepts: a key in a dependency table (`fprint-core = { … }`) and a
/// table of its own (`[dependencies.fprint-core]`). `[features]` is `Table::Other`, so a
/// `dep:fprint-x` there is never read as a dependency.
fn declared(manifest: &str) -> Vec<(&str, Table)> {
    let mut out = Vec::new();
    let mut table = Table::Other;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('#') {
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            table = table_of(header);
            // `[dependencies.fprint-core]` declares one dependency and opens *its* table, whose
            // keys are that dependency's fields (`version`, `workspace`, …) and not more
            // dependencies. So the name is taken here and the table that follows is not read.
            if table == Table::Other {
                if let Some((prefix, name)) = header.rsplit_once('.') {
                    let t = table_of(prefix);
                    if t != Table::Other {
                        out.push((name, t));
                    }
                }
            }
            continue;
        }
        if table == Table::Other {
            continue;
        }
        if let Some((key, _)) = line.split_once('=') {
            let key = key.trim();
            if !key.is_empty() && !key.contains(' ') {
                out.push((key, table));
            }
        }
    }
    out
}

/// Whether `from` may name `to`, directly or through anything it may name.
///
/// One lookup, because [`ALLOWED`] is transitively closed — the same property the module docs rest
/// on, doing a second job here.
fn reaches(from: &str, to: &str) -> bool {
    ALLOWED
        .iter()
        .find(|(name, _)| *name == from)
        .is_some_and(|(_, allowed)| allowed.contains(&to))
}

/// Check every crate's manifest against the rules above.
pub fn check(root: &Path, findings: &mut Vec<Finding>) -> Result<(), String> {
    for (krate, allowed) in ALLOWED {
        let manifest_path = manifest_of(root, krate);
        let manifest = std::fs::read_to_string(&manifest_path)
            .map_err(|e| format!("read {}: {e}", manifest_path.display()))?;

        for (dep, table) in declared(&manifest) {
            let line = line_of(&manifest, dep);

            // R2 — a zero-dependency crate has no normal dependency of any kind, workspace or not.
            if table == Table::Normal && ZERO_DEP.contains(krate) {
                findings.push(Finding {
                    file: manifest_path.clone(),
                    line,
                    rule: "this crate's dependency-freedom is architecture (ARCHITECTURE.md \
                           principle 2) — put the dependency in a leaf that may hold it",
                    text: dep.to_string(),
                });
                continue;
            }

            let is_workspace_crate = ALLOWED.iter().any(|(name, _)| *name == dep);
            if !is_workspace_crate {
                continue;
            }

            // R5 — the testkit is a dev-dependency and nothing else.
            if DEV_ONLY.contains(&dep) && table == Table::Normal {
                findings.push(Finding {
                    file: manifest_path.clone(),
                    line,
                    rule: "this crate is a dev-dependency and nothing else — it is `publish = \
                           false` and must reach no shipped artifact",
                    text: dep.to_string(),
                });
                continue;
            }

            match table {
                // R1 — the one rule. A normal dependency must be an arrow the shipped graph has.
                Table::Normal if !allowed.contains(&dep) => findings.push(Finding {
                    file: manifest_path.clone(),
                    line,
                    rule: "dependency points back up — lift the coupling to the integration crate \
                           (ARCHITECTURE.md, the one rule)",
                    text: dep.to_string(),
                }),
                // R3 — a dev-dependency ships in nothing, so the one rule does not reach it. What
                // it may not do is close a cycle: a crate whose tests are written in the terms of
                // something that depends on it has inverted, whatever the tarball contains.
                Table::Dev if reaches(dep, krate) => findings.push(Finding {
                    file: manifest_path.clone(),
                    line,
                    rule: "dev-dependency closes a cycle — this crate is below the one it is \
                           testing with, so the test states the architecture backwards",
                    text: dep.to_string(),
                }),
                _ => {}
            }
        }

        // R4 — `unsafe` is quarantined to the leaves (ARCHITECTURE.md principle 6).
        if !UNSAFE_QUARANTINE.contains(krate) {
            let src = crate_dir(root, krate).join("src");
            let entry = ["lib.rs", "main.rs"]
                .iter()
                .map(|f| src.join(f))
                .find(|p| p.is_file())
                .ok_or_else(|| format!("{krate}: no src/lib.rs or src/main.rs"))?;
            let text = std::fs::read_to_string(&entry)
                .map_err(|e| format!("read {}: {e}", entry.display()))?;
            if !text.contains("#![forbid(unsafe_code)]") {
                findings.push(Finding {
                    file: entry.clone(),
                    line: 1,
                    rule: "every crate but the FFI quarantine forbids unsafe (ARCHITECTURE.md \
                           principle 6)",
                    text: "#![forbid(unsafe_code)] is missing".to_string(),
                });
            }
        }
    }
    Ok(())
}

fn crate_dir(root: &Path, krate: &str) -> PathBuf {
    if krate == "xtask" {
        root.join("xtask")
    } else {
        root.join("crates").join(krate)
    }
}

fn manifest_of(root: &Path, krate: &str) -> PathBuf {
    crate_dir(root, krate).join("Cargo.toml")
}

/// The 1-based line `dep` is declared on, for a message that points somewhere.
fn line_of(manifest: &str, dep: &str) -> usize {
    manifest
        .lines()
        .position(|l| {
            let l = l.trim();
            l.starts_with(dep) || (l.starts_with('[') && l.contains(dep))
        })
        .map_or(1, |i| i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(manifest: &str) -> Vec<(&str, bool)> {
        declared(manifest)
            .into_iter()
            .map(|(n, t)| (n, t == Table::Dev))
            .collect()
    }

    #[test]
    fn a_dependency_key_is_read_with_its_table() {
        let m = "\
[package]
name = \"x\"

[dependencies]
fprint-core = { workspace = true }

[dev-dependencies]
fprint-testkit = { workspace = true }
";
        assert_eq!(names(m), [("fprint-core", false), ("fprint-testkit", true)]);
    }

    #[test]
    fn a_target_table_is_read_and_keeps_its_kind() {
        // The quoted cfg contains dots and brackets, so the suffix must decide, not a split.
        let m = "\
[target.'cfg(target_os = \"linux\")'.dependencies]
fprint-core = { workspace = true }

[target.'cfg(target_os = \"linux\")'.dev-dependencies]
fprint-backend-native = { workspace = true }
";
        assert_eq!(
            names(m),
            [("fprint-core", false), ("fprint-backend-native", true)]
        );
    }

    #[test]
    fn a_dependency_with_its_own_table_is_read_once() {
        // The shape `cargo publish` generates. The keys under it are the dependency's fields, so
        // reading them as dependencies too would make `version` a crate.
        let m = "[dev-dependencies.fprint-bozorth3]\nversion = \"0.1.0\"\nfeatures = []\n";
        assert_eq!(names(m), [("fprint-bozorth3", true)]);
    }

    #[test]
    fn a_dep_prefixed_feature_is_not_a_dependency() {
        // `[features]` is not a dependency table, so `dep:` syntax inside it must stay invisible.
        let m = "[features]\nusb = [\"dep:nusb\"]\nhwtest = []\n";
        assert!(names(m).is_empty());
    }

    #[test]
    fn comments_and_other_tables_are_not_dependencies() {
        let m = "\
[package]
name = \"fprint-fp3\"

[dependencies]
# fprint-backend-native = { workspace = true }
fprint-core = { workspace = true }

[lints]
workspace = true

[lints.clippy.all]
level = \"deny\"
";
        assert_eq!(names(m), [("fprint-core", false)]);
    }

    #[test]
    fn the_matrix_is_transitively_closed() {
        // The claim the module docs rest on: if A may name B, then A may name everything B may.
        // Without this, a per-manifest scan would not equal a full-graph check.
        for (krate, allowed) in ALLOWED {
            for dep in *allowed {
                let (_, deps_of_dep) = ALLOWED
                    .iter()
                    .find(|(name, _)| name == dep)
                    .unwrap_or_else(|| panic!("{krate} may name {dep}, which has no row"));
                for transitive in *deps_of_dep {
                    assert!(
                        allowed.contains(transitive),
                        "{krate} may name {dep}, which may name {transitive} — but {krate} may \
                         not. ALLOWED must be transitively closed."
                    );
                }
            }
        }
    }

    #[test]
    fn no_crate_may_name_itself() {
        for (krate, allowed) in ALLOWED {
            assert!(!allowed.contains(krate), "{krate} names itself");
        }
    }

    #[test]
    fn every_zero_dep_crate_has_an_empty_row() {
        // The two rules must agree: a crate that may depend on nothing has nothing in its row.
        for krate in ZERO_DEP {
            let (_, allowed) = ALLOWED
                .iter()
                .find(|(name, _)| name == krate)
                .unwrap_or_else(|| panic!("{krate} is ZERO_DEP but has no ALLOWED row"));
            assert!(
                allowed.is_empty(),
                "{krate} is ZERO_DEP but may name {allowed:?}"
            );
        }
    }

    #[test]
    fn a_dev_dependency_is_a_cycle_only_when_the_target_can_reach_back() {
        // The arrows that exist today, and why each is fine. A dev-dependency ships in nothing, so
        // the one rule does not reach it; the only thing it may not do is close a cycle.
        assert!(
            !reaches("fprint-bozorth3", "fprint-fp3"),
            "fp3 -> bozorth3 (dev)"
        );
        assert!(
            !reaches("fprint-backend-native", "fprint-fp3"),
            "fp3 -> backend-native (dev): backend-native names core, bozorth3 and mindtct, not fp3"
        );
        assert!(
            !reaches("fprint-backend-native", "fprintd"),
            "fprintd -> backend-native (dev)"
        );
        assert!(
            !reaches("fprint-testkit", "fprint-core"),
            "core -> testkit (dev)"
        );

        // And the arrow the rule exists to refuse: the domain model tested in an implementor's
        // terms. `fprint-core` needs no exception row for this — it falls out of the graph.
        assert!(
            reaches("fprint-backend-native", "fprint-core"),
            "core -> backend-native (dev) must read as a cycle"
        );
        assert!(
            reaches("fprint-fp3", "fprint-core"),
            "core -> fp3 (dev) must read as a cycle"
        );
    }
}
