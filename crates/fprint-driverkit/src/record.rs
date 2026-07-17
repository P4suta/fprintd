// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `fpdev record`: capture a live USB session from a connected sensor into a `.cassette`.
//!
//! A contributor with the hardware runs this once; the recorded `.cassette` is the fixture a
//! maintainer with no sensor replays in CI forever. The device named by `--vid`/`--pid` is opened
//! over the backend's real transport (the `usb` feature) wrapped in a [`RecordingTransport`], a
//! capture is driven through the same [`UsbFrameSource`](fprint_backend_native::UsbFrameSource) path
//! `replay` consumes, and the resulting [`Session`] is saved with [`crate::cassette::save`]. Without
//! the `usb` feature there is no live transport, so the command says so plainly rather than fake a
//! capture.
//!
//! [`RecordingTransport`] is the reusable piece: it wraps *any*
//! [`UsbTransport`], forwards every control/bulk call, and
//! appends the exchange — including each device-to-host `bulk_in` response — to a shared [`Session`].
//! Because it is generic and pure, wrapping a scripted transport records a capture offline, which is
//! how the record → cassette → replay loop is tested with no hardware.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use clap::Args;
use fprint_backend_native::{Session, UsbId, UsbTransfer, UsbTransport};

/// Arguments for `fpdev record`.
///
/// `--vid`/`--pid` name the device to open (both hex, `0x`-prefixed or bare). `-o` sets the output
/// cassette; with none, a name is derived from the device ids.
#[derive(Args)]
pub struct RecordArgs {
    /// USB vendor id in hex, e.g. `138a` or `0x138a`.
    #[arg(long, value_name = "HEX")]
    pub vid: String,
    /// USB product id in hex, e.g. `0011` or `0x0011`.
    #[arg(long, value_name = "HEX")]
    pub pid: String,
    /// Write the cassette here instead of a name derived from the device ids.
    #[arg(short = 'o', long, value_name = "CASSETTE")]
    pub out: Option<PathBuf>,
}

/// A [`UsbTransport`] wrapper that forwards every call to an inner transport and records the exchange.
///
/// Each `bulk_out`/`control` write and each `bulk_in` *response* is appended, in wire order, to a
/// [`Session`] shared behind an `Rc<RefCell<_>>`. The share is what lets a driver take the transport
/// by value ([`UsbFrameSource`](fprint_backend_native::UsbFrameSource) owns it) while the recorder
/// keeps a handle to read the growing tape back out afterward — grab it with [`Self::tape`] before
/// handing the transport off.
///
/// The wrapper is generic over the inner transport and does no I/O of its own, so it records a live
/// `nusb` capture and a scripted offline capture through exactly the same code.
pub struct RecordingTransport<T: UsbTransport> {
    inner: T,
    tape: Rc<RefCell<Session>>,
}

impl<T: UsbTransport> RecordingTransport<T> {
    /// Wrap `inner`, recording into a fresh untagged [`Session`].
    #[must_use]
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            tape: Rc::new(RefCell::new(Session::new())),
        }
    }

    /// Wrap `inner`, recording into a [`Session`] tagged with the device it belongs to.
    #[must_use]
    pub fn for_device(inner: T, device: UsbId) -> Self {
        Self {
            inner,
            tape: Rc::new(RefCell::new(Session::for_device(device))),
        }
    }

    /// A shared handle to the growing recording.
    ///
    /// Clone this before moving the transport into a driver; reading the handle after the capture
    /// yields the completed [`Session`].
    #[must_use]
    pub fn tape(&self) -> Rc<RefCell<Session>> {
        Rc::clone(&self.tape)
    }

    /// A snapshot of the recording captured so far.
    #[must_use]
    pub fn session(&self) -> Session {
        self.tape.borrow().clone()
    }
}

impl<T: UsbTransport> UsbTransport for RecordingTransport<T> {
    async fn bulk_out(&mut self, ep: u8, data: &[u8]) -> fprint_core::Result<()> {
        self.inner.bulk_out(ep, data).await?;
        self.tape.borrow_mut().push(UsbTransfer::BulkOut {
            ep,
            data: data.to_vec(),
        });
        Ok(())
    }

    async fn bulk_in(&mut self, ep: u8, len: usize) -> fprint_core::Result<Vec<u8>> {
        let data = self.inner.bulk_in(ep, len).await?;
        self.tape.borrow_mut().push(UsbTransfer::BulkIn {
            ep,
            data: data.clone(),
        });
        Ok(data)
    }

    async fn control(
        &mut self,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        data: &[u8],
    ) -> fprint_core::Result<Vec<u8>> {
        let response = self
            .inner
            .control(request_type, request, value, index, data)
            .await?;
        self.tape.borrow_mut().push(UsbTransfer::Control {
            request_type,
            request,
            value,
            index,
            data: data.to_vec(),
        });
        Ok(response)
    }
}

/// Record a live session to a `.cassette`.
///
/// # Errors
/// Returns an error if the ids are malformed, the device cannot be opened, the capture fails, or the
/// cassette cannot be written.
pub fn run(args: RecordArgs) -> Result<(), Box<dyn std::error::Error>> {
    let id = UsbId {
        vid: parse_hex_id("vid", &args.vid)?,
        pid: parse_hex_id("pid", &args.pid)?,
    };
    let out = args.out.unwrap_or_else(|| default_out(id));
    record_live(id, &out)
}

/// The cassette name derived from a device's ids when `-o` is absent.
fn default_out(id: UsbId) -> PathBuf {
    PathBuf::from(format!("{:04x}-{:04x}.cassette", id.vid, id.pid))
}

/// Parse one hex id, tolerating a `0x`/`0X` prefix, into a `u16`.
fn parse_hex_id(field: &str, value: &str) -> Result<u16, String> {
    let digits = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    u16::from_str_radix(digits, 16)
        .map_err(|_| format!("--{field} `{value}` is not 16-bit hex (e.g. 138a or 0x138a)"))
}

/// Open the device over real USB, drive a capture through a [`RecordingTransport`], and save the tape.
///
/// HW-verified: required. The open and the capture I/O reach a physical sensor over `nusb`; only
/// their reconciliation against hardware is unverified, exactly as [`fprint_backend_native`]'s
/// transport documents. The recording, cassette, and replay around them are exercised offline.
#[cfg(feature = "usb")]
fn record_live(id: UsbId, out: &Path) -> Result<(), Box<dyn std::error::Error>> {
    use fprint_backend_native::{FrameSource as _, NusbTransport, UsbFrameSource};
    use pollster::block_on;

    /// How many frames one `fpdev record` capture drives.
    const FRAMES: usize = 3;

    // HW-verified: required. Enumerate and claim the physical sensor.
    let transport = NusbTransport::open(id)?;
    let recorder = RecordingTransport::for_device(transport, id);
    let tape = recorder.tape();
    let mut source = UsbFrameSource::new(recorder);

    // HW-verified: required. Arm, read a few frames, disarm — the exact path `replay` re-drives.
    block_on(async {
        source.arm().await?;
        for _ in 0..FRAMES {
            let _ = source.capture().await?;
        }
        source.disarm().await
    })?;

    let session = tape.borrow().clone();
    crate::cassette::save(&session, out)?;
    println!(
        "fpdev record: {} transfers from {:04x}:{:04x} -> {}",
        session.transfers.len(),
        id.vid,
        id.pid,
        out.display()
    );
    Ok(())
}

/// Without the `usb` feature there is no live transport, so say so plainly rather than fake it.
#[cfg(not(feature = "usb"))]
fn record_live(_id: UsbId, _out: &Path) -> Result<(), Box<dyn std::error::Error>> {
    println!("live record needs the usb feature + hardware (HW-verified: required)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fprint_backend_native::{
        Capture, Frame, FrameSource as _, ScriptedTransport, SyntheticFrameSource, UsbFrameSource,
    };
    use pollster::block_on;

    /// How many frames the offline record/replay loop drives.
    const FRAMES: usize = 3;

    /// One deterministic reference frame, lifted out of the synthetic source.
    fn reference_frame() -> Frame {
        let mut src = SyntheticFrameSource::reference();
        match block_on(src.capture()).unwrap() {
            Capture::Frame(f) => f,
            Capture::Retry(_) => unreachable!("the reference source never retries"),
        }
    }

    /// Drive `n` captures through `source` (arm, capture, disarm), collecting the assembled frames.
    fn capture_frames<T: UsbTransport>(source: &mut UsbFrameSource<T>, n: usize) -> Vec<Frame> {
        block_on(async {
            source.arm().await.unwrap();
            let mut frames = Vec::with_capacity(n);
            for _ in 0..n {
                match source.capture().await.unwrap() {
                    Capture::Frame(f) => frames.push(f),
                    Capture::Retry(_) => {
                        unreachable!("the scripted transport always yields a frame")
                    }
                }
            }
            source.disarm().await.unwrap();
            frames
        })
    }

    // The whole record -> cassette -> replay loop, offline: wrap a ScriptedTransport (seeded with
    // synthetic frames) in a RecordingTransport, capture through UsbFrameSource, and prove the
    // recorded Session both round-trips as a cassette and replays to the identical frames — with no
    // hardware and no `usb` feature.
    #[test]
    fn record_round_trips_and_replays_to_the_same_frames() {
        let frame = reference_frame();
        let id = UsbId {
            vid: 0x138a,
            pid: 0x0011,
        };

        // Seed a scripted device with FRAMES synthetic captures, then record a capture off it.
        let mut scripted = ScriptedTransport::new();
        for _ in 0..FRAMES {
            scripted
                .push_frame(&frame)
                .expect("reference frame fits the wire header");
        }
        let recorder = RecordingTransport::for_device(scripted, id);
        let tape = recorder.tape();
        let mut source = UsbFrameSource::new(recorder);
        let recorded_frames = capture_frames(&mut source, FRAMES);
        assert_eq!(recorded_frames.len(), FRAMES);

        let session = tape.borrow().clone();
        assert_eq!(session.device, Some(id));
        // Every capture recorded its two device-to-host reads (header, then payload).
        assert_eq!(session.bulk_in_payloads().count(), FRAMES * 2);

        // 1) The recording round-trips through the on-disk cassette format unchanged.
        let path = std::env::temp_dir().join(format!(
            "fpdev-record-{}-{:x}.cassette",
            std::process::id(),
            recorded_frames.len()
        ));
        crate::cassette::save(&session, &path).unwrap();
        let reloaded = crate::cassette::load(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(session, reloaded);

        // 2) Replaying the recorded cassette reproduces the same frames, byte for byte.
        let replay = ScriptedTransport::from_session(&reloaded);
        let mut replay_source = UsbFrameSource::new(replay);
        let replayed_frames = capture_frames(&mut replay_source, FRAMES);
        assert_eq!(replayed_frames.len(), FRAMES);
        for (recorded, replayed) in recorded_frames.iter().zip(&replayed_frames) {
            assert_eq!(recorded.width, replayed.width);
            assert_eq!(recorded.height, replayed.height);
            assert_eq!(recorded.data, replayed.data);
        }
    }

    #[test]
    fn recording_forwards_writes_to_the_inner_transport() {
        // A bare RecordingTransport must both forward writes to its inner transport and log them.
        let recorder = RecordingTransport::new(ScriptedTransport::new());
        let tape = recorder.tape();
        let mut source = UsbFrameSource::new(recorder);
        let _ = capture_frames(&mut source, 0); // arm + disarm only, no frames

        // The tape holds the arm/disarm host writes.
        assert!(!tape.borrow().transfers.is_empty());
        // and every recorded transfer is a host-to-device write (arm/disarm never read).
        assert!(tape
            .borrow()
            .transfers
            .iter()
            .all(|t| !matches!(t, UsbTransfer::BulkIn { .. })));
    }
}
