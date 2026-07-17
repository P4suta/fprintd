// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Running the fuzz targets from a dev box that cannot run them directly.
//!
//! libFuzzer needs nightly and does not work on windows-msvc, so the toolchain lives in a
//! container (`docker/Dockerfile.fuzz`) and the repository is bind-mounted into it. The harness is
//! a separate workspace under `fuzz/`, which keeps `libfuzzer-sys` and `arbitrary` out of the root
//! lockfile — see `fuzz/Cargo.toml`.
//!
//! Fuzzing is **deliberate**, like the NBIS oracles: it needs a nightly toolchain, a network fetch
//! and a wall-clock budget, none of which belong on a CI schedule. What survives a campaign is
//! frozen as an ordinary `#[test]` (`crates/fprint-fp3/tests/regressions.rs`), which needs none of
//! the three.

use std::path::Path;
use std::process::Command;

use crate::docker::{container_path, relative_to, Run};

/// The image built from `docker/Dockerfile.fuzz`. Local to this repository, not published.
const IMAGE: &str = "fprintd-fuzz";

/// A fuzz target, and the corpus it is worth seeding from.
struct Target {
    name: &'static str,
    /// A repository directory of **valid inputs**, handed to libFuzzer as a second, read-only
    /// corpus. [`None`] where no such directory exists: the generator-driven targets read their
    /// input as an `Unstructured` byte stream, and a `.xyt` or an image fixture is not an encoding
    /// of that stream, so seeding them with one would feed the generator noise.
    seeds: Option<&'static str>,
}

/// The five targets, matching `fuzz/fuzz_targets/`.
const TARGETS: &[Target] = &[
    Target {
        name: "fp3_from_bytes",
        seeds: Some("crates/fprint-fp3/tests/fixtures"),
    },
    Target {
        name: "fp3_fixed_point",
        seeds: Some("crates/fprint-fp3/tests/fixtures"),
    },
    Target {
        name: "bozorth3_match",
        seeds: None,
    },
    Target {
        name: "mindtct_detect",
        seeds: None,
    },
    Target {
        name: "capture_import",
        // Valid pcapng/pcap/usbmon captures. The target reads its input as capture bytes, so a
        // fixture is an encoding of that input: seeding lands mutations inside real USB framing.
        seeds: Some("crates/fprint-driverkit/tests/fixtures"),
    },
];

/// Fuzz `name` for `seconds`.
pub fn run(root: &Path, name: &str, seconds: u64) -> Result<(), String> {
    let target = TARGETS
        .iter()
        .find(|t| t.name == name)
        .ok_or_else(|| format!("unknown fuzz target `{name}`\n\n{}", targets()))?;

    build_image(root)?;

    // libFuzzer writes new units to the first corpus directory and will not create it.
    let corpus = root.join("fuzz/corpus").join(target.name);
    std::fs::create_dir_all(&corpus).map_err(|e| format!("create {}: {e}", corpus.display()))?;

    let mut run = Run::new(IMAGE)
        .args(["cargo", "fuzz", "run"])
        // Explicit rather than inferred: the root manifest excludes `fuzz`, so nothing about this
        // directory is discoverable from the workspace it sits next to.
        .args(["--fuzz-dir", &container_path(Path::new("fuzz"))])
        .arg(target.name)
        .arg(container_path(&relative_to(root, &corpus)?));

    if let Some(seeds) = target.seeds {
        // A second corpus directory is **read-only**: libFuzzer takes its units as input and writes
        // new ones only to the first. The frozen fixtures seed a campaign without being copied into
        // it, and without a run ever writing to them.
        run = run.arg(container_path(Path::new(seeds)));
    }

    println!(
        "xtask: fuzzing {} for {seconds}s (output arrives when the run ends)",
        target.name
    );
    let out = run
        .arg("--")
        .arg(format!("-max_total_time={seconds}"))
        .output(root)?;
    print!("{out}");

    println!(
        "xtask: {} survived {seconds}s — corpus in {}",
        target.name,
        corpus.display()
    );
    Ok(())
}

/// Build the fuzzing image. Cheap once cached, and it keeps the task one command for the caller.
///
/// The build context is `docker/` rather than the repository: the image holds a toolchain and no
/// source, and the code under test arrives on the bind mount at run time.
fn build_image(root: &Path) -> Result<(), String> {
    println!("xtask: building the {IMAGE} image (cached after the first run)");
    let status = Command::new("docker")
        .arg("build")
        .arg("-f")
        .arg(root.join("docker/Dockerfile.fuzz"))
        .arg("-t")
        .arg(IMAGE)
        .arg(root.join("docker"))
        .status()
        .map_err(|e| format!("spawn docker: {e} (is Docker running?)"))?;
    if !status.success() {
        return Err(format!("could not build the {IMAGE} image ({status})"));
    }
    Ok(())
}

/// The target list, for the error message.
fn targets() -> String {
    let mut msg = String::from("targets:\n");
    for t in TARGETS {
        msg.push_str(&format!("  {}\n", t.name));
    }
    msg.push_str("\nusage: cargo xtask fuzz <target> [seconds]");
    msg
}
