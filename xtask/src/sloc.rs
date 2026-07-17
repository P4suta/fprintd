// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The M0 ground-truth measurement: how much C there is, by subsystem.
//!
//! `drivers/` sizes the shim-vs-native decision — see `docs/M0-ground-truth.md` and
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
