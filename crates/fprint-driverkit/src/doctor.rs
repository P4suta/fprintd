// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `fpdev doctor`: inspect one capture's fitness for detection and suggest what to fix.
//!
//! It assembles a `.cassette`'s frame, detects its minutiae ([`crate::diag::detect`]), scores the
//! frame ([`crate::diag::quality_report`]), and reads the score against the same thresholds
//! [`crate::diag::hints`] uses, so every metric prints an `ok`/`warn` verdict beside its bar and the
//! concrete hints follow. The output is a plain scriptable report by default, structured JSON under
//! `--json`, and an overlay PNG under `--out-overlay`.
//!
//! `--ppi`, `--transpose`, and `--frame` are the bring-up knobs a driver author sweeps to find the
//! true geometry and resolution: MINDTCT's thresholds are resolution-relative, and a transposed
//! buffer shears ridges into noise. `--tui` drives those same knobs interactively in a full-screen
//! dashboard; it is a convenience over the functions here, isolated in `tui` so the offline tests
//! never touch it.

use std::path::{Path, PathBuf};

use clap::Args;
use serde::Serialize;

use fprint_backend_native::{assemble_frame, parse_frame_header, Frame, Session};
use fprint_mindtct::Minutia;

use crate::diag::{self, QualityReport};

/// Scan resolution stamped on a frame assembled from a cassette (500 ppi, the NBIS reference), the
/// same default `fpdev frame` and `fpdev replay` use. `--ppi` overrides it for the detection sweep.
const DEFAULT_PPI: u16 = 500;

/// Smallest side MINDTCT's map windows fit in: below it the detector has no room, so a smaller
/// geometry is a bring-up mistake worth flagging. Matches the `fpdev frame` width-search floor.
const MIN_DIM: usize = 25;

/// Below this many minutiae, geometry or resolution is suspect. Mirrors the [`diag::hints`] floor so
/// a metric's `warn` verdict and the hint that explains it agree.
const FEW_MINUTIAE: usize = 8;

/// Below this gray-level span, contrast or exposure is suspect. Mirrors the [`diag::hints`] floor.
const LOW_DYNAMIC_RANGE: u8 = 40;

/// Below this pixel standard deviation the ridge/valley swing is too small to binarize cleanly — the
/// spread companion to [`LOW_DYNAMIC_RANGE`], which a lone bright speck could otherwise satisfy.
const LOW_CONTRAST: f64 = 20.0;

/// Below this foreground fraction the finger covers too little of the frame. Mirrors [`diag::hints`].
const LOW_FOREGROUND: f64 = 0.25;

/// Below this mean reliability the detected minutiae are weak. Mirrors [`diag::hints`].
const LOW_RELIABILITY: f64 = 25.0;

/// A mean gray level this dark, or (mirrored) this bright, reads as a saturated exposure. Mirrors
/// [`diag::hints`].
const DARK_MEAN: f64 = 32.0;

/// Arguments for `fpdev doctor`.
///
/// The positional `cassette` is the capture to inspect. `--frame` selects which assembled frame,
/// `--ppi` overrides the detection resolution, and `--transpose` swaps rows and columns before
/// detection — the geometry and resolution knobs a bring-up sweeps. `--tui` opens the interactive
/// dashboard over those same knobs, `--json` emits the report as structured output, and
/// `--out-overlay` writes the minutiae overlay.
#[derive(Args)]
pub struct DoctorArgs {
    /// The `.cassette` whose capture to inspect.
    #[arg(value_name = "CASSETTE")]
    pub cassette: PathBuf,
    /// Which assembled frame to inspect (0-based); a cassette may hold several.
    #[arg(long, value_name = "INDEX", default_value_t = 0)]
    pub frame: usize,
    /// Detect at this resolution instead of the frame's stamped ppi (MINDTCT is resolution-relative).
    #[arg(long, value_name = "PPI")]
    pub ppi: Option<u16>,
    /// Transpose rows and columns before detection, the fix for a row/column-swapped buffer.
    #[arg(long)]
    pub transpose: bool,
    /// Open the interactive terminal dashboard instead of printing once.
    #[arg(long)]
    pub tui: bool,
    /// Emit the report as structured JSON instead of prose.
    #[arg(long)]
    pub json: bool,
    /// Write the minutiae overlay to this PNG.
    #[arg(long, value_name = "PNG")]
    pub out_overlay: Option<PathBuf>,
}

/// One metric's verdict: its value, the threshold it is judged against, and whether it passes.
#[derive(Clone, Debug, Serialize)]
struct Check {
    /// The metric's name, e.g. `dynamic range`.
    label: &'static str,
    /// The metric's value, formatted for the report.
    value: String,
    /// The threshold the value is judged against, e.g. `>= 40`.
    threshold: &'static str,
    /// Whether the value clears its threshold.
    ok: bool,
}

/// Everything the doctor derives from one prepared frame: the detected minutiae, the quality report,
/// the per-metric verdicts, and the hints. The overlay and both output forms read from this.
struct Reading {
    minutiae: Vec<Minutia>,
    report: QualityReport,
    checks: Vec<Check>,
    hints: Vec<String>,
}

/// Detect, score, and grade one frame.
fn analyze(frame: &Frame) -> Reading {
    let minutiae = diag::detect(frame);
    let report = diag::quality_report(frame, &minutiae);
    let checks = grade(&report);
    let hints = diag::hints(&report);
    Reading {
        minutiae,
        report,
        checks,
        hints,
    }
}

/// Grade a [`QualityReport`] into one [`Check`] per metric, judged against the [`diag::hints`]
/// thresholds so a `warn` verdict and the hint that explains it never disagree.
fn grade(r: &QualityReport) -> Vec<Check> {
    let reliability_ok = r.minutiae_count == 0 || r.mean_reliability >= LOW_RELIABILITY;
    vec![
        Check {
            label: "geometry",
            value: format!("{}x{}", r.width, r.height),
            threshold: "each side >= 25px",
            ok: r.width >= MIN_DIM && r.height >= MIN_DIM,
        },
        Check {
            label: "dynamic range",
            value: r.dynamic_range.to_string(),
            threshold: ">= 40",
            ok: r.dynamic_range >= LOW_DYNAMIC_RANGE,
        },
        Check {
            label: "contrast (stdev)",
            value: format!("{:.1}", r.pixel_stdev),
            threshold: ">= 20.0",
            ok: r.pixel_stdev >= LOW_CONTRAST,
        },
        Check {
            label: "minutiae",
            value: r.minutiae_count.to_string(),
            threshold: ">= 8",
            ok: r.minutiae_count >= FEW_MINUTIAE,
        },
        Check {
            label: "mean reliability",
            value: format!("{:.1}", r.mean_reliability),
            threshold: ">= 25.0",
            ok: reliability_ok,
        },
        Check {
            label: "foreground",
            value: format!("{:.2}", r.foreground_fraction),
            threshold: ">= 0.25",
            ok: r.foreground_fraction >= LOW_FOREGROUND,
        },
        Check {
            label: "exposure (mean)",
            value: format!("{:.1}", r.pixel_mean),
            threshold: "32.0..=223.0",
            ok: r.pixel_mean >= DARK_MEAN && r.pixel_mean <= 255.0 - DARK_MEAN,
        },
    ]
}

/// The prepared frame the doctor inspects and the report it produced.
struct Inspection {
    frame: Frame,
    reading: Reading,
}

/// Re-assemble every frame a session's device-to-host transfers describe.
///
/// The transfers come in header/payload pairs — the two bulk-in reads the driver does per capture —
/// so an odd count is a truncated final frame and is rejected.
fn frames_from_session(session: &Session) -> Result<Vec<Frame>, String> {
    let payloads: Vec<&[u8]> = session.bulk_in_payloads().collect();
    if payloads.len() % 2 != 0 {
        return Err("cassette ends mid-frame: a trailing header with no pixel payload".to_string());
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

/// Apply the geometry and resolution knobs to one assembled frame before detection.
fn prepare(frame: &Frame, ppi: Option<u16>, transpose: bool) -> Frame {
    let base = if transpose {
        transpose_frame(frame)
    } else {
        frame.clone()
    };
    match ppi {
        Some(ppi) => Frame { ppi, ..base },
        None => base,
    }
}

/// Read a frame's row-major buffer back as its transpose: a `w`x`h` frame becomes `h`x`w`, with
/// `out[x*h + y] = src[y*w + x]`. The fix for a stream whose rows and columns are swapped.
fn transpose_frame(frame: &Frame) -> Frame {
    let (w, h) = (frame.width, frame.height);
    let mut data = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            data[x * h + y] = frame.data[y * w + x];
        }
    }
    Frame {
        data,
        width: h,
        height: w,
        ppi: frame.ppi,
    }
}

/// Select and prepare one frame from an assembled set, then grade it.
fn inspect(
    frames: &[Frame],
    index: usize,
    ppi: Option<u16>,
    transpose: bool,
) -> Result<Inspection, String> {
    let frame = frames.get(index).ok_or_else(|| {
        format!(
            "frame {index} out of range: cassette holds {} frame(s)",
            frames.len()
        )
    })?;
    let frame = prepare(frame, ppi, transpose);
    let reading = analyze(&frame);
    Ok(Inspection { frame, reading })
}

/// The plain scriptable report: a header, one graded line per metric, then the hints.
fn plain_report(
    cassette: &Path,
    index: usize,
    total: usize,
    transpose: bool,
    inspection: &Inspection,
) -> String {
    let r = &inspection.reading;
    let f = &inspection.frame;
    let mut out = String::new();
    out.push_str(&format!("doctor: {}\n", cassette.display()));
    let note = if transpose { "  (transposed)" } else { "" };
    out.push_str(&format!(
        "frame {index} of {total}   {}x{} @ {}ppi{note}\n\n",
        f.width, f.height, f.ppi
    ));
    for c in &r.checks {
        let verdict = if c.ok { "ok" } else { "warn" };
        out.push_str(&format!(
            "  {:<18}{:>10}  {:<4} ({})\n",
            c.label, c.value, verdict, c.threshold
        ));
    }
    out.push_str("\nhints:\n");
    for h in &r.hints {
        out.push_str(&format!("  - {h}\n"));
    }
    out
}

/// The report as one pretty JSON object: geometry, the [`QualityReport`], the graded checks, and the
/// hints. `serde_json` gives the report and each check their derived shape.
#[derive(Serialize)]
struct DoctorJson<'a> {
    cassette: String,
    frame: usize,
    frames_total: usize,
    width: usize,
    height: usize,
    ppi: u16,
    transpose: bool,
    report: &'a QualityReport,
    checks: &'a [Check],
    hints: &'a [String],
}

/// Render the inspection as pretty JSON.
fn json_report(
    cassette: &Path,
    index: usize,
    total: usize,
    transpose: bool,
    inspection: &Inspection,
) -> String {
    let f = &inspection.frame;
    let r = &inspection.reading;
    let doc = DoctorJson {
        cassette: cassette.display().to_string(),
        frame: index,
        frames_total: total,
        width: f.width,
        height: f.height,
        ppi: f.ppi,
        transpose,
        report: &r.report,
        checks: &r.checks,
        hints: &r.hints,
    };
    serde_json::to_string_pretty(&doc).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
}

/// Run `fpdev doctor`.
///
/// # Errors
/// Returns an error if the cassette cannot be read or decoded, the selected frame is out of range,
/// or the overlay cannot be written.
pub fn run(args: &DoctorArgs) -> Result<(), Box<dyn std::error::Error>> {
    let session = crate::cassette::load(&args.cassette)?;
    let frames = frames_from_session(&session)?;
    if frames.is_empty() {
        return Err("cassette holds no frames".into());
    }

    if args.tui {
        return tui::run(&args.cassette, &frames, args);
    }

    let total = frames.len();
    let inspection = inspect(&frames, args.frame, args.ppi, args.transpose)?;

    if let Some(path) = &args.out_overlay {
        let overlay = diag::render_overlay(
            &inspection.frame,
            &inspection.reading.minutiae,
            &diag::OverlayOptions::default(),
        );
        overlay
            .save(path)
            .map_err(|e| format!("write overlay {}: {e}", path.display()))?;
    }

    if args.json {
        println!(
            "{}",
            json_report(
                &args.cassette,
                args.frame,
                total,
                args.transpose,
                &inspection
            )
        );
    } else {
        print!(
            "{}",
            plain_report(
                &args.cassette,
                args.frame,
                total,
                args.transpose,
                &inspection
            )
        );
    }
    Ok(())
}

/// The optional full-screen tuning dashboard.
///
/// It sweeps the same knobs the CLI flags expose — frame, ppi, transpose — recomputing through
/// [`super::analyze`] on every keypress, so the numbers it shows are the numbers `--json` would emit
/// for the same knobs. The module is isolated from the offline tests: nothing here runs without a
/// live terminal, and [`super::run`] reaches it only under `--tui`.
mod tui {
    use std::path::Path;

    use crossterm::event::{self, Event, KeyCode, KeyEventKind};
    use ratatui::layout::{Constraint, Direction, Layout};
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Borders, Paragraph};
    use ratatui::Frame as TuiFrame;

    use fprint_backend_native::Frame;
    use fprint_mindtct::Minutia;

    use super::{analyze, prepare, DoctorArgs, Reading};

    /// The gray ramp the ascii preview paints, dark ridge to light valley.
    const RAMP: &[u8] = b"@%#*+=-:. ";

    /// The live knobs the dashboard sweeps.
    struct State<'a> {
        frames: &'a [Frame],
        index: usize,
        ppi: u16,
        transpose: bool,
    }

    /// Drive the dashboard until the user quits.
    pub(super) fn run(
        cassette: &Path,
        frames: &[Frame],
        args: &DoctorArgs,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut state = State {
            frames,
            index: args.frame.min(frames.len().saturating_sub(1)),
            ppi: args.ppi.unwrap_or(frames[0].ppi),
            transpose: args.transpose,
        };

        let mut terminal = ratatui::init();
        let result = event_loop(&mut terminal, cassette, &mut state);
        ratatui::restore();
        result
    }

    /// Recompute, draw, and read one key until quit.
    fn event_loop(
        terminal: &mut ratatui::DefaultTerminal,
        cassette: &Path,
        state: &mut State,
    ) -> Result<(), Box<dyn std::error::Error>> {
        loop {
            let frame = prepare(&state.frames[state.index], Some(state.ppi), state.transpose);
            let reading = analyze(&frame);
            terminal.draw(|f| draw(f, cassette, state, &frame, &reading))?;

            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Right | KeyCode::Char('n') => {
                    state.index = (state.index + 1).min(state.frames.len() - 1);
                }
                KeyCode::Left | KeyCode::Char('p') => {
                    state.index = state.index.saturating_sub(1);
                }
                KeyCode::Char('+') | KeyCode::Char('=') => {
                    state.ppi = state.ppi.saturating_add(50).min(2000);
                }
                KeyCode::Char('-') | KeyCode::Char('_') => {
                    state.ppi = state.ppi.saturating_sub(50).max(50);
                }
                KeyCode::Char('t') => state.transpose = !state.transpose,
                _ => {}
            }
        }
    }

    /// Lay out the header, the graded metrics, the hints, and the ascii preview.
    fn draw(f: &mut TuiFrame, cassette: &Path, state: &State, frame: &Frame, reading: &Reading) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(0),
                Constraint::Length(3),
            ])
            .split(f.area());

        let header = Paragraph::new(format!(
            "doctor: {}   frame {}/{}   {}x{} @ {}ppi{}",
            cassette.display(),
            state.index,
            state.frames.len(),
            frame.width,
            frame.height,
            frame.ppi,
            if state.transpose {
                "  (transposed)"
            } else {
                ""
            },
        ))
        .block(Block::default().borders(Borders::ALL).title("capture"));
        f.render_widget(header, rows[0]);

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(rows[1]);

        f.render_widget(metrics(reading), cols[0]);
        f.render_widget(
            preview(frame, &reading.minutiae, cols[1].width, cols[1].height),
            cols[1],
        );

        let help = Paragraph::new("<-/->  frame    +/-  ppi    t  transpose    q  quit")
            .block(Block::default().borders(Borders::ALL).title("keys"));
        f.render_widget(help, rows[2]);
    }

    /// The graded metrics and the hints, colored by verdict.
    fn metrics(reading: &Reading) -> Paragraph<'static> {
        let mut lines: Vec<Line> = Vec::new();
        for c in &reading.checks {
            let (mark, color) = if c.ok {
                ("ok  ", Color::Green)
            } else {
                ("warn", Color::Red)
            };
            lines.push(Line::from(vec![
                Span::styled(
                    mark.to_string(),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(
                    " {:<18}{:>10}  ({})",
                    c.label, c.value, c.threshold
                )),
            ]));
        }
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "hints",
            Style::default().add_modifier(Modifier::BOLD),
        ));
        for h in &reading.hints {
            lines.push(Line::raw(format!("- {h}")));
        }
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("metrics"))
    }

    /// A downsampled ascii view of the frame with detected minutiae marked, to read placement and
    /// geometry at a glance. It shares no pixels with the PNG overlay; it is the terminal's cheap echo
    /// of it.
    fn preview(frame: &Frame, minutiae: &[Minutia], width: u16, height: u16) -> Paragraph<'static> {
        // Leave a cell of border on each side.
        let cols = usize::from(width.saturating_sub(2)).max(1);
        let rows = usize::from(height.saturating_sub(2)).max(1);
        let step_x = frame.width.div_ceil(cols).max(1);
        let step_y = frame.height.div_ceil(rows).max(1);

        let out_w = frame.width.div_ceil(step_x);
        let out_h = frame.height.div_ceil(step_y);
        let mut grid = vec![vec![b' '; out_w]; out_h];
        for (oy, row) in grid.iter_mut().enumerate() {
            for (ox, cell) in row.iter_mut().enumerate() {
                let px = (ox * step_x).min(frame.width - 1);
                let py = (oy * step_y).min(frame.height - 1);
                let g = frame.data[py * frame.width + px];
                let idx = usize::from(g) * (RAMP.len() - 1) / 255;
                *cell = RAMP[idx];
            }
        }

        // MINDTCT's origin is bottom-left, y upward; the grid's is top-left.
        let flip = frame.height.saturating_sub(1);
        for m in minutiae {
            let mx = m.x.clamp(0, frame.width as i32 - 1) as usize;
            let my = (flip as i32 - m.y).clamp(0, frame.height as i32 - 1) as usize;
            let ox = (mx / step_x).min(out_w - 1);
            let oy = (my / step_y).min(out_h - 1);
            grid[oy][ox] = b'*';
        }

        let text: Vec<Line> = grid
            .into_iter()
            .map(|row| Line::raw(String::from_utf8_lossy(&row).into_owned()))
            .collect();
        Paragraph::new(text).block(Block::default().borders(Borders::ALL).title("preview"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed reference cassette: one healthy synthetic finger, the same fixture `fpdev replay`
    /// freezes.
    fn reference_cassette() -> PathBuf {
        PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/replay_reference.cassette"
        ))
    }

    fn frame(data: Vec<u8>, width: usize, height: usize) -> Frame {
        Frame {
            data,
            width,
            height,
            ppi: 500,
        }
    }

    #[test]
    fn reference_cassette_reads_as_healthy() {
        let session = crate::cassette::load(reference_cassette()).unwrap();
        let frames = frames_from_session(&session).unwrap();
        let inspection = inspect(&frames, 0, None, false).unwrap();
        let r = &inspection.reading.report;

        // The report agrees with the diagnostics' frozen golden over the reference finger.
        assert_eq!((r.width, r.height), (256, 256));
        assert_eq!(r.minutiae_count, 17);
        assert_eq!(r.dynamic_range, 226);
        assert!((r.foreground_fraction - 1.0).abs() < 1e-9);
        assert!(r.mean_reliability > 90.0);

        // Every metric passes, so the only hint is the all-clear.
        assert!(
            inspection.reading.checks.iter().all(|c| c.ok),
            "a healthy reference frame flags no metric"
        );
        assert_eq!(inspection.reading.hints.len(), 1);
        assert!(inspection.reading.hints[0].contains("no obvious defect"));
    }

    #[test]
    fn reference_plain_report_reads_ok_everywhere() {
        let session = crate::cassette::load(reference_cassette()).unwrap();
        let frames = frames_from_session(&session).unwrap();
        let inspection = inspect(&frames, 0, None, false).unwrap();
        let text = plain_report(&reference_cassette(), 0, frames.len(), false, &inspection);

        assert!(text.contains("frame 0 of 1"));
        assert!(text.contains("256x256 @ 500ppi"));
        assert!(text.contains("no obvious defect"));
        assert!(
            !text.contains("warn"),
            "no metric warns on the reference frame"
        );
    }

    #[test]
    fn reference_json_carries_the_report_and_checks() {
        let session = crate::cassette::load(reference_cassette()).unwrap();
        let frames = frames_from_session(&session).unwrap();
        let inspection = inspect(&frames, 0, None, false).unwrap();
        let json = json_report(&reference_cassette(), 0, frames.len(), false, &inspection);

        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["report"]["minutiae_count"], 17);
        assert_eq!(value["report"]["dynamic_range"], 226);
        assert_eq!(value["width"], 256);
        assert_eq!(value["checks"].as_array().unwrap().len(), 7);
        assert!(value["checks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["ok"] == true));
        assert_eq!(value["hints"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn a_weak_frame_warns_and_hints() {
        // A near-flat, low-contrast frame: values sit in a 10-level band, so it carries almost no
        // ridge structure. It plants few minutiae, spans little dynamic range, and reads as no
        // foreground — the three symptoms a bring-up needs named.
        let (w, h) = (96usize, 96usize);
        let mut data = vec![125u8; w * h];
        for (i, p) in data.iter_mut().enumerate() {
            *p = if i % 2 == 0 { 120 } else { 130 };
        }
        let reading = analyze(&frame(data, w, h));

        let by = |label: &str| reading.checks.iter().find(|c| c.label == label).unwrap();
        assert!(by("geometry").ok, "the geometry itself is fine");
        assert!(
            !by("dynamic range").ok,
            "a 10-level band is low dynamic range"
        );
        assert!(!by("contrast (stdev)").ok);
        assert!(!by("minutiae").ok, "a flat band plants too few minutiae");
        assert!(!by("foreground").ok);
        assert!(by("exposure (mean)").ok, "the mean sits mid-range");

        assert!(reading.hints.iter().any(|h| h.contains("few minutiae")));
        assert!(reading
            .hints
            .iter()
            .any(|h| h.contains("low dynamic range")));
        assert!(reading
            .hints
            .iter()
            .any(|h| h.contains("little foreground")));
    }

    #[test]
    fn ppi_override_reaches_detection() {
        // The ppi knob must travel from the flag into the frame the detector reads.
        let f = frame(vec![128u8; 64 * 64], 64, 64);
        let prepared = prepare(&f, Some(1000), false);
        assert_eq!(prepared.ppi, 1000);
    }

    #[test]
    fn transpose_swaps_the_axes() {
        // A 3x2 frame becomes its 2x3 transpose, pixels carried across.
        let f = frame(vec![0, 1, 2, 3, 4, 5], 3, 2);
        let t = transpose_frame(&f);
        assert_eq!((t.width, t.height), (2, 3));
        assert_eq!(t.data, vec![0, 3, 1, 4, 2, 5]);
    }

    #[test]
    fn out_of_range_frame_is_rejected() {
        let session = crate::cassette::load(reference_cassette()).unwrap();
        let frames = frames_from_session(&session).unwrap();
        match inspect(&frames, 9, None, false) {
            Err(e) => assert!(e.contains("out of range")),
            Ok(_) => panic!("frame 9 is out of range for a one-frame cassette"),
        }
    }
}
