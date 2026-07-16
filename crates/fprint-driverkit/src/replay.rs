// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `fpdev replay`: drive the real host-image driver over a `.cassette`, with no hardware.
//!
//! The cassette's device-to-host bytes are lifted into a [`fprint_backend_native::ScriptedTransport`]
//! and the genuine VFS5011 driver runs on top of it —
//! `ImageDevice<UsbFrameSource<ScriptedTransport>>` in the shipped stack, exercised here through its
//! capture seam ([`fprint_backend_native::UsbFrameSource`]). Each recorded frame is re-assembled by
//! the driver's own framing, then measured through the crown-jewel kernels: MINDTCT reports how many
//! minutiae the frame carries and how reliable they are, and BOZORTH3 scores the frame against
//! itself. A clean replay proves the driver's framing reproduces the recorded pixels; a mismatch
//! names the frame whose transfer diverged.

use std::path::{Path, PathBuf};

use clap::Args;
use pollster::block_on;

use fprint_backend_native::{
    Capture, Frame, FrameSource, ScriptedTransport, Session, UsbFrameSource, UsbId, UsbTransfer,
};

/// BOZORTH3 score at or above which a frame's self-match counts as a strong capture, following the
/// `end_to_end` / `real_matching` convention the rest of the stack uses.
const DEFAULT_THRESHOLD: u32 = 40;

/// Arguments for `fpdev replay`.
///
/// `cassette` is the recording to drive the driver over. `--json` emits the per-frame report as JSON
/// instead of the human transcript.
#[derive(Args)]
pub struct ReplayArgs {
    /// The `.cassette` to replay.
    #[arg(value_name = "CASSETTE")]
    pub cassette: PathBuf,
    /// Emit the per-frame report as JSON instead of the human transcript.
    #[arg(long)]
    pub json: bool,
}

/// What MINDTCT and BOZORTH3 measure in one re-assembled frame.
struct FrameReport {
    index: usize,
    width: usize,
    height: usize,
    minutiae: usize,
    mean_quality: f64,
    self_score: u32,
}

/// The point at which the driver's framing stopped matching the recording.
struct Divergence {
    index: usize,
    message: String,
}

/// The outcome of replaying one cassette: what the driver saw, frame by frame.
struct ReplayReport {
    device: Option<UsbId>,
    threshold: u32,
    frames: Vec<FrameReport>,
    diverged: Option<Divergence>,
}

/// Replay a cassette to stdout.
///
/// # Errors
/// Returns an error if the cassette cannot be read or parsed, or if the driver's framing diverged
/// from the recording (the report is still printed first, so the divergence is visible).
pub fn run(args: ReplayArgs) -> Result<(), Box<dyn std::error::Error>> {
    let session = crate::cassette::load(&args.cassette)?;
    let report = replay_session(&session);

    if args.json {
        println!("{}", report.to_json(&args.cassette));
    } else {
        report.print_human(&args.cassette);
    }

    if let Some(d) = &report.diverged {
        return Err(format!("replay diverged at frame {}: {}", d.index, d.message).into());
    }
    Ok(())
}

/// Drive the real driver over the session and collect a per-frame report.
///
/// The driver reads two bulk-in transfers per capture (a self-describing header, then the pixel
/// payload), so the recording holds two device-to-host transfers per frame. The bring-up handshake
/// only writes to the device, which the scripted transport records and acknowledges without touching
/// the frame queue.
fn replay_session(session: &Session) -> ReplayReport {
    let mut source = UsbFrameSource::new(ScriptedTransport::from_session(session));
    let _ = block_on(source.arm());

    let bulk_ins = session
        .transfers
        .iter()
        .filter(|t| matches!(t, UsbTransfer::BulkIn { .. }))
        .count();
    let frame_count = bulk_ins / 2;

    let mut frames = Vec::new();
    let mut diverged = None;
    for index in 0..frame_count {
        match block_on(source.capture()) {
            Ok(Capture::Frame(frame)) => frames.push(measure(index, &frame)),
            Ok(Capture::Retry(_)) => {
                diverged = Some(Divergence {
                    index,
                    message: "the driver reported a weak capture, which this source never emits"
                        .to_string(),
                });
                break;
            }
            Err(e) => {
                diverged = Some(Divergence {
                    index,
                    message: e.to_string(),
                });
                break;
            }
        }
    }

    if diverged.is_none() && bulk_ins % 2 == 1 {
        diverged = Some(Divergence {
            index: frame_count,
            message: "cassette ends mid-frame: a trailing header with no pixel payload".to_string(),
        });
    }

    ReplayReport {
        device: session.device,
        threshold: DEFAULT_THRESHOLD,
        frames,
        diverged,
    }
}

/// Run MINDTCT and a BOZORTH3 self-match over one re-assembled frame.
///
/// MINDTCT's per-point `quality` is averaged (the detector-facing crates drop it when projecting onto
/// the matcher's xyt triple, so it is read straight from the kernel here). The self-match scores the
/// frame's own minutiae against themselves — a set that carries matchable structure scores high.
fn measure(index: usize, frame: &Frame) -> FrameReport {
    let minutiae = fprint_mindtct::detect_minutiae(frame.as_gray());
    let count = minutiae.len();
    let mean_quality = if count == 0 {
        0.0
    } else {
        minutiae.iter().map(|m| f64::from(m.quality)).sum::<f64>() / count as f64
    };
    let bz: Vec<fprint_bozorth3::Minutia> = minutiae
        .iter()
        .map(|m| fprint_bozorth3::Minutia {
            x: m.x,
            y: m.y,
            theta: m.theta,
        })
        .collect();
    let self_score = fprint_bozorth3::match_score(&bz, &bz);

    FrameReport {
        index,
        width: frame.width,
        height: frame.height,
        minutiae: count,
        mean_quality,
        self_score,
    }
}

impl ReplayReport {
    /// The human transcript: one line of geometry and kernel readings per frame.
    fn print_human(&self, cassette: &Path) {
        println!("replay: {}", cassette.display());
        match self.device {
            Some(id) => println!("device: {:04x}:{:04x}", id.vid, id.pid),
            None => println!("device: (unrecorded)"),
        }
        println!("handshake: replayed");

        for f in &self.frames {
            let verdict = if f.self_score >= self.threshold {
                "PASS"
            } else {
                "WEAK"
            };
            println!(
                "frame {}  {}x{}  minutiae {}  mean-quality {:.1}  self-score {} ({verdict} >= {})",
                f.index,
                f.width,
                f.height,
                f.minutiae,
                f.mean_quality,
                f.self_score,
                self.threshold,
            );
        }

        match &self.diverged {
            Some(d) => println!("frame {}  DIVERGED: {}", d.index, d.message),
            None => println!(
                "frames: {} assembled, framing reproduced",
                self.frames.len()
            ),
        }
    }

    /// The same report as one JSON object.
    fn to_json(&self, cassette: &Path) -> String {
        let device = match self.device {
            Some(id) => format!("{{\"vid\":\"{:04x}\",\"pid\":\"{:04x}\"}}", id.vid, id.pid),
            None => "null".to_string(),
        };
        let frames: Vec<String> = self
            .frames
            .iter()
            .map(|f| {
                format!(
                    "{{\"index\":{},\"width\":{},\"height\":{},\"minutiae\":{},\
                     \"mean_quality\":{:.2},\"self_score\":{},\"matched\":{}}}",
                    f.index,
                    f.width,
                    f.height,
                    f.minutiae,
                    f.mean_quality,
                    f.self_score,
                    f.self_score >= self.threshold,
                )
            })
            .collect();
        let diverged = match &self.diverged {
            Some(d) => format!(
                "{{\"index\":{},\"message\":{}}}",
                d.index,
                json_str(&d.message)
            ),
            None => "null".to_string(),
        };
        format!(
            "{{\"cassette\":{},\"device\":{},\"threshold\":{},\"frames\":[{}],\"diverged\":{}}}",
            json_str(&cassette.display().to_string()),
            device,
            self.threshold,
            frames.join(","),
            diverged,
        )
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

#[cfg(test)]
mod tests {
    use super::*;
    use fprint_backend_native::{encode_frame_header, SyntheticFrameSource};

    /// The committed replay fixture: a session of one healthy reference frame.
    fn fixture_path() -> PathBuf {
        PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/replay_reference.cassette"
        ))
    }

    /// Script one [`SyntheticFrameSource::reference`] frame into a session, exactly as the driver
    /// reads it back: a frame header, then the pixel payload, as two device-to-host transfers.
    fn reference_session() -> Session {
        let mut source = SyntheticFrameSource::reference();
        let Capture::Frame(frame) = block_on(source.capture()).unwrap() else {
            panic!("the reference source yields a frame");
        };
        let w = u16::try_from(frame.width).unwrap();
        let h = u16::try_from(frame.height).unwrap();

        let mut session = Session::for_device(UsbId {
            vid: 0x138a,
            pid: 0x0011,
        });
        session
            .push(UsbTransfer::BulkIn {
                ep: 0x81,
                data: encode_frame_header(w, h),
            })
            .push(UsbTransfer::BulkIn {
                ep: 0x81,
                data: frame.data.clone(),
            });
        session
    }

    /// Re-writes the committed fixture from [`reference_session`]. Run with
    /// `--ignored write_reference_fixture` after changing the reference frame.
    #[test]
    #[ignore = "regenerates the committed fixture; run explicitly"]
    fn write_reference_fixture() {
        crate::cassette::save(&reference_session(), fixture_path()).unwrap();
    }

    #[test]
    fn committed_fixture_matches_the_reference_session() {
        let loaded = crate::cassette::load(fixture_path()).unwrap();
        assert_eq!(loaded, reference_session());
    }

    #[test]
    fn replay_reproduces_the_recorded_frame() {
        let session = crate::cassette::load(fixture_path()).unwrap();
        let report = replay_session(&session);

        assert!(
            report.diverged.is_none(),
            "a faithful cassette must not diverge"
        );
        assert_eq!(report.frames.len(), 1);

        // The driver's framing must reproduce the reference geometry.
        let f = &report.frames[0];
        assert_eq!((f.width, f.height), (256, 256));

        // The report must agree with detecting and self-matching the reference frame directly.
        let Capture::Frame(frame) = block_on(SyntheticFrameSource::reference().capture()).unwrap()
        else {
            panic!("reference frame");
        };
        let expected = measure(0, &frame);
        assert_eq!(f.minutiae, expected.minutiae);
        assert_eq!(f.self_score, expected.self_score);
        assert!(
            (f.mean_quality - expected.mean_quality).abs() < 1e-9,
            "mean quality is deterministic"
        );

        // A healthy reference finger clears the strong-capture bar against itself.
        assert!(f.minutiae > 0, "the reference frame carries minutiae");
        assert!(
            f.self_score >= report.threshold,
            "a reference frame self-matches strongly (score {} >= {})",
            f.self_score,
            report.threshold,
        );
    }

    #[test]
    fn a_truncated_cassette_pinpoints_the_diverging_frame() {
        // Drop the pixel payload: the header still arrives, but the frame cannot be assembled.
        let mut session = reference_session();
        session.transfers.truncate(1);

        let report = replay_session(&session);
        let d = report
            .diverged
            .expect("a mid-frame truncation must diverge");
        assert_eq!(d.index, 0);
        assert!(report.frames.is_empty());
    }
}
