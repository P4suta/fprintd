// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Decode each bundled FP3 fixture and re-encode it, reporting whether the bytes are a fixed point.
//!
//! ```text
//! cargo run -p fprint-fp3 --example roundtrip
//! ```
//!
//! The fixtures are real libfprint `fp_print_serialize` output, one per template kind. Decoding
//! with [`fprint_fp3::from_bytes`] and re-encoding with [`fprint_fp3::to_bytes`] must reproduce the
//! input exactly — the interop contract the codec upholds against libfprint's own bytes.

use std::process::ExitCode;

use fprint_core::Print;
use fprint_fp3::Fp3Error;

/// The frozen libfprint fixtures, embedded so the example needs no file on disk.
const FIXTURES: &[(&str, &[u8])] = &[
    (
        "libfprint_virtual_device.fp3",
        include_bytes!("../tests/fixtures/libfprint_virtual_device.fp3"),
    ),
    (
        "libfprint_virtual_image_nbis.fp3",
        include_bytes!("../tests/fixtures/libfprint_virtual_image_nbis.fp3"),
    ),
];

fn main() -> ExitCode {
    let mut all_fixed = true;
    for (name, blob) in FIXTURES {
        match roundtrip(blob) {
            Ok(true) => println!("{name}: fixed point ({} bytes)", blob.len()),
            Ok(false) => {
                println!("{name}: NOT a fixed point — re-encoded bytes differ");
                all_fixed = false;
            }
            Err(e) => {
                println!("{name}: {e}");
                all_fixed = false;
            }
        }
    }

    if all_fixed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Decode then re-encode a blob; `true` when the bytes are reproduced exactly.
fn roundtrip(blob: &[u8]) -> Result<bool, Fp3Error> {
    let print: Print = fprint_fp3::from_bytes(blob)?;
    let reencoded = fprint_fp3::to_bytes(&print)?;
    Ok(reencoded.as_slice() == blob)
}
