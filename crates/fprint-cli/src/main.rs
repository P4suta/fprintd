// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `fprint`: a demo command-line tool over the pure-Rust fingerprint stack.
//!
//! Five subcommands, each with a `--json` variant:
//!
//! * `demo` — a file-free, hardware-free walkthrough: enroll a synthetic finger, verify a
//!   recapture of it (PASS), then verify a stranger (REJECT), all through the virtual device.
//! * `enroll` — detect minutiae in a synthetic capture and write the template to a `.fp3` file.
//! * `verify` — read two `.fp3` files and score one against the other over the device seam.
//! * `detect` — run the MINDTCT detector on a PGM image (or a synthetic one).
//! * `match` — run the BOZORTH3 matcher on two `.xyt` files (or a synthetic pair).
//!
//! The backend's operations are `async fn`; they are driven to completion with
//! [`pollster::block_on`], which needs no runtime.

#![forbid(unsafe_code)]

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use fprint_backend_native::{
    nbis_match_score, template_from_images, Scenario, VirtualDeviceBuilder,
};
use fprint_core::{Device, Finger, Minutia, Print, Template};
use fprint_mindtct::GrayImage;
use pollster::block_on;

/// Scan resolution stamped on synthetic frames, and the PGM default (500 ppi).
const DEFAULT_PPI: u16 = 500;
/// Driver match threshold, following the `real_matching` / `end_to_end` convention.
const DEFAULT_THRESHOLD: u32 = 40;
/// Minutiae count of a synthetic finger in the scripted `demo`.
const DEMO_MINUTIAE: usize = 40;
/// Size of a synthetic capture frame.
const IMG_W: usize = 256;
const IMG_H: usize = 256;

#[derive(Parser)]
#[command(
    name = "fprint",
    version,
    about = "Pure-Rust fingerprint stack demo tool"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scripted enroll -> same-finger verify (PASS) -> stranger verify (REJECT), no files.
    Demo(DemoArgs),
    /// Detect minutiae in a synthetic capture and persist the template to a `.fp3` file.
    Enroll(EnrollArgs),
    /// Score a probe `.fp3` against an enrolled `.fp3` over the device seam.
    Verify(VerifyArgs),
    /// Run the MINDTCT detector on an image and print the minutiae.
    Detect(DetectArgs),
    /// Run the BOZORTH3 matcher on two minutiae sets and print the score.
    Match(MatchArgs),
}

#[derive(Args)]
struct DemoArgs {
    /// Emit machine-readable JSON instead of prose.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct EnrollArgs {
    /// Where to write the enrolled `.fp3` template.
    #[arg(long)]
    output: PathBuf,
    /// Which finger to tag the template with.
    #[arg(long, default_value = "right-index", value_parser = parse_finger)]
    finger: Finger,
    /// Owning username recorded in the template.
    #[arg(long, default_value = "alice")]
    username: String,
    /// Seed for the synthetic capture (the same seed yields the same finger).
    #[arg(long, default_value_t = 0x1111)]
    seed: u64,
    /// Emit machine-readable JSON instead of prose.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct VerifyArgs {
    /// The enrolled template to verify against.
    #[arg(long)]
    enrolled: PathBuf,
    /// The probe template presented as the live scan.
    #[arg(long)]
    probe: PathBuf,
    /// Match threshold: a score at or above this is a match.
    #[arg(long, default_value_t = DEFAULT_THRESHOLD)]
    threshold: u32,
    /// Emit machine-readable JSON instead of prose.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct DetectArgs {
    /// A binary (P5) PGM image to detect in; omit to use a synthetic capture.
    #[arg(long)]
    input: Option<PathBuf>,
    /// Seed for the synthetic capture when `--input` is absent.
    #[arg(long, default_value_t = 0x1111)]
    seed: u64,
    /// Scan resolution in pixels-per-inch.
    #[arg(long, default_value_t = DEFAULT_PPI)]
    ppi: u16,
    /// Emit machine-readable JSON instead of prose.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct MatchArgs {
    /// The probe minutiae, as an `.xyt` file (`x y theta` per line); omit for a synthetic pair.
    #[arg(long)]
    probe: Option<PathBuf>,
    /// The gallery minutiae, as an `.xyt` file; omit for a synthetic pair.
    #[arg(long)]
    gallery: Option<PathBuf>,
    /// Seed for the synthetic pair when the files are absent.
    #[arg(long, default_value_t = 2026)]
    seed: u64,
    /// Match threshold: a score at or above this is a match.
    #[arg(long, default_value_t = DEFAULT_THRESHOLD)]
    threshold: u32,
    /// Emit machine-readable JSON instead of prose.
    #[arg(long)]
    json: bool,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("fprint: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    match cli.command {
        Command::Demo(a) => cmd_demo(&a),
        Command::Enroll(a) => cmd_enroll(&a),
        Command::Verify(a) => cmd_verify(&a),
        Command::Detect(a) => cmd_detect(&a),
        Command::Match(a) => cmd_match(&a),
    }
}

// --- demo -----------------------------------------------------------------------------------

fn cmd_demo(a: &DemoArgs) -> Result<(), Box<dyn Error>> {
    let finger = synth_triples(2026, DEMO_MINUTIAE);
    let recapture = jitter_triples(&finger, 7);
    let stranger = synth_triples(555, DEMO_MINUTIAE);

    let (genuine_matched, genuine_score) = demo_verify(&finger, &recapture)?;
    let (impostor_matched, impostor_score) = demo_verify(&finger, &stranger)?;

    if a.json {
        println!(
            "{{\"enrolled_minutiae\":{},\"threshold\":{},\
             \"genuine\":{{\"matched\":{},\"score\":{}}},\
             \"impostor\":{{\"matched\":{},\"score\":{}}}}}",
            finger.len(),
            DEFAULT_THRESHOLD,
            genuine_matched,
            genuine_score,
            impostor_matched,
            impostor_score,
        );
    } else {
        println!("enrolled a synthetic finger ({} minutiae)", finger.len());
        println!(
            "  genuine  (same finger, recaptured): {} (score {}, threshold {})",
            verdict(genuine_matched),
            genuine_score,
            DEFAULT_THRESHOLD,
        );
        println!(
            "  impostor (unrelated finger):        {} (score {}, threshold {})",
            verdict(impostor_matched),
            impostor_score,
            DEFAULT_THRESHOLD,
        );
    }
    Ok(())
}

/// Enroll `enroll` on a virtual device, present `present` as the live scan, and return
/// `(matched, score)` — the device's decision and the raw BOZORTH3 score behind it.
fn demo_verify(
    enroll: &[(i32, i32, i32)],
    present: &[(i32, i32, i32)],
) -> Result<(bool, u32), Box<dyn Error>> {
    let enrolled_t = triples_to_nbis(enroll);
    let present_t = triples_to_nbis(present);
    let score = nbis_match_score(&enrolled_t, &present_t);

    let mut dev = VirtualDeviceBuilder::host_image_sensor()
        .bozorth3_matching(DEFAULT_THRESHOLD)
        .scenario(
            Scenario::new()
                .enroll_real(enrolled_t)
                .present_real(present_t),
        )
        .build();
    block_on(dev.open())?;
    let enrolled = block_on(dev.enroll(Print::new_for_enroll(Finger::RightIndex), |_p| {}))?;
    let matched = block_on(dev.verify(&enrolled))?.matched;
    Ok((matched, score))
}

// --- enroll ---------------------------------------------------------------------------------

fn cmd_enroll(a: &EnrollArgs) -> Result<(), Box<dyn Error>> {
    let (data, w, h) = reference_image(a.seed);
    let template = template_from_images(&[image(&data, w, h, DEFAULT_PPI)]);
    let minutiae = first_sample_len(&template);

    let mut dev = VirtualDeviceBuilder::host_image_sensor()
        .scenario(Scenario::new().enroll_real(template))
        .build();
    block_on(dev.open())?;
    let enrolled = block_on(dev.enroll(Print::new_for_enroll(a.finger), |_p| {}))?;

    let mut print = enrolled;
    print.username = Some(a.username.clone());
    let bytes = fprint_fp3::to_bytes(&print)?;
    std::fs::write(&a.output, &bytes)?;

    let path = a.output.display().to_string();
    if a.json {
        println!(
            "{{\"output\":{},\"finger\":{},\"username\":{},\"minutiae\":{},\"bytes\":{}}}",
            json_str(&path),
            json_str(finger_name(a.finger)),
            json_str(&a.username),
            minutiae,
            bytes.len(),
        );
    } else {
        println!(
            "enrolled {} for {} ({} minutiae) -> {} ({} bytes)",
            finger_name(a.finger),
            a.username,
            minutiae,
            path,
            bytes.len(),
        );
    }
    Ok(())
}

// --- verify ---------------------------------------------------------------------------------

fn cmd_verify(a: &VerifyArgs) -> Result<(), Box<dyn Error>> {
    let enrolled = fprint_fp3::from_bytes(&std::fs::read(&a.enrolled)?)?;
    let probe = fprint_fp3::from_bytes(&std::fs::read(&a.probe)?)?;

    let score = nbis_match_score(&enrolled.template, &probe.template);
    let finger = enrolled.finger.unwrap_or(Finger::RightIndex);

    let mut dev = VirtualDeviceBuilder::host_image_sensor()
        .bozorth3_matching(a.threshold)
        .scenario(
            Scenario::new()
                .enroll_real(enrolled.template.clone())
                .present_real(probe.template.clone()),
        )
        .build();
    block_on(dev.open())?;
    let device_print = block_on(dev.enroll(Print::new_for_enroll(finger), |_p| {}))?;
    let matched = block_on(dev.verify(&device_print))?.matched;

    if a.json {
        println!(
            "{{\"matched\":{},\"score\":{},\"threshold\":{}}}",
            matched, score, a.threshold,
        );
    } else {
        println!(
            "{} (score {}, threshold {})",
            verdict(matched),
            score,
            a.threshold,
        );
    }
    Ok(())
}

// --- detect ---------------------------------------------------------------------------------

fn cmd_detect(a: &DetectArgs) -> Result<(), Box<dyn Error>> {
    let (data, w, h) = match &a.input {
        Some(path) => read_pgm(path)?,
        None => reference_image(a.seed),
    };
    let minutiae = fprint_mindtct::detect_minutiae(image(&data, w, h, a.ppi));

    if a.json {
        let items: Vec<String> = minutiae
            .iter()
            .map(|m| {
                format!(
                    "{{\"x\":{},\"y\":{},\"theta\":{},\"quality\":{}}}",
                    m.x, m.y, m.theta, m.quality,
                )
            })
            .collect();
        println!(
            "{{\"width\":{},\"height\":{},\"count\":{},\"minutiae\":[{}]}}",
            w,
            h,
            minutiae.len(),
            items.join(","),
        );
    } else {
        println!("detected {} minutiae in {}x{}", minutiae.len(), w, h);
        for m in &minutiae {
            println!(
                "  x={:<4} y={:<4} theta={:<3} quality={}",
                m.x, m.y, m.theta, m.quality,
            );
        }
    }
    Ok(())
}

// --- match ----------------------------------------------------------------------------------

fn cmd_match(a: &MatchArgs) -> Result<(), Box<dyn Error>> {
    let (probe, gallery) = match (&a.probe, &a.gallery) {
        (Some(p), Some(g)) => (read_xyt(p)?, read_xyt(g)?),
        (None, None) => {
            let gallery = synth_bozorth(a.seed, DEMO_MINUTIAE);
            let probe = jitter_bozorth(&gallery, 7);
            (probe, gallery)
        }
        _ => return Err("provide both --probe and --gallery, or neither".into()),
    };

    let score = fprint_bozorth3::match_score(&probe, &gallery);
    let matched = score >= a.threshold;

    if a.json {
        println!(
            "{{\"probe\":{},\"gallery\":{},\"score\":{},\"threshold\":{},\"matched\":{}}}",
            probe.len(),
            gallery.len(),
            score,
            a.threshold,
            matched,
        );
    } else {
        println!(
            "{} probe={} gallery={} score={} threshold={}",
            verdict(matched),
            probe.len(),
            gallery.len(),
            score,
            a.threshold,
        );
    }
    Ok(())
}

// --- shared helpers -------------------------------------------------------------------------

fn image(data: &[u8], w: usize, h: usize, ppi: u16) -> GrayImage<'_> {
    GrayImage::new(data, w, h, ppi).expect("valid dims")
}

fn verdict(matched: bool) -> &'static str {
    if matched {
        "PASS"
    } else {
        "REJECT"
    }
}

fn first_sample_len(t: &Template) -> usize {
    match t {
        Template::Nbis(s) => s.first().map_or(0, Vec::len),
        _ => 0,
    }
}

fn triples_to_nbis(pts: &[(i32, i32, i32)]) -> Template {
    Template::Nbis(vec![pts
        .iter()
        .map(|&(x, y, theta)| Minutia { x, y, theta })
        .collect()])
}

/// Parse a kebab-case finger name (`right-index`, `left-thumb`, …).
fn parse_finger(s: &str) -> Result<Finger, String> {
    let f = match s {
        "left-thumb" => Finger::LeftThumb,
        "left-index" => Finger::LeftIndex,
        "left-middle" => Finger::LeftMiddle,
        "left-ring" => Finger::LeftRing,
        "left-little" => Finger::LeftLittle,
        "right-thumb" => Finger::RightThumb,
        "right-index" => Finger::RightIndex,
        "right-middle" => Finger::RightMiddle,
        "right-ring" => Finger::RightRing,
        "right-little" => Finger::RightLittle,
        other => return Err(format!("unknown finger {other:?} (e.g. right-index)")),
    };
    Ok(f)
}

fn finger_name(f: Finger) -> &'static str {
    match f {
        Finger::Unknown => "unknown",
        Finger::LeftThumb => "left-thumb",
        Finger::LeftIndex => "left-index",
        Finger::LeftMiddle => "left-middle",
        Finger::LeftRing => "left-ring",
        Finger::LeftLittle => "left-little",
        Finger::RightThumb => "right-thumb",
        Finger::RightIndex => "right-index",
        Finger::RightMiddle => "right-middle",
        Finger::RightRing => "right-ring",
        Finger::RightLittle => "right-little",
    }
}

/// Escape a string as a JSON string literal, quotes included.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// --- synthetic minutiae (demo / match) ------------------------------------------------------

/// A tiny LCG: seeded, reproducible, and dependency-free.
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Lcg(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1))
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
    fn next_i32(&mut self) -> i32 {
        self.next_u64() as i32
    }
}

/// `n` distinct synthetic minutiae `(x, y, theta)` inside a plausible sensor window.
fn synth_triples(seed: u64, n: usize) -> Vec<(i32, i32, i32)> {
    let mut r = Lcg::new(seed);
    let mut pts = Vec::new();
    let mut used = std::collections::HashSet::new();
    while pts.len() < n {
        let x = 40 + r.next_i32().rem_euclid(420);
        let y = 40 + r.next_i32().rem_euclid(420);
        if used.insert((x, y)) {
            pts.push((x, y, r.next_i32().rem_euclid(360)));
        }
    }
    pts
}

/// A small per-minutia jitter: a recapture of the same finger, not a copy.
fn jitter_triples(pts: &[(i32, i32, i32)], seed: u64) -> Vec<(i32, i32, i32)> {
    let mut r = Lcg::new(seed);
    let mut used = std::collections::HashSet::new();
    pts.iter()
        .map(|&(x0, y0, t0)| {
            let mut x = x0 + r.next_i32().rem_euclid(5) - 2;
            let y = y0 + r.next_i32().rem_euclid(5) - 2;
            while !used.insert((x, y)) {
                x += 1;
            }
            (x, y, (t0 + r.next_i32().rem_euclid(5) - 2).rem_euclid(360))
        })
        .collect()
}

fn synth_bozorth(seed: u64, n: usize) -> Vec<fprint_bozorth3::Minutia> {
    synth_triples(seed, n)
        .into_iter()
        .map(|(x, y, theta)| fprint_bozorth3::Minutia { x, y, theta })
        .collect()
}

fn jitter_bozorth(pts: &[fprint_bozorth3::Minutia], seed: u64) -> Vec<fprint_bozorth3::Minutia> {
    let triples: Vec<(i32, i32, i32)> = pts.iter().map(|m| (m.x, m.y, m.theta)).collect();
    jitter_triples(&triples, seed)
        .into_iter()
        .map(|(x, y, theta)| fprint_bozorth3::Minutia { x, y, theta })
        .collect()
}

// --- synthetic capture image (enroll / detect) ----------------------------------------------
// A deterministic ridge field: parallel sinusoidal ridges, gently curved, with scattered
// ridge-dislocation dipoles that plant the ridge endings / bifurcations MINDTCT detects. The
// same idiom as `docker/mindtct-oracle/gen_corpus.py`.

/// The parameters of one synthetic ridge field.
struct Grating {
    width: usize,
    height: usize,
    seed: u64,
    period: f64,
    angle_deg: f64,
    curve: f64,
    noise: i64,
    disloc: usize,
}

fn reference_image(seed: u64) -> (Vec<u8>, usize, usize) {
    let g = Grating {
        width: IMG_W,
        height: IMG_H,
        seed,
        period: 9.0,
        angle_deg: 20.0,
        curve: 0.0018,
        noise: 18,
        disloc: 10,
    };
    (grating(&g), g.width, g.height)
}

/// Round-to-nearest into the 8-bit range.
fn clamp8(v: f64) -> u8 {
    (v + 0.5).floor().clamp(0.0, 255.0) as u8
}

/// Ridge-dislocation dipoles inside the image, positions drawn from the LCG.
fn dislocations(g: &Grating) -> Vec<(f64, f64, f64)> {
    let mut r = Lcg::new(g.seed ^ 0x0D15_104A);
    let margin = (2.0 * g.period) as usize + 10;
    let mut sings = Vec::new();
    if g.width <= 2 * margin || g.height <= 2 * margin {
        return sings;
    }
    let sep = (g.period.round() as usize).max(4);
    for _ in 0..g.disloc {
        let sx = margin + (r.next_u64() as usize) % (g.width - 2 * margin);
        let sy = margin + (r.next_u64() as usize) % (g.height - 2 * margin);
        let ox = sep + (r.next_u64() as usize) % 3;
        let oy = (r.next_u64() as i64) % 3 - 1;
        sings.push((sx as f64, sy as f64, 1.0));
        sings.push(((sx + ox) as f64, (sy as i64 + oy) as f64, -1.0));
    }
    sings
}

/// Render a [`Grating`] to a row-major 8-bit grayscale buffer.
fn grating(g: &Grating) -> Vec<u8> {
    const AMP: f64 = 95.0;
    const DC: f64 = 128.0;
    let mut r = Lcg::new(g.seed);
    let (cx, cy) = (g.width as f64 / 2.0, g.height as f64 / 2.0);
    let (ca, sa) = (
        g.angle_deg.to_radians().cos(),
        g.angle_deg.to_radians().sin(),
    );
    let kf = 2.0 * std::f64::consts::PI / g.period;
    let sings = if g.disloc > 0 {
        dislocations(g)
    } else {
        Vec::new()
    };
    let mut data = vec![0u8; g.width * g.height];
    for y in 0..g.height {
        for x in 0..g.width {
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            let u = dx * ca + dy * sa;
            let v = -dx * sa + dy * ca;
            let mut phase = kf * (u + g.curve * v * v);
            for &(sx, sy, ch) in &sings {
                phase += ch * (y as f64 - sy).atan2(x as f64 - sx);
            }
            let mut val = DC + AMP * phase.cos();
            if g.noise != 0 {
                val += ((r.next_u64() as i64) % (2 * g.noise + 1) - g.noise) as f64;
            }
            data[y * g.width + x] = clamp8(val);
        }
    }
    data
}

// --- file readers ---------------------------------------------------------------------------

/// Read a binary (P5) PGM image, returning `(data, width, height)`. Comments (`#`) and
/// arbitrary header whitespace are tolerated; 16-bit (`maxval > 255`) is rejected.
fn read_pgm(path: &Path) -> Result<(Vec<u8>, usize, usize), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut pos = 0;

    let magic = next_token(&bytes, &mut pos).ok_or("PGM: empty file")?;
    if magic != b"P5" {
        return Err("PGM: not a binary P5 image".to_string());
    }
    let width = parse_header_num(&bytes, &mut pos)?;
    let height = parse_header_num(&bytes, &mut pos)?;
    let maxval = parse_header_num(&bytes, &mut pos)?;
    if maxval > 255 {
        return Err("PGM: 16-bit images are not supported".to_string());
    }
    // Exactly one whitespace byte separates the header from the raster.
    pos += 1;

    let need = width
        .checked_mul(height)
        .ok_or("PGM: dimensions overflow")?;
    let end = pos.checked_add(need).ok_or("PGM: dimensions overflow")?;
    let raster = bytes
        .get(pos..end)
        .ok_or("PGM: raster shorter than the header declares")?;
    Ok((raster.to_vec(), width, height))
}

fn skip_ws_comments(b: &[u8], pos: &mut usize) {
    loop {
        while *pos < b.len() && b[*pos].is_ascii_whitespace() {
            *pos += 1;
        }
        if *pos < b.len() && b[*pos] == b'#' {
            while *pos < b.len() && b[*pos] != b'\n' {
                *pos += 1;
            }
        } else {
            break;
        }
    }
}

fn next_token<'a>(b: &'a [u8], pos: &mut usize) -> Option<&'a [u8]> {
    skip_ws_comments(b, pos);
    if *pos >= b.len() {
        return None;
    }
    let start = *pos;
    while *pos < b.len() && !b[*pos].is_ascii_whitespace() {
        *pos += 1;
    }
    Some(&b[start..*pos])
}

fn parse_header_num(b: &[u8], pos: &mut usize) -> Result<usize, String> {
    let token = next_token(b, pos).ok_or("PGM: truncated header")?;
    std::str::from_utf8(token)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| "PGM: non-numeric header field".to_string())
}

/// Read an `.xyt` minutiae file: `x y theta` (a fourth quality column is ignored) per line,
/// blank lines and `#` comments skipped.
fn read_xyt(path: &Path) -> Result<Vec<fprint_bozorth3::Minutia>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let x = xyt_field(fields.next(), i, "x")?;
        let y = xyt_field(fields.next(), i, "y")?;
        let theta = xyt_field(fields.next(), i, "theta")?;
        out.push(fprint_bozorth3::Minutia { x, y, theta });
    }
    Ok(out)
}

fn xyt_field(field: Option<&str>, line: usize, name: &str) -> Result<i32, String> {
    field
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("xyt line {}: missing or non-integer {name}", line + 1))
}
