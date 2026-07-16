// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Print the BOZORTH3 match score between two `.xyt` minutiae files.
//!
//! ```text
//! cargo run -p fprint-bozorth3 --example match_xyt -- <probe.xyt> <gallery.xyt>
//! ```
//!
//! Each `.xyt` file holds one minutia per line as whitespace-separated integers `x y theta`
//! (extra columns, such as a quality value, are ignored) — the interchange form the stock NBIS
//! tools read and write. A higher score means more corresponding ridge structure; the caller
//! picks a match threshold.

use std::path::Path;
use std::process::ExitCode;

use fprint_bozorth3::{match_score, Minutia, MIN_COMPUTABLE_BOZORTH_MINUTIAE};

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let (Some(probe_path), Some(gallery_path), None) = (args.next(), args.next(), args.next())
    else {
        eprintln!("usage: match_xyt <probe.xyt> <gallery.xyt>");
        return ExitCode::from(2);
    };

    let probe = match load_xyt(probe_path.as_ref()) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let gallery = match load_xyt(gallery_path.as_ref()) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    for (label, set) in [("probe", &probe), ("gallery", &gallery)] {
        if set.len() < MIN_COMPUTABLE_BOZORTH_MINUTIAE {
            eprintln!(
                "warning: {label} has {} minutiae, below the {MIN_COMPUTABLE_BOZORTH_MINUTIAE} \
                 needed to compute a score — the result will be 0",
                set.len()
            );
        }
    }

    println!("{}", match_score(&probe, &gallery));
    ExitCode::SUCCESS
}

/// Parse a `.xyt` file: one minutia per line as whitespace-separated `x y theta`.
fn load_xyt(path: &Path) -> Result<Vec<Minutia>, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let mut it = line.split_whitespace();
        let mut field = |name: &str| -> Result<i32, String> {
            it.next()
                .ok_or_else(|| format!("{}:{}: missing {name}", path.display(), i + 1))?
                .parse()
                .map_err(|e| format!("{}:{}: bad {name}: {e}", path.display(), i + 1))
        };
        let x = field("x")?;
        let y = field("y")?;
        let theta = field("theta")?;
        out.push(Minutia { x, y, theta });
    }
    Ok(out)
}
