// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Detect minutiae in a binary PGM (P5) fingerprint image and print them as `xyt`.
//!
//! ```text
//! cargo run -p fprint-mindtct --example detect_pgm -- <image.pgm>
//! ```
//!
//! Reads a binary PGM (8- or 16-bit), runs [`fprint_mindtct::detect_minutiae`], and writes one
//! minutia per line as `x y theta quality` — the NIST `xyt` (plus quality) form the stock NBIS
//! tools emit. The scan resolution is assumed to be 500 ppi, the common fingerprint value; PGM
//! carries none of its own.

use std::process::ExitCode;

use fprint_mindtct::{detect_minutiae, GrayImage};

/// Assumed scan resolution: PGM stores no resolution, and 500 ppi is the common fingerprint value.
const PPI: u16 = 500;

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let (Some(path), None) = (args.next(), args.next()) else {
        eprintln!("usage: detect_pgm <image.pgm>");
        return ExitCode::from(2);
    };

    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("read {}: {e}", path.to_string_lossy());
            return ExitCode::FAILURE;
        }
    };

    let (data, width, height) = match parse_p5(&bytes) {
        Ok(img) => img,
        Err(e) => {
            eprintln!("{}: {e}", path.to_string_lossy());
            return ExitCode::FAILURE;
        }
    };

    let img = match GrayImage::new(&data, width, height, PPI) {
        Ok(img) => img,
        Err(e) => {
            eprintln!("{}: {e}", path.to_string_lossy());
            return ExitCode::FAILURE;
        }
    };
    let minutiae = detect_minutiae(img);

    for m in &minutiae {
        println!("{} {} {} {}", m.x, m.y, m.theta, m.quality);
    }
    eprintln!("{} minutiae in {width}x{height}", minutiae.len());
    ExitCode::SUCCESS
}

/// Parse a binary PGM (P5): magic, width, height, maxval, then the pixel raster.
///
/// Header tokens are whitespace-separated and `#` starts a comment to end of line. A single
/// whitespace byte separates the maxval from the pixel data. 8-bit samples (maxval `<= 255`) are
/// taken as-is; 16-bit samples are read big-endian and downscaled to 8-bit.
fn parse_p5(bytes: &[u8]) -> Result<(Vec<u8>, usize, usize), String> {
    let mut pos = 0;
    let magic = next_token(bytes, &mut pos)?;
    if magic != "P5" {
        return Err(format!(
            "not a binary PGM (want magic \"P5\", got {magic:?})"
        ));
    }
    let width: usize = parse_field(&next_token(bytes, &mut pos)?, "width")?;
    let height: usize = parse_field(&next_token(bytes, &mut pos)?, "height")?;
    let maxval: usize = parse_field(&next_token(bytes, &mut pos)?, "maxval")?;
    if maxval == 0 || maxval > 65535 {
        return Err(format!("maxval {maxval} out of range (want 1..=65535)"));
    }

    // Exactly one whitespace byte separates the header from the raster.
    pos += 1;
    let pixels = width
        .checked_mul(height)
        .ok_or("width * height overflows usize")?;
    let data = if maxval <= 255 {
        raster(bytes, pos, pixels)?.to_vec()
    } else {
        // 16-bit: two big-endian bytes per sample, downscaled to 8-bit as `sample * 255 / maxval`.
        let need = pixels
            .checked_mul(2)
            .ok_or("width * height * 2 overflows usize")?;
        raster(bytes, pos, need)?
            .chunks_exact(2)
            .map(|s| (u16::from_be_bytes([s[0], s[1]]) as usize * 255 / maxval) as u8)
            .collect()
    };
    Ok((data, width, height))
}

/// Borrow `count` raster bytes at `pos`, or a "truncated raster" error naming how many are missing.
fn raster(bytes: &[u8], pos: usize, count: usize) -> Result<&[u8], String> {
    let end = pos
        .checked_add(count)
        .ok_or("raster length overflows usize")?;
    bytes.get(pos..end).ok_or_else(|| {
        format!(
            "truncated raster: need {count} bytes, have {}",
            bytes.len().saturating_sub(pos)
        )
    })
}

/// Read the next header token, skipping leading whitespace and `#` comment lines. Leaves `pos` on
/// the delimiter after the token.
fn next_token(bytes: &[u8], pos: &mut usize) -> Result<String, String> {
    loop {
        while bytes.get(*pos).is_some_and(u8::is_ascii_whitespace) {
            *pos += 1;
        }
        if bytes.get(*pos) == Some(&b'#') {
            while *pos < bytes.len() && bytes[*pos] != b'\n' {
                *pos += 1;
            }
        } else {
            break;
        }
    }
    let start = *pos;
    while *pos < bytes.len() && !bytes[*pos].is_ascii_whitespace() && bytes[*pos] != b'#' {
        *pos += 1;
    }
    if *pos == start {
        return Err("unexpected end of PGM header".into());
    }
    String::from_utf8(bytes[start..*pos].to_vec()).map_err(|_| "non-ASCII in PGM header".into())
}

/// Parse a header field to `usize` (or `u32` for maxval).
fn parse_field<T: std::str::FromStr<Err = std::num::ParseIntError>>(
    token: &str,
    name: &str,
) -> Result<T, String> {
    token
        .parse()
        .map_err(|e| format!("bad {name} {token:?}: {e}"))
}
