// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The M0 ground-truth measurement: how much C there is, by subsystem.
//!
//! What sizes the shim-vs-native decision is `drivers/` — see `docs/M0-ground-truth.md` and
//! ARCHITECTURE.md §Non-goals, where "an unbounded, device-dependent axis" is this number.

use std::path::Path;
use std::process::Command;

/// Upstream subsystems worth counting separately, and why each one is here.
const SUBSYSTEMS: [(&str, &str); 3] = [
    ("libfprint (whole)", "reference/libfprint/libfprint"),
    (
        "drivers/ — the axis we do not race",
        "reference/libfprint/libfprint/drivers",
    ),
    (
        "nbis/ — the part we ported",
        "reference/libfprint/libfprint/nbis",
    ),
];

pub fn measure(root: &Path) -> Result<(), String> {
    let checkout = root.join("reference/libfprint");
    if !checkout.is_dir() {
        return Err(format!(
            "{} not found — clone the upstream references first (`mise run clone-ref`)",
            checkout.display()
        ));
    }

    for (label, rel) in SUBSYSTEMS {
        let dir = root.join(rel);
        if !dir.is_dir() {
            return Err(format!("{} not found in the checkout", dir.display()));
        }
        // A `println!`, not an `echo` in a task runner: this label used to be single-quoted
        // shell in mise.toml, and printed with its quotes on Windows, where cmd.exe runs it.
        println!("\n== {label} ==");
        let status = Command::new("tokei")
            .arg(&dir)
            .status()
            .map_err(|e| format!("spawn tokei: {e} (is it installed? `mise install`)"))?;
        if !status.success() {
            return Err(format!("tokei failed on {} ({status})", dir.display()));
        }
    }
    Ok(())
}
