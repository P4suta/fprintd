// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `fpdev frame`: turn a captured frame into a PNG for the eye.
//!
//! Two inputs. A `.cassette`, whose device-to-host payloads are re-assembled through the backend's
//! frame helpers ([`fprint_backend_native::parse_frame_header`] /
//! [`fprint_backend_native::assemble_frame`]) into one PNG per frame. Or a `--raw` pixel buffer with
//! an explicit geometry, for a stream whose framing is not yet known — carrying the knobs a bring-up
//! needs to find the right shape: byte order, a row/column transpose, and a width search.
//!
//! The width search is the bring-up's oracle: for a headerless buffer of unknown geometry it enumerates
//! the divisors of the pixel count, assembles a frame at each candidate width, and runs the MINDTCT
//! detector over it. The true geometry is the one where ridges line up, so it yields the greatest mass
//! of reliable minutiae; a sheared width breaks ridges into many low-quality false ones. Ranking the
//! candidates by that quality mass lets the detector name the geometry for free.

use std::path::{Path, PathBuf};

use byteorder::{BigEndian, ByteOrder, LittleEndian};
use clap::{Args, ValueEnum};

use fprint_backend_native::{assemble_frame, parse_frame_header, Session};
use fprint_mindtct::GrayImage;

/// Scan resolution stamped on an assembled frame (500 ppi, the NBIS reference). The PNG ignores it;
/// the MINDTCT detector reads it for its resolution-relative thresholds during the width search.
const DEFAULT_PPI: u16 = 500;

/// Smallest side the width search will hand to MINDTCT: below it the detector's map windows have no
/// room to sit, so a degenerate candidate is skipped rather than measured.
const MIN_DIM: usize = 25;

/// Widest aspect ratio the width search considers: a real sensor frame is not a thin strip, so a
/// candidate more lopsided than this is skipped before detection.
const MAX_ASPECT: usize = 8;

/// How many top-ranked candidates the width search writes a thumbnail for when `--out` is given.
const THUMBNAILS: usize = 3;

/// Arguments for `fpdev frame`.
///
/// Either `cassette` (decode the frames a recording holds) or `--raw <bin>` with `--width`/`--height`
/// (decode a headerless pixel buffer). `--out` sets the PNG path; with none, a name is derived from
/// the input. The raw knobs — `--endian`, `--transpose`, `--guess-width` — search for the geometry of
/// an unknown stream.
#[derive(Args)]
pub struct FrameArgs {
    /// A `.cassette` whose frames to decode.
    #[arg(value_name = "CASSETTE", required_unless_present = "raw")]
    pub cassette: Option<PathBuf>,
    /// A headerless raw pixel buffer to decode instead of a cassette.
    #[arg(long, value_name = "BIN", conflicts_with = "cassette")]
    pub raw: Option<PathBuf>,
    /// Image width in pixels (raw input).
    #[arg(long, requires = "raw")]
    pub width: Option<usize>,
    /// Image height in pixels (raw input).
    #[arg(long, requires = "raw")]
    pub height: Option<usize>,
    /// Byte order of multi-byte samples (raw input).
    #[arg(long, value_enum, default_value_t = Endian::Le)]
    pub endian: Endian,
    /// Transpose rows and columns (raw input).
    #[arg(long, requires = "raw")]
    pub transpose: bool,
    /// Search for a plausible width instead of taking `--width` (raw input).
    #[arg(long, requires = "raw")]
    pub guess_width: bool,
    /// Write the PNG here instead of a name derived from the input.
    #[arg(long, value_name = "PNG")]
    pub out: Option<PathBuf>,
}

/// Byte order for multi-byte samples in a raw buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Endian {
    /// Little-endian.
    Le,
    /// Big-endian.
    Be,
}

/// Decode a frame to a PNG.
///
/// # Errors
/// Returns an error if the input cannot be read, the geometry does not fit the payload, or the PNG
/// cannot be written.
pub fn run(args: FrameArgs) -> Result<(), Box<dyn std::error::Error>> {
    match &args.raw {
        Some(raw) => run_raw(raw, &args),
        None => {
            let cassette = args
                .cassette
                .as_ref()
                .ok_or("frame needs a cassette or --raw <bin>")?;
            run_cassette(cassette, args.out.as_ref())
        }
    }
}

/// Re-assemble every frame a cassette holds and write one PNG each.
fn run_cassette(cassette: &Path, out: Option<&PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    let session = crate::cassette::load(cassette)?;
    let frames = frames_from_session(&session)?;
    if frames.is_empty() {
        return Err("cassette holds no frames".into());
    }

    let many = frames.len() > 1;
    for (i, frame) in frames.iter().enumerate() {
        let path = out_path(out, cassette, many.then_some(i));
        write_png(&frame.data, frame.width, frame.height, &path)?;
        println!(
            "frame {i}: {}x{} -> {}",
            frame.width,
            frame.height,
            path.display()
        );
    }
    Ok(())
}

/// Image a raw pixel buffer: either at a fixed geometry, or by searching for the width.
fn run_raw(raw: &Path, args: &FrameArgs) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = std::fs::read(raw)?;

    if args.guess_width {
        return guess_width(&bytes, raw, args.transpose, args.out.as_ref());
    }

    let (Some(width), Some(height)) = (args.width, args.height) else {
        return Err("raw input needs --width and --height (or --guess-width)".into());
    };
    let pixels = raw_to_pixels(&bytes, width, height, args.endian, args.transpose)?;
    let path = out_path(args.out.as_ref(), raw, None);
    write_png(&pixels, width, height, &path)?;
    println!("{width}x{height} -> {}", path.display());
    Ok(())
}

/// Re-assemble the frames a session's device-to-host transfers describe.
///
/// The transfers come in header/payload pairs — the two bulk-in reads the driver does per capture —
/// so an odd count is a truncated final frame and is rejected.
fn frames_from_session(
    session: &Session,
) -> Result<Vec<fprint_backend_native::Frame>, Box<dyn std::error::Error>> {
    let payloads: Vec<&[u8]> = session.bulk_in_payloads().collect();
    if payloads.len() % 2 != 0 {
        return Err("cassette ends mid-frame: a trailing header with no pixel payload".into());
    }
    let mut frames = Vec::new();
    for pair in payloads.chunks_exact(2) {
        let (w, h) = parse_frame_header(pair[0]).map_err(|e| format!("frame header: {e}"))?;
        let frame =
            assemble_frame(&[pair[1]], w, h, DEFAULT_PPI).map_err(|e| format!("assemble: {e}"))?;
        frames.push(frame);
    }
    Ok(frames)
}

/// Project a raw byte buffer onto one 8-bit grayscale plane of `width`x`height`.
///
/// One byte per pixel is taken as-is; two bytes per pixel are read as `endian` 16-bit samples and
/// scaled to 8 bits by their high byte. `transpose` reads the buffer column-major, the fix for a
/// stream whose rows and columns are swapped.
fn raw_to_pixels(
    bytes: &[u8],
    width: usize,
    height: usize,
    endian: Endian,
    transpose: bool,
) -> Result<Vec<u8>, String> {
    let px = width
        .checked_mul(height)
        .ok_or_else(|| format!("geometry {width}x{height} overflows"))?;

    let plane = if bytes.len() == px {
        bytes.to_vec()
    } else if bytes.len() == 2 * px {
        bytes
            .chunks_exact(2)
            .map(|c| {
                let s = match endian {
                    Endian::Le => LittleEndian::read_u16(c),
                    Endian::Be => BigEndian::read_u16(c),
                };
                (s >> 8) as u8
            })
            .collect()
    } else {
        return Err(format!(
            "{} bytes fit neither {width}x{height} 8-bit ({px}) nor 16-bit ({})",
            bytes.len(),
            2 * px
        ));
    };

    Ok(if transpose {
        transpose_plane(&plane, width, height)
    } else {
        plane
    })
}

/// Read a column-major plane back as row-major: `out[y*w + x] = src[x*h + y]`.
fn transpose_plane(src: &[u8], width: usize, height: usize) -> Vec<u8> {
    let mut out = vec![0u8; width * height];
    for y in 0..height {
        for x in 0..width {
            out[y * width + x] = src[x * height + y];
        }
    }
    out
}

/// A width the search tried, and how the detector scored the frame at that width.
struct Candidate {
    width: usize,
    height: usize,
    minutiae: usize,
    mean_quality: f64,
    /// Total detected quality (`minutiae` × `mean_quality`): the ranking key. The true geometry
    /// aligns the ridges into fewer, more reliable minutiae; a sheared reading breaks them into many
    /// low-quality false ones, so quality mass separates them where a raw count does not.
    quality_sum: f64,
}

/// Rank every plausible geometry of a headerless buffer by how well the detector reads it.
///
/// Each divisor of the pixel count is a candidate width (its height is the quotient); a candidate is
/// viable when both sides clear [`MIN_DIM`] and the aspect ratio stays within [`MAX_ASPECT`]. Each
/// viable candidate is run through MINDTCT and the results are ranked by total detected quality
/// ([`Candidate::quality_sum`]), then minutiae count. The real geometry aligns the ridges and rises to
/// the top.
fn rank_candidates(bytes: &[u8], transpose: bool) -> Vec<Candidate> {
    let n = bytes.len();
    let mut candidates: Vec<Candidate> = Vec::new();
    for width in 1..=n {
        if n % width != 0 {
            continue;
        }
        let height = n / width;
        if width < MIN_DIM || height < MIN_DIM {
            continue;
        }
        if width > height * MAX_ASPECT || height > width * MAX_ASPECT {
            continue;
        }

        let plane = if transpose {
            transpose_plane(bytes, width, height)
        } else {
            bytes.to_vec()
        };
        // `width` divides `n` and `plane` holds all `n` bytes, so the image dimensions fit exactly.
        let image = GrayImage::new(&plane, width, height, DEFAULT_PPI)
            .expect("plane holds width * height bytes");
        let minutiae = fprint_mindtct::detect_minutiae(image);
        let count = minutiae.len();
        let quality_sum = minutiae.iter().map(|m| f64::from(m.quality)).sum::<f64>();
        let mean_quality = if count == 0 {
            0.0
        } else {
            quality_sum / count as f64
        };
        candidates.push(Candidate {
            width,
            height,
            minutiae: count,
            mean_quality,
            quality_sum,
        });
    }

    candidates.sort_by(|a, b| {
        b.quality_sum
            .partial_cmp(&a.quality_sum)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.minutiae.cmp(&a.minutiae))
    });
    candidates
}

/// Search a headerless buffer for its width by asking the detector which geometry the ridges fit.
fn guess_width(
    bytes: &[u8],
    raw: &Path,
    transpose: bool,
    out: Option<&PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let n = bytes.len();
    if n == 0 {
        return Err("raw buffer is empty".into());
    }

    let candidates = rank_candidates(bytes, transpose);
    if candidates.is_empty() {
        return Err(format!(
            "{n} pixels have no plausible geometry (each side >= {MIN_DIM}, aspect <= {MAX_ASPECT})"
        )
        .into());
    }

    println!("guessing width for {n} pixels ({})", raw.display());
    println!(" rank  width  height  minutiae  mean-quality  quality-sum");
    for (rank, c) in candidates.iter().enumerate() {
        println!(
            "  {:>3}  {:>5}  {:>6}  {:>8}  {:>12.1}  {:>11.0}",
            rank + 1,
            c.width,
            c.height,
            c.minutiae,
            c.mean_quality,
            c.quality_sum,
        );
    }
    let best = &candidates[0];
    println!(
        "best guess: {}x{} ({} minutiae, quality-sum {:.0})",
        best.width, best.height, best.minutiae, best.quality_sum
    );

    if let Some(out) = out {
        for c in candidates.iter().take(THUMBNAILS) {
            let plane = if transpose {
                transpose_plane(bytes, c.width, c.height)
            } else {
                bytes.to_vec()
            };
            let path = geometry_named(out, c.width, c.height);
            write_png(&plane, c.width, c.height, &path)?;
            println!("thumbnail {}x{} -> {}", c.width, c.height, path.display());
        }
    }
    Ok(())
}

/// Write an 8-bit grayscale plane as a PNG.
fn write_png(pixels: &[u8], width: usize, height: usize, path: &Path) -> Result<(), String> {
    let buf = image::GrayImage::from_raw(width as u32, height as u32, pixels.to_vec())
        .ok_or_else(|| format!("{width}x{height} does not match {} pixels", pixels.len()))?;
    buf.save_with_format(path, image::ImageFormat::Png)
        .map_err(|e| format!("write {}: {e}", path.display()))
}

/// Where to write a frame's PNG: `out` if given (indexed for a multi-frame cassette), else a name
/// derived from the input beside it.
fn out_path(out: Option<&PathBuf>, input: &Path, index: Option<usize>) -> PathBuf {
    match (out, index) {
        (Some(p), None) => p.clone(),
        (Some(p), Some(i)) => indexed(p, i),
        (None, index) => default_out(input, index),
    }
}

/// A default PNG name beside `input`: `<stem>.png`, or `<stem>-<i>.png` for a multi-frame cassette.
fn default_out(input: &Path, index: Option<usize>) -> PathBuf {
    let stem = input
        .file_stem()
        .map_or_else(|| "frame".to_string(), |s| s.to_string_lossy().into_owned());
    let name = match index {
        None => format!("{stem}.png"),
        Some(i) => format!("{stem}-{i}.png"),
    };
    input.with_file_name(name)
}

/// Insert an index before the extension of `path`: `out.png` -> `out-1.png`.
fn indexed(path: &Path, index: usize) -> PathBuf {
    let stem = path
        .file_stem()
        .map_or_else(|| "frame".to_string(), |s| s.to_string_lossy().into_owned());
    let ext = path
        .extension()
        .map_or_else(|| "png".to_string(), |s| s.to_string_lossy().into_owned());
    path.with_file_name(format!("{stem}-{index}.{ext}"))
}

/// Tag a thumbnail path with the geometry it images: `out.png` -> `out-256x256.png`.
fn geometry_named(path: &Path, width: usize, height: usize) -> PathBuf {
    let stem = path
        .file_stem()
        .map_or_else(|| "frame".to_string(), |s| s.to_string_lossy().into_owned());
    let ext = path
        .extension()
        .map_or_else(|| "png".to_string(), |s| s.to_string_lossy().into_owned());
    path.with_file_name(format!("{stem}-{width}x{height}.{ext}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic ridge grating at a known geometry, the same idiom the synthetic capture source
    /// uses: parallel sinusoidal ridges with scattered dislocations that plant minutiae.
    fn grating(width: usize, height: usize) -> Vec<u8> {
        let mut data = vec![0u8; width * height];
        let (cx, cy) = (width as f64 / 2.0, height as f64 / 2.0);
        let period = 9.0;
        let (ca, sa) = 20.0_f64.to_radians().sin_cos();
        let kf = 2.0 * std::f64::consts::PI / period;
        for y in 0..height {
            for x in 0..width {
                let dx = x as f64 - cx;
                let dy = y as f64 - cy;
                let u = dx * sa + dy * ca;
                let v = -dx * ca + dy * sa;
                let phase = kf * (u + 0.0018 * v * v);
                let val = 128.0 + 95.0 * phase.cos();
                data[y * width + x] = (val + 0.5).floor().clamp(0.0, 255.0) as u8;
            }
        }
        data
    }

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("fpdev-frame-{}-{name}", std::process::id()))
    }

    #[test]
    fn png_round_trips_dimensions_and_pixels() {
        let (w, h) = (64usize, 48usize);
        let pixels = grating(w, h);
        let path = scratch("roundtrip.png");
        write_png(&pixels, w, h, &path).unwrap();

        let decoded = image::open(&path).unwrap().into_luma8();
        let _ = std::fs::remove_file(&path);

        assert_eq!(decoded.dimensions(), (w as u32, h as u32));
        assert_eq!(
            decoded.into_raw(),
            pixels,
            "the PNG carries the exact pixels"
        );
    }

    #[test]
    fn eight_bit_raw_is_taken_verbatim() {
        let (w, h) = (40usize, 30usize);
        let pixels = grating(w, h);
        let out = raw_to_pixels(&pixels, w, h, Endian::Le, false).unwrap();
        assert_eq!(out, pixels);
    }

    #[test]
    fn sixteen_bit_raw_reads_the_high_byte_by_endian() {
        // One 16-bit sample per pixel, value 0x1234: the high byte 0x12 is the 8-bit sample.
        let (w, h) = (25usize, 25usize);
        let mut le = Vec::new();
        let mut be = Vec::new();
        for _ in 0..w * h {
            le.extend_from_slice(&0x1234u16.to_le_bytes());
            be.extend_from_slice(&0x1234u16.to_be_bytes());
        }
        assert!(raw_to_pixels(&le, w, h, Endian::Le, false)
            .unwrap()
            .iter()
            .all(|&p| p == 0x12));
        assert!(raw_to_pixels(&be, w, h, Endian::Be, false)
            .unwrap()
            .iter()
            .all(|&p| p == 0x12));
    }

    #[test]
    fn transpose_swaps_the_axes() {
        // A 3x2 column-major source becomes its row-major transpose.
        let src = vec![0, 1, 2, 3, 4, 5]; // columns: (0,1)(2,3)(4,5)
        let out = transpose_plane(&src, 3, 2);
        assert_eq!(out, vec![0, 2, 4, 1, 3, 5]);
    }

    #[test]
    fn raw_geometry_mismatch_is_rejected() {
        let bytes = vec![0u8; 100];
        assert!(raw_to_pixels(&bytes, 7, 7, Endian::Le, false).is_err());
    }

    #[test]
    fn guess_width_ranks_the_true_width_first() {
        // The reference finger's pixels, headerless: its true 256x256 geometry is the one that aligns
        // the ridges and dislocations, so the detector must rank it first among the divisor widths.
        use fprint_backend_native::{Capture, FrameSource, SyntheticFrameSource};
        use pollster::block_on;

        let Capture::Frame(frame) = block_on(SyntheticFrameSource::reference().capture()).unwrap()
        else {
            panic!("reference frame");
        };

        let candidates = rank_candidates(&frame.data, false);
        assert!(!candidates.is_empty());
        assert_eq!(
            (candidates[0].width, candidates[0].height),
            (frame.width, frame.height),
            "the true geometry aligns the ridges into the most reliable minutiae"
        );
    }
}
