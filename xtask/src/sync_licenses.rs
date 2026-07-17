// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Mirroring the canonical licence texts into each published crate.
//!
//! `cargo package` ships only a crate's own directory, so a licence text kept in the workspace
//! root `LICENSES/` never reaches the tarball. Each published crate therefore carries its own
//! `LICENSES/` copy: `reuse lint` on an extracted tarball resolves the crate's REUSE.toml against
//! it, and the tarball is self-describing on its own.
//!
//! `LICENSES/` at the workspace root stays the single source of truth. This copies it; the copies
//! are a mechanical mirror, and [`publish::check`](crate::publish::check) fails the build if any
//! shipped copy drifts from its source.

use std::path::Path;

/// The published crates and the SPDX ids whose text each tarball must carry. Every crate ships the
/// dual `MIT`/`Apache-2.0` texts its source headers declare; the two NBIS ports also ship the
/// public-domain text their fixtures' REUSE.toml names.
pub(crate) const MIRRORS: &[(&str, &[&str])] = &[
    ("fprint-core", DUAL),
    ("fprint-fp3", DUAL),
    ("fprint-pipeline", DUAL),
    ("fprint-bozorth3", WITH_NBIS),
    ("fprint-mindtct", WITH_NBIS),
    ("fprint-backend-libfprint", DUAL),
];

/// The workspace licence pair every crate's source declares.
const DUAL: &[&str] = &["MIT", "Apache-2.0"];

/// The pair plus the public-domain text the NBIS golden fixtures reference.
const WITH_NBIS: &[&str] = &["MIT", "Apache-2.0", "LicenseRef-NBIS-PD"];

/// The canonical text for one SPDX id, relative to a `LICENSES/` directory.
fn text_name(id: &str) -> String {
    format!("{id}.txt")
}

/// Copy the canonical `LICENSES/` texts into each published crate's own `LICENSES/`.
pub fn run(root: &Path) -> Result<(), String> {
    let canonical = root.join("LICENSES");
    let mut copied = 0usize;
    for (krate, ids) in MIRRORS {
        let dest_dir = root.join("crates").join(krate).join("LICENSES");
        std::fs::create_dir_all(&dest_dir)
            .map_err(|e| format!("create {}: {e}", dest_dir.display()))?;
        for id in *ids {
            let name = text_name(id);
            let src = canonical.join(&name);
            let dest = dest_dir.join(&name);
            let text = std::fs::read(&src).map_err(|e| format!("read {}: {e}", src.display()))?;
            std::fs::write(&dest, &text).map_err(|e| format!("write {}: {e}", dest.display()))?;
            copied += 1;
        }
    }
    println!("xtask: mirrored {copied} licence texts into the published crates");
    Ok(())
}
