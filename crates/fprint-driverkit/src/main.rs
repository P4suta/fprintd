// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `fpdev`: the workbench for bringing up a native driver for an unsupported fingerprint sensor.
//!
//! Each subcommand is one step of the bring-up, from identifying a device to shipping a driver:
//! `probe` (identify a connected sensor), `new-driver` (scaffold a host-image driver), `shell`
//! (poke a sensor over a control/bulk REPL), `import`/`record` (capture a USB session to a
//! `.cassette`), `replay`/`frame` (inspect a recording, decode a frame to PNG), `match`/`doctor`
//! (score captures, diagnose a frame's fitness for detection), and `ship` (package the driver).
//! The offline-verifiable path is complete; the live-USB seam (`--features usb`) is
//! hardware-gated — see `docs/known-issues.md`. The binary stays a thin clap shell: each subcommand
//! parses its arguments and hands off to the matching module in the library.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};

use std::path::PathBuf;

use fprint_driverkit::capture;
use fprint_driverkit::doctor;
use fprint_driverkit::frame;
use fprint_driverkit::matchcmd;
use fprint_driverkit::newdriver;
use fprint_driverkit::probe;
use fprint_driverkit::record;
use fprint_driverkit::replay;
use fprint_driverkit::shell;
use fprint_driverkit::ship;

#[derive(Parser)]
#[command(
    name = "fpdev",
    version,
    about = "Workbench for writing a native fingerprint-sensor driver"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Identify a connected sensor: report what it is before writing a driver for it.
    Probe(ProbeArgs),
    /// Scaffold a working host-image driver skeleton for an unsupported sensor.
    NewDriver(NewDriverArgs),
    /// Poke a sensor with an interactive control/bulk REPL (offline dry-run, or live over USB).
    Shell(ShellArgs),
    /// Import a captured USB trace (pcapng/usbmon/USBPcap) into a `.cassette`.
    Import(capture::ImportArgs),
    /// Record a live USB session from a connected sensor into a `.cassette`.
    Record(record::RecordArgs),
    /// Replay a `.cassette`, printing the USB traffic it holds.
    Replay(replay::ReplayArgs),
    /// Decode a captured frame (from a `.cassette` or a raw buffer) to a PNG.
    Frame(frame::FrameArgs),
    /// Score a probe capture against a gallery capture and show why they match or not.
    Match(matchcmd::MatchArgs),
    /// Inspect one capture's fitness for detection and suggest what to fix.
    Doctor(doctor::DoctorArgs),
    /// Package a bring-up driver for contribution (integrated, or an isolated LGPL crate).
    Ship(ship::ShipArgs),
}

/// Arguments for `fpdev probe`.
///
/// A `(--vid, --pid)` pair selects one device to classify offline; `--all` dumps the whole known
/// database. With no selector, live enumeration is reported as not-yet-wired. Both ids accept
/// `0x`-prefixed or bare hex.
#[derive(Args)]
struct ProbeArgs {
    /// USB vendor id in hex, e.g. `138a` or `0x138a`. Requires `--pid`.
    #[arg(long, value_name = "HEX")]
    vid: Option<String>,
    /// USB product id in hex, e.g. `0011` or `0x0011`. Requires `--vid`.
    #[arg(long, value_name = "HEX")]
    pid: Option<String>,
    /// List every known device, grouped by family.
    #[arg(long)]
    all: bool,
    /// Emit structured JSON instead of the human report.
    #[arg(long)]
    json: bool,
}

/// Arguments for `fpdev new-driver`.
///
/// `--name` is the driver's lowercase snake identifier (directory and module name); `--vid`/`--pid`
/// are the device's USB ids in hex. `--check` re-renders and diffs against the committed golden
/// fixture instead of writing, so a template that drifts from its golden fails loudly.
#[derive(Args)]
struct NewDriverArgs {
    /// Driver name: a lowercase snake identifier, e.g. `acme` or `acme_x`.
    #[arg(long, value_name = "IDENT")]
    name: String,
    /// USB vendor id in hex, e.g. `1c7a` or `0x1c7a`.
    #[arg(long, value_name = "HEX")]
    vid: String,
    /// USB product id in hex, e.g. `0570` or `0x0570`.
    #[arg(long, value_name = "HEX")]
    pid: String,
    /// Which archetype to scaffold. Only `host-image` is generated; `match-on-chip` prints a note.
    #[arg(long, value_enum, default_value_t = FamilyArg::HostImage)]
    family: FamilyArg,
    /// The worked example the scaffold is modeled on, recorded in the provenance note.
    #[arg(long, default_value = "vfs5011")]
    from: String,
    /// Write the tree here instead of the real driver location under `fprint-backend-native`.
    #[arg(long, value_name = "DIR")]
    out: Option<PathBuf>,
    /// Re-render in memory and diff against the committed golden fixture; write nothing.
    #[arg(long)]
    check: bool,
}

/// Arguments for `fpdev shell`.
///
/// With `--replay <file>` the shell runs a deterministic, offline dry-run over the recorded device
/// bytes. With `--vid`/`--pid` it opens that device over USB (the live seam, built only under
/// `--features usb`). With neither, it runs a dry-run over an empty transport. Ids accept
/// `0x`-prefixed or bare hex.
#[derive(Args)]
struct ShellArgs {
    /// USB vendor id in hex, e.g. `138a` or `0x138a`. Requires `--pid`.
    #[arg(long, value_name = "HEX")]
    vid: Option<String>,
    /// USB product id in hex, e.g. `0011` or `0x0011`. Requires `--vid`.
    #[arg(long, value_name = "HEX")]
    pid: Option<String>,
    /// A recorded session (one hex device-to-host payload per line) to replay offline.
    #[arg(long, value_name = "FILE")]
    replay: Option<PathBuf>,
}

/// The `--family` choices, mapped to [`newdriver::Family`].
#[derive(Clone, Copy, ValueEnum)]
enum FamilyArg {
    /// A sensor that streams pixels to the host (scaffolded).
    HostImage,
    /// A sensor that matches on the chip (not scaffolded).
    MatchOnChip,
}

impl From<FamilyArg> for newdriver::Family {
    fn from(arg: FamilyArg) -> Self {
        match arg {
            FamilyArg::HostImage => newdriver::Family::HostImage,
            FamilyArg::MatchOnChip => newdriver::Family::MatchOnChip,
        }
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("fpdev: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        Command::Probe(args) => {
            let options = probe::ProbeOptions::from_args(
                args.vid.as_deref(),
                args.pid.as_deref(),
                args.all,
                args.json,
            )?;
            probe::run(&options)
        }
        Command::NewDriver(args) => {
            let options = newdriver::NewDriverOptions::from_args(
                &args.name,
                &args.vid,
                &args.pid,
                args.family.into(),
                &args.from,
                args.out,
                args.check,
            )?;
            newdriver::run(&options)?;
            Ok(())
        }
        Command::Shell(args) => {
            let options = shell::ShellOptions::from_args(
                args.vid.as_deref(),
                args.pid.as_deref(),
                args.replay,
            )?;
            shell::run(&options)
        }
        Command::Import(args) => capture::run(args),
        Command::Record(args) => record::run(args),
        Command::Replay(args) => replay::run(args),
        Command::Frame(args) => frame::run(args),
        Command::Match(args) => matchcmd::run(&args),
        Command::Doctor(args) => doctor::run(&args),
        Command::Ship(args) => ship::run(&args),
    }
}
