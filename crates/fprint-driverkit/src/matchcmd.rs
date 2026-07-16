// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `fpdev match`: score a probe capture against a gallery capture and show why they do or do not
//! match.
//!
//! Both inputs are a `.cassette` (whose device-to-host payloads reassemble into a frame) or a frame
//! image the [`image`] crate reads (PNG, PGM). The command detects minutiae in each
//! ([`crate::diag::detect`], quality kept), scores them through BOZORTH3, and prints a panel: the
//! per-print `xyt` table, the score, the threshold, and the margin that decides accept or reject.
//! `--out` writes the two minutiae overlays side by side, `--json` emits the structured result, and
//! `--verbose` adds a correspondence view that makes a low score legible.
//!
//! BOZORTH3 returns only a scalar score — it exposes no record of which minutiae pairs its cluster
//! grew from — so `--verbose` builds a tooling-side diagnostic instead: the compatible-edge count
//! from [`fprint_bozorth3::debug_pipeline`] and a nearest-neighbour pairing of the two prints in
//! `xyt` space. The pairing ignores the rotation and translation search the score itself performs,
//! so it is indicative only when the prints already sit in roughly the same frame; it is labelled as
//! such. The charter matcher is not modified.

use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};

use clap::Args;
use image::{Rgb, RgbImage};
use owo_colors::{OwoColorize as _, Style};
use serde::Serialize;

use fprint_backend_native::{assemble_frame, parse_frame_header, Frame, Session};
use fprint_mindtct::Minutia;

use crate::diag;

/// Default match threshold: a score at or above this is a match, the same value the pure-Rust
/// pipeline's end-to-end matcher uses.
const DEFAULT_THRESHOLD: u32 = 40;

/// Scan resolution stamped on a frame decoded from an image whose header carries no resolution;
/// MINDTCT reads it for its resolution-relative thresholds. Matches the value the frame decoder uses.
const DEFAULT_PPI: u16 = 500;

/// Gap in pixels between the probe and gallery overlays in the side-by-side composite, filled with
/// the divider color so the two prints read as separate panels.
const OVERLAY_GAP: u32 = 8;

/// Divider / letterbox color behind the side-by-side overlays.
const DIVIDER: Rgb<u8> = Rgb([32, 32, 32]);

/// A pairing is "close" when its spatial distance is within this many pixels — the radius under
/// which the diagnostic nearest-neighbour view counts a probe minutia as landing on a gallery one.
const CLOSE_RADIUS: f64 = 8.0;

/// Arguments for `fpdev match`.
///
/// `--probe` and `--gallery` each name a `.cassette` or a frame image to detect and score.
/// `--threshold` sets the accept cutoff, `--out` writes the overlay, `--json` emits structured
/// output, and `--verbose` adds the correspondence detail.
#[derive(Args)]
pub struct MatchArgs {
    /// The probe capture (a `.cassette` or a frame image) presented as the live scan.
    #[arg(long, value_name = "CASSETTE|IMAGE")]
    pub probe: PathBuf,
    /// The gallery capture (a `.cassette` or a frame image) to score the probe against.
    #[arg(long, value_name = "CASSETTE|IMAGE")]
    pub gallery: PathBuf,
    /// The match cutoff: a score at or above this accepts.
    #[arg(long, default_value_t = DEFAULT_THRESHOLD)]
    pub threshold: u32,
    /// Write a side-by-side overlay of the two prints' minutiae to this PNG.
    #[arg(long, value_name = "PNG")]
    pub out: Option<PathBuf>,
    /// Emit the result as structured JSON instead of prose.
    #[arg(long)]
    pub json: bool,
    /// Add the correspondence view that explains the score.
    #[arg(long)]
    pub verbose: bool,
}

/// One minutia as it appears in the structured output: the `xyt` triple plus the detector's quality.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct XytEntry {
    /// Pixel column.
    pub x: i32,
    /// Pixel row.
    pub y: i32,
    /// Ridge orientation in degrees.
    pub theta: i32,
    /// Detector reliability estimate (0..=100).
    pub quality: i32,
}

impl From<&Minutia> for XytEntry {
    fn from(m: &Minutia) -> Self {
        Self {
            x: m.x,
            y: m.y,
            theta: m.theta,
            quality: m.quality,
        }
    }
}

/// The structured result of a match: both prints' minutiae, the score, and the decision.
///
/// `margin` is `score - threshold` (widened to `i64` so a below-threshold score reads negative), and
/// `matched` is `score >= threshold`. This is the exact shape `--json` emits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MatchOutcome {
    /// The probe print's detected minutiae.
    pub probe_xyt: Vec<XytEntry>,
    /// The gallery print's detected minutiae.
    pub gallery_xyt: Vec<XytEntry>,
    /// The BOZORTH3 match score.
    pub score: u32,
    /// The accept cutoff the score is compared against.
    pub threshold: u32,
    /// `score - threshold`: non-negative on accept, negative on reject.
    pub margin: i64,
    /// Whether the score cleared the threshold.
    pub matched: bool,
}

/// Detect both prints, score them, and package the decision.
///
/// Pure over its two frames and the threshold: the same frames always yield the same [`MatchOutcome`],
/// which is what lets the offline tests pin the score relation without a terminal or a file.
#[must_use]
pub fn analyze(probe: &Frame, gallery: &Frame, threshold: u32) -> MatchOutcome {
    let probe_ms = diag::detect(probe);
    let gallery_ms = diag::detect(gallery);
    let score = fprint_bozorth3::match_score(&to_bozorth(&probe_ms), &to_bozorth(&gallery_ms));
    let margin = i64::from(score) - i64::from(threshold);
    MatchOutcome {
        probe_xyt: probe_ms.iter().map(XytEntry::from).collect(),
        gallery_xyt: gallery_ms.iter().map(XytEntry::from).collect(),
        score,
        threshold,
        margin,
        matched: score >= threshold,
    }
}

/// Run `fpdev match`.
///
/// # Errors
/// Returns an error if an input cannot be read or decoded, or the overlay cannot be written.
pub fn run(args: &MatchArgs) -> Result<(), Box<dyn std::error::Error>> {
    let probe = load_frame(&args.probe)?;
    let gallery = load_frame(&args.gallery)?;
    let outcome = analyze(&probe, &gallery, args.threshold);

    if let Some(out) = &args.out {
        let probe_ms = diag::detect(&probe);
        let gallery_ms = diag::detect(&gallery);
        let opts = diag::OverlayOptions::default();
        let composite = side_by_side(
            &diag::render_overlay(&probe, &probe_ms, &opts),
            &diag::render_overlay(&gallery, &gallery_ms, &opts),
        );
        composite
            .save_with_format(out, image::ImageFormat::Png)
            .map_err(|e| format!("write {}: {e}", out.display()))?;
        if !args.json {
            println!("overlay -> {}", out.display());
        }
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&outcome)?);
    } else {
        let color = std::io::stdout().is_terminal();
        print!(
            "{}",
            render_panel(
                &outcome,
                &args.probe,
                &args.gallery,
                (probe.width, probe.height),
                (gallery.width, gallery.height),
                args.verbose,
                color,
            )
        );
    }
    Ok(())
}

/// Read a capture into a [`Frame`]: a `.cassette` reassembles its first frame, anything else is
/// decoded as a grayscale image (PNG, PGM) by the [`image`] crate.
fn load_frame(path: &Path) -> Result<Frame, Box<dyn std::error::Error>> {
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("cassette"))
    {
        let session = crate::cassette::load(path)?;
        first_frame(&session)
    } else {
        let luma = image::open(path)
            .map_err(|e| format!("read {}: {e}", path.display()))?
            .into_luma8();
        let (w, h) = luma.dimensions();
        Ok(Frame {
            data: luma.into_raw(),
            width: w as usize,
            height: h as usize,
            ppi: DEFAULT_PPI,
        })
    }
}

/// Reassemble the first frame a session's device-to-host transfers describe: a header payload
/// followed by its pixel payload.
fn first_frame(session: &Session) -> Result<Frame, Box<dyn std::error::Error>> {
    let payloads: Vec<&[u8]> = session.bulk_in_payloads().collect();
    let pair = payloads
        .chunks_exact(2)
        .next()
        .ok_or("cassette holds no frame (need a header and a pixel payload)")?;
    let (w, h) = parse_frame_header(pair[0]).map_err(|e| format!("frame header: {e}"))?;
    let frame =
        assemble_frame(&[pair[1]], w, h, DEFAULT_PPI).map_err(|e| format!("assemble: {e}"))?;
    Ok(frame)
}

/// Convert detector minutiae to the matcher's `xyt` triples — the one fact BOZORTH3 reads.
fn to_bozorth(ms: &[Minutia]) -> Vec<fprint_bozorth3::Minutia> {
    ms.iter()
        .map(|m| fprint_bozorth3::Minutia {
            x: m.x,
            y: m.y,
            theta: m.theta,
        })
        .collect()
}

/// Stitch two overlays into one image, probe left and gallery right, separated by a divider strip.
///
/// The composite is as tall as the taller input; a shorter panel is letterboxed against the divider
/// color rather than stretched, so each print keeps its true geometry.
#[must_use]
fn side_by_side(probe: &RgbImage, gallery: &RgbImage) -> RgbImage {
    let height = probe.height().max(gallery.height());
    let width = probe.width() + OVERLAY_GAP + gallery.width();
    let mut out = RgbImage::from_pixel(width, height, DIVIDER);
    for (x, y, p) in probe.enumerate_pixels() {
        out.put_pixel(x, y, *p);
    }
    let offset = probe.width() + OVERLAY_GAP;
    for (x, y, p) in gallery.enumerate_pixels() {
        out.put_pixel(x + offset, y, *p);
    }
    out
}

/// One diagnostic pairing: a probe minutia and its nearest gallery minutia in `xyt` space.
struct Pair {
    probe_idx: usize,
    gallery_idx: usize,
    dx: i32,
    dy: i32,
    dtheta: i32,
    dist: f64,
}

/// Pair each probe minutia with its spatially nearest gallery minutia, sorted closest first.
///
/// This is a tooling-side approximation, not BOZORTH3's own correspondence: it ignores the rotation
/// and translation the score tolerates, so it is meaningful only when the prints already sit in the
/// same frame (a self-match, or two aligned captures). `dtheta` is the folded angular difference in
/// degrees (0..=180).
fn nearest_pairs(probe: &[XytEntry], gallery: &[XytEntry]) -> Vec<Pair> {
    let mut pairs: Vec<Pair> = Vec::with_capacity(probe.len());
    for (pi, p) in probe.iter().enumerate() {
        let mut best: Option<(usize, f64)> = None;
        for (gi, g) in gallery.iter().enumerate() {
            let dx = f64::from(p.x - g.x);
            let dy = f64::from(p.y - g.y);
            let d = dx.hypot(dy);
            if best.is_none_or(|(_, bd)| d < bd) {
                best = Some((gi, d));
            }
        }
        if let Some((gi, dist)) = best {
            let g = &gallery[gi];
            pairs.push(Pair {
                probe_idx: pi,
                gallery_idx: gi,
                dx: p.x - g.x,
                dy: p.y - g.y,
                dtheta: fold_angle(p.theta - g.theta),
                dist,
            });
        }
    }
    pairs.sort_by(|a, b| {
        a.dist
            .partial_cmp(&b.dist)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    pairs
}

/// Fold a signed degree difference into `0..=180`, so a wrap past 180 reads as the short way round.
fn fold_angle(delta: i32) -> i32 {
    let d = delta.rem_euclid(360);
    if d > 180 {
        360 - d
    } else {
        d
    }
}

/// The styles for one render pass; every field is a no-op [`Style`] when color is off, so the same
/// code path serves terminals, pipes, and tests.
struct Palette {
    head: Style,
    key: Style,
    dim: Style,
    accept: Style,
    reject: Style,
    score: Style,
}

impl Palette {
    fn new(color: bool) -> Self {
        if !color {
            let plain = Style::new();
            return Self {
                head: plain,
                key: plain,
                dim: plain,
                accept: plain,
                reject: plain,
                score: plain,
            };
        }
        Self {
            head: Style::new().bold(),
            key: Style::new().dimmed(),
            dim: Style::new().dimmed(),
            accept: Style::new().green().bold(),
            reject: Style::new().red().bold(),
            score: Style::new().cyan().bold(),
        }
    }
}

/// Render the whole match panel as a string: the two captures, their `xyt` tables, the score and its
/// decision, and — under `verbose` — the correspondence view.
#[must_use]
fn render_panel(
    outcome: &MatchOutcome,
    probe_path: &Path,
    gallery_path: &Path,
    probe_dims: (usize, usize),
    gallery_dims: (usize, usize),
    verbose: bool,
    color: bool,
) -> String {
    let p = Palette::new(color);
    let mut out = String::new();

    out.push_str(&format!("{}\n\n", "fpdev match".style(p.head)));
    out.push_str(&format!(
        "  {:<8} {:<28} {}x{}  {} minutiae\n",
        "probe".style(p.key),
        probe_path.display(),
        probe_dims.0,
        probe_dims.1,
        outcome.probe_xyt.len(),
    ));
    out.push_str(&format!(
        "  {:<8} {:<28} {}x{}  {} minutiae\n\n",
        "gallery".style(p.key),
        gallery_path.display(),
        gallery_dims.0,
        gallery_dims.1,
        outcome.gallery_xyt.len(),
    ));

    out.push_str(&render_xyt("probe minutiae", &outcome.probe_xyt, &p));
    out.push('\n');
    out.push_str(&render_xyt("gallery minutiae", &outcome.gallery_xyt, &p));
    out.push('\n');

    let decision = if outcome.matched {
        "accept".style(p.accept).to_string()
    } else {
        "reject".style(p.reject).to_string()
    };
    out.push_str(&format!(
        "  {:<14} {}\n",
        "BOZORTH3 score".style(p.key),
        outcome.score.style(p.score),
    ));
    out.push_str(&format!(
        "  {:<14} {}\n",
        "threshold".style(p.key),
        outcome.threshold,
    ));
    out.push_str(&format!(
        "  {:<14} {} - {} = {}  ({})\n",
        "margin".style(p.key),
        outcome.score,
        outcome.threshold,
        outcome.margin,
        decision,
    ));

    if verbose {
        out.push('\n');
        out.push_str(&render_correspondence(outcome, &p));
    }
    out
}

/// Render one print's `xyt` table (x, y, theta, quality).
fn render_xyt(title: &str, xyt: &[XytEntry], p: &Palette) -> String {
    let mut out = String::new();
    out.push_str(&format!("  {} (x y theta quality)\n", title.style(p.head)));
    out.push_str(&format!(
        "  {}\n",
        format!(
            "{:>4}  {:>5} {:>5} {:>5} {:>7}",
            "idx", "x", "y", "theta", "quality"
        )
        .style(p.dim)
    ));
    for (i, m) in xyt.iter().enumerate() {
        out.push_str(&format!(
            "  {i:>4}  {:>5} {:>5} {:>5} {:>7}\n",
            m.x, m.y, m.theta, m.quality
        ));
    }
    out
}

/// Render the correspondence view: the compatible-edge count BOZORTH3 clusters over, plus the
/// tooling-side nearest-neighbour pairing that makes the score legible.
fn render_correspondence(outcome: &MatchOutcome, p: &Palette) -> String {
    let mut out = String::new();
    let (probe_web, gallery_web, compat) = fprint_bozorth3::debug_pipeline(
        &to_bozorth_xyt(&outcome.probe_xyt),
        &to_bozorth_xyt(&outcome.gallery_xyt),
    );

    out.push_str(&format!(
        "  {}\n",
        "correspondence (diagnostic view)".style(p.head)
    ));
    out.push_str(&format!(
        "  {}\n",
        "BOZORTH3 exposes no matched-pair list; the cluster stage grows the score from the \
         compatible edges below."
            .style(p.dim)
    ));
    out.push_str(&format!(
        "  {:<20} {}\n",
        "probe web edges".style(p.key),
        probe_web
    ));
    out.push_str(&format!(
        "  {:<20} {}\n",
        "gallery web edges".style(p.key),
        gallery_web
    ));
    out.push_str(&format!(
        "  {:<20} {}\n\n",
        "compatible edges".style(p.key),
        compat.style(p.score)
    ));

    let pairs = nearest_pairs(&outcome.probe_xyt, &outcome.gallery_xyt);
    let close = pairs.iter().filter(|q| q.dist <= CLOSE_RADIUS).count();
    out.push_str(&format!(
        "  {}\n",
        "nearest-neighbour pairing in xyt space — ignores the rotation/translation the score \
         tolerates, so it reads only for already-aligned prints."
            .style(p.dim)
    ));
    out.push_str(&format!(
        "  {} of {} probe minutiae land within {:.0}px of a gallery minutia\n",
        close,
        pairs.len(),
        CLOSE_RADIUS,
    ));
    out.push_str(&format!(
        "  {}\n",
        format!(
            "{:>5} {:>5}  {:>5} {:>5} {:>6}  {:>6}",
            "probe", "gall", "dx", "dy", "dtheta", "dist"
        )
        .style(p.dim)
    ));
    for q in &pairs {
        out.push_str(&format!(
            "  {:>5} {:>5}  {:>5} {:>5} {:>6}  {:>6.1}\n",
            q.probe_idx, q.gallery_idx, q.dx, q.dy, q.dtheta, q.dist
        ));
    }
    out
}

/// Convert structured `xyt` entries back to matcher minutiae (for [`fprint_bozorth3::debug_pipeline`]).
fn to_bozorth_xyt(xyt: &[XytEntry]) -> Vec<fprint_bozorth3::Minutia> {
    xyt.iter()
        .map(|m| fprint_bozorth3::Minutia {
            x: m.x,
            y: m.y,
            theta: m.theta,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use pollster::block_on;

    use fprint_backend_native::{
        encode_frame_header, Capture, FrameSource, SyntheticFrameSource, UsbId, UsbTransfer,
    };

    fn reference_frame() -> Frame {
        let Capture::Frame(frame) = block_on(SyntheticFrameSource::reference().capture()).unwrap()
        else {
            panic!("the reference source yields a frame");
        };
        frame
    }

    fn stranger_frame() -> Frame {
        let Capture::Frame(frame) = block_on(SyntheticFrameSource::stranger().capture()).unwrap()
        else {
            panic!("the stranger source yields a frame");
        };
        frame
    }

    /// Encode a frame as a one-frame cassette session (a header payload then the pixels).
    fn cassette_of(frame: &Frame) -> Session {
        let mut session = Session::for_device(UsbId {
            vid: 0x138a,
            pid: 0x0011,
        });
        session
            .push(UsbTransfer::BulkIn {
                ep: 0x81,
                data: encode_frame_header(frame.width as u16, frame.height as u16),
            })
            .push(UsbTransfer::BulkIn {
                ep: 0x81,
                data: frame.data.clone(),
            });
        session
    }

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("fpdev-match-{}-{name}", std::process::id()))
    }

    #[test]
    fn reference_self_match_accepts_with_a_nonnegative_margin() {
        let frame = reference_frame();
        let outcome = analyze(&frame, &frame, DEFAULT_THRESHOLD);
        assert!(
            outcome.score >= DEFAULT_THRESHOLD,
            "a reference finger against itself must clear the threshold, got {}",
            outcome.score
        );
        assert!(outcome.matched);
        assert_eq!(
            outcome.margin,
            i64::from(outcome.score) - i64::from(DEFAULT_THRESHOLD)
        );
        assert!(outcome.margin >= 0);
        assert_eq!(outcome.probe_xyt, outcome.gallery_xyt);
        assert!(!outcome.probe_xyt.is_empty());
    }

    #[test]
    fn reference_against_a_stranger_scores_lower() {
        let reference = reference_frame();
        let stranger = stranger_frame();
        let self_score = analyze(&reference, &reference, DEFAULT_THRESHOLD).score;
        let cross = analyze(&reference, &stranger, DEFAULT_THRESHOLD);
        assert!(
            cross.score < self_score,
            "a distinct finger must score below the self-match: cross {} vs self {}",
            cross.score,
            self_score
        );
    }

    #[test]
    fn a_cassette_round_trips_to_the_same_frame() {
        let frame = reference_frame();
        let path = scratch("roundtrip.cassette");
        crate::cassette::save(&cassette_of(&frame), &path).unwrap();
        let loaded = load_frame(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(loaded.width, frame.width);
        assert_eq!(loaded.height, frame.height);
        assert_eq!(loaded.data, frame.data);
    }

    #[test]
    fn the_overlay_is_the_two_panels_plus_the_gap() {
        let reference = reference_frame();
        let stranger = stranger_frame();
        let ms_p = diag::detect(&reference);
        let ms_g = diag::detect(&stranger);
        let opts = diag::OverlayOptions::default();
        let composite = side_by_side(
            &diag::render_overlay(&reference, &ms_p, &opts),
            &diag::render_overlay(&stranger, &ms_g, &opts),
        );
        assert_eq!(
            composite.dimensions(),
            (
                (reference.width + OVERLAY_GAP as usize + stranger.width) as u32,
                reference.height.max(stranger.height) as u32,
            )
        );
    }

    #[test]
    fn the_json_carries_the_documented_fields() {
        let frame = reference_frame();
        let outcome = analyze(&frame, &frame, DEFAULT_THRESHOLD);
        let value: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&outcome).unwrap()).unwrap();
        for key in [
            "probe_xyt",
            "gallery_xyt",
            "score",
            "threshold",
            "margin",
            "matched",
        ] {
            assert!(value.get(key).is_some(), "json must carry {key}");
        }
        assert_eq!(value["matched"], serde_json::json!(true));
        assert_eq!(
            value["probe_xyt"].as_array().unwrap().len(),
            outcome.probe_xyt.len()
        );
        let first = &value["probe_xyt"][0];
        for key in ["x", "y", "theta", "quality"] {
            assert!(first.get(key).is_some(), "each xyt entry must carry {key}");
        }
    }

    #[test]
    fn fold_angle_takes_the_short_way_round() {
        assert_eq!(fold_angle(0), 0);
        assert_eq!(fold_angle(190), 170);
        assert_eq!(fold_angle(-10), 10);
        assert_eq!(fold_angle(180), 180);
        assert_eq!(fold_angle(540), 180);
    }

    #[test]
    fn self_match_pairs_every_minutia_onto_itself() {
        let frame = reference_frame();
        let outcome = analyze(&frame, &frame, DEFAULT_THRESHOLD);
        let pairs = nearest_pairs(&outcome.probe_xyt, &outcome.gallery_xyt);
        assert_eq!(pairs.len(), outcome.probe_xyt.len());
        assert!(
            pairs.iter().all(|q| q.dist == 0.0),
            "identical prints pair at zero distance"
        );
    }

    #[test]
    fn the_panel_names_the_score_and_decision() {
        let frame = reference_frame();
        let outcome = analyze(&frame, &frame, DEFAULT_THRESHOLD);
        let panel = render_panel(
            &outcome,
            Path::new("ref.cassette"),
            Path::new("ref.cassette"),
            (frame.width, frame.height),
            (frame.width, frame.height),
            true,
            false,
        );
        assert!(panel.contains("BOZORTH3 score"));
        assert!(panel.contains("accept"));
        assert!(panel.contains("compatible edges"));
        assert!(panel.contains(&outcome.score.to_string()));
    }
}
