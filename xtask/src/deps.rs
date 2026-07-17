// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The one rule, and the crate-level invariants around it, checked against the resolved graph.
//!
//! ARCHITECTURE.md opens with "**dependencies flow only toward the leaves**". `cargo fmt` and
//! `clippy` do not check it; this does. The check reads `cargo metadata`, the resolver's answer,
//! rather than manifest text, because three things it must know are held only by the resolver: a
//! `package = "..."` rename (which crate a dependency really is), a transitive third-party edge
//! into a crate that claims zero dependencies, and the exact set of workspace members.
//!
//! The rules, keyed to the crate they defend:
//!
//! * **R1 — the one rule.** A workspace member's normal dependency on another member must be an
//!   arrow [`ALLOWED`] carries.
//! * **R2 — the charter's dependency-freedom.** A [`ZERO_DEP`] crate declares no normal dependency
//!   of any kind (ARCHITECTURE.md principle 2).
//! * **R3 — no dev cycle.** A dev-dependency ships in nothing, so the one rule does not reach it;
//!   it may not close a cycle, which would state the architecture backwards.
//! * **R4 — the unsafe quarantine.** Every crate but the FFI leaf forbids unsafe (principle 6).
//! * **R5 — the testkit is dev-only.** It is `publish = false` and must reach no shipped artifact.
//! * **R6 — the charter takes no third party.** A [`ZERO_DEP`] crate holds no third-party crate in
//!   *any* table, dev included; the purity is transitive, so it is checked against the graph.
//! * **R7 — total coverage.** Every workspace member has an [`ALLOWED`] row; a new crate cannot slip
//!   past the graph check by being unlisted.
//! * **no external tool is a dependency.** The tools the workspace invokes but never links
//!   ([`NEVER_A_DEP`]) appear in no dependency table.

use std::path::{Path, PathBuf};

use cargo_metadata::{DependencyKind, MetadataCommand};

use crate::lint::Finding;

/// Which workspace crates each crate may name. **Transitively closed**, so a single lookup answers
/// "may `from` name `to`, directly or through anything it names" (see [`reaches`]).
///
/// The rows are the shipped graph. `fprintd` names the shim directly and does not consume
/// `fprint-integration`, matching the code and ARCHITECTURE.md's diagram.
const ALLOWED: &[(&str, &[&str])] = &[
    ("fprint-core", &[]),
    ("fprint-testkit", &[]),
    ("fprint-bozorth3", &[]),
    ("fprint-mindtct", &[]),
    ("xtask", &[]),
    ("fprint-fp3", &["fprint-core"]),
    (
        "fprint-pipeline",
        &["fprint-core", "fprint-bozorth3", "fprint-mindtct"],
    ),
    (
        "fprint-backend-native",
        &[
            "fprint-core",
            "fprint-bozorth3",
            "fprint-mindtct",
            "fprint-pipeline",
        ],
    ),
    (
        "fprint-cli",
        &[
            "fprint-core",
            "fprint-fp3",
            "fprint-bozorth3",
            "fprint-mindtct",
            "fprint-pipeline",
            "fprint-backend-native",
        ],
    ),
    (
        "fprint-driverkit",
        &[
            "fprint-core",
            "fprint-mindtct",
            "fprint-bozorth3",
            "fprint-pipeline",
            "fprint-backend-native",
        ],
    ),
    ("fprint-backend-libfprint", &["fprint-core", "fprint-fp3"]),
    (
        "fprint-integration",
        &[
            "fprint-core",
            "fprint-fp3",
            "fprint-bozorth3",
            "fprint-mindtct",
            "fprint-pipeline",
            "fprint-backend-native",
            "fprint-backend-libfprint",
        ],
    ),
    (
        "fprintd",
        &["fprint-core", "fprint-fp3", "fprint-backend-libfprint"],
    ),
];

/// The charter: crates whose dependency-freedom is a fixed architectural rule.
///
/// `fprint-core` is ARCHITECTURE.md principle 2. The two kernels take their input as an
/// interoperability fact (the xyt triple), so they need no domain model and carry the bit-exact
/// NBIS port. These three take no third-party crate in any table; every other crate may take the
/// dependencies it needs.
const ZERO_DEP: &[&str] = &["fprint-core", "fprint-bozorth3", "fprint-mindtct"];

/// The one crate that may omit `#![forbid(unsafe_code)]`: it is the FFI quarantine
/// (ARCHITECTURE.md principle 6).
const UNSAFE_QUARANTINE: &[&str] = &["fprint-backend-libfprint"];

/// Crates that may be named only from a dev-dependency table.
const DEV_ONLY: &[&str] = &["fprint-testkit"];

/// Tools the workspace invokes as external programs and never links. They read the tree or the
/// lockfile; naming one as a dependency is an error.
const NEVER_A_DEP: &[&str] = &[
    "cargo-nextest",
    "cargo-llvm-cov",
    "cargo-mutants",
    "cargo-deny",
    "release-plz",
    "cargo-release",
    "git-cliff",
    "mdbook",
    "bacon",
    "committed",
];

/// Whether `from` may name `to`, directly or through anything it may name.
///
/// One lookup, because [`ALLOWED`] is transitively closed.
fn reaches(from: &str, to: &str) -> bool {
    ALLOWED
        .iter()
        .find(|(name, _)| *name == from)
        .is_some_and(|(_, allowed)| allowed.contains(&to))
}

/// Check every workspace member against the rules above.
pub fn check(root: &Path, findings: &mut Vec<Finding>) -> Result<(), String> {
    let metadata = MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .exec()
        .map_err(|e| format!("cargo metadata: {e}"))?;

    let members = metadata.workspace_packages();
    let is_member = |name: &str| members.iter().any(|p| p.name == name);

    // R7 — every member has a row, so a new crate cannot escape the graph check by being unlisted.
    for pkg in &members {
        if !ALLOWED.iter().any(|(name, _)| pkg.name == *name) {
            findings.push(Finding {
                file: pkg.manifest_path.clone().into_std_path_buf(),
                line: 1,
                rule: "workspace member has no row in xtask/src/deps.rs ALLOWED — add one so the \
                       one rule reaches it",
                text: pkg.name.to_string(),
            });
        }
    }

    for (krate, allowed) in ALLOWED {
        let Some(pkg) = members.iter().find(|p| p.name == *krate) else {
            continue;
        };
        let manifest_path = pkg.manifest_path.clone().into_std_path_buf();
        let manifest = std::fs::read_to_string(&manifest_path)
            .map_err(|e| format!("read {}: {e}", manifest_path.display()))?;

        for dep in &pkg.dependencies {
            let dep_name = dep.name.as_str();
            let line = line_of(&manifest, dep_name);

            // No external tool is a dependency: the workspace invokes them, it does not link them.
            if NEVER_A_DEP.contains(&dep_name) {
                findings.push(Finding {
                    file: manifest_path.clone(),
                    line,
                    rule: "this is a tool the workspace invokes, not a library — it belongs in \
                           mise.toml or a CI step, not a dependency table",
                    text: dep_name.to_string(),
                });
                continue;
            }

            let on_charter = ZERO_DEP.contains(krate);

            // R2 — a charter crate declares no normal dependency of any kind, workspace or not.
            if on_charter && dep.kind == DependencyKind::Normal {
                findings.push(Finding {
                    file: manifest_path.clone(),
                    line,
                    rule: "this crate's dependency-freedom is architecture (ARCHITECTURE.md \
                           principle 2) — put the dependency in a leaf that may hold it",
                    text: dep_name.to_string(),
                });
                continue;
            }

            let is_workspace_crate = is_member(dep_name);

            // R6 — a charter crate holds no third-party crate in any table, dev included.
            if on_charter && !is_workspace_crate {
                findings.push(Finding {
                    file: manifest_path.clone(),
                    line,
                    rule: "a charter crate takes no third-party dependency in any table (the \
                           purity is the product) — move the test that needs it to a crate off \
                           the charter",
                    text: dep_name.to_string(),
                });
                continue;
            }

            if !is_workspace_crate {
                continue;
            }

            // R5 — the testkit is a dev-dependency and nothing else.
            if DEV_ONLY.contains(&dep_name) && dep.kind == DependencyKind::Normal {
                findings.push(Finding {
                    file: manifest_path.clone(),
                    line,
                    rule: "this crate is a dev-dependency and nothing else — it is `publish = \
                           false` and must reach no shipped artifact",
                    text: dep_name.to_string(),
                });
                continue;
            }

            match dep.kind {
                // R1 — the one rule. A normal dependency must be an arrow the shipped graph has.
                DependencyKind::Normal if !allowed.contains(&dep_name) => findings.push(Finding {
                    file: manifest_path.clone(),
                    line,
                    rule: "dependency points back up — lift the coupling to the integration crate \
                           (ARCHITECTURE.md, the one rule)",
                    text: dep_name.to_string(),
                }),
                // R3 — a dev-dependency may not close a cycle: a crate whose tests are written in
                // the terms of something that depends on it has inverted, whatever the tarball holds.
                DependencyKind::Development if reaches(dep_name, krate) => findings.push(Finding {
                    file: manifest_path.clone(),
                    line,
                    rule: "dev-dependency closes a cycle — this crate is below the one it is \
                           testing with, so the test states the architecture backwards",
                    text: dep_name.to_string(),
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

/// The 1-based line `dep` is declared on, for the finding's location.
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

    #[test]
    fn the_matrix_is_transitively_closed() {
        // The claim [`reaches`] rests on: if A may name B, then A may name everything B may.
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
    fn every_charter_crate_has_an_empty_row() {
        // The two rules must agree: a crate that may depend on nothing has nothing in its row.
        for krate in ZERO_DEP {
            let (_, allowed) = ALLOWED
                .iter()
                .find(|(name, _)| name == krate)
                .unwrap_or_else(|| panic!("{krate} is on the charter but has no ALLOWED row"));
            assert!(
                allowed.is_empty(),
                "{krate} is on the charter but may name {allowed:?}"
            );
        }
    }

    #[test]
    fn a_dev_dependency_is_a_cycle_only_when_the_target_can_reach_back() {
        // The current arrows, and why each is fine. A dev-dependency ships in nothing, so the one
        // rule does not reach it; it may not close a cycle.
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
