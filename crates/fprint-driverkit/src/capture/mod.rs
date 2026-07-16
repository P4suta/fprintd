// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `fpdev import`: turn a captured USB trace (a `.pcapng`, a classic `.pcap`, or a `usbmon` text
//! log) into a `.cassette` the rest of the toolkit replays.
//!
//! The capture is read, decoded to format-neutral `UsbEvent`s, and folded into a
//! [`fprint_backend_native::Session`] of bulk/control transfers, which is saved with
//! [`crate::cassette::save`]. The `--format` selector names the on-disk shape; `auto` sniffs it from
//! the file's magic. `--vid`/`--pid` (or `--bus`/`--addr`) isolate one device's traffic from a
//! capture that carries many.
//!
//! Three capture shapes are understood, dispatched by the container's magic and each packet's USB
//! link type:
//! - **pcapng** (`pcap-file`), carrying Windows USBPcap (`DLT_USBPCAP`) or Linux usbmon
//!   (`DLT_USB_LINUX` / `DLT_USB_LINUX_MMAPPED`) packets;
//! - **classic pcap** (`pcap-file`), carrying the same USB link types;
//! - **usbmon text**, the `/sys/kernel/debug/usb/usbmon/<n>u` ASCII log.
//!
//! The USB pseudo-header layouts are documented in `docs/re-capture-formats.md`.

mod container;
mod event;
mod usbmon;
mod usbpcap;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use clap::{Args, ValueEnum};

use fprint_backend_native::{Session, UsbId};

use crate::cassette::{self, CassetteError};
use event::{Assembled, DeviceKey, Endian, UsbEvent};

/// `DLT_USB_LINUX`: the 48-byte usbmon pseudo-header.
const LINKTYPE_USB_LINUX: u32 = 189;
/// `DLT_USB_LINUX_MMAPPED`: the 64-byte usbmon pseudo-header.
const LINKTYPE_USB_LINUX_MMAPPED: u32 = 220;
/// `DLT_USBPCAP`: the Windows USBPcap pseudo-header.
const LINKTYPE_USBPCAP: u32 = 249;

/// Arguments for `fpdev import`.
///
/// `input` is the capture file to read. `--vid`/`--pid` (both hex) or `--bus`/`--addr` name the
/// device whose traffic to keep; with none, every device's traffic is imported. A vendor/product
/// filter needs the device's descriptor to appear in the capture, since a trace keys devices by
/// bus/address. `-o` sets the output cassette (default: the input path with a `.cassette`
/// extension). `--format` picks the input parser, `auto` sniffing it from the file.
#[derive(Args)]
pub struct ImportArgs {
    /// The captured trace to import.
    #[arg(value_name = "INPUT")]
    pub input: PathBuf,
    /// USB vendor id in hex to keep, e.g. `138a` or `0x138a`. Pairs with `--pid`.
    #[arg(long, value_name = "HEX")]
    pub vid: Option<String>,
    /// USB product id in hex to keep, e.g. `0011` or `0x0011`. Pairs with `--vid`.
    #[arg(long, value_name = "HEX")]
    pub pid: Option<String>,
    /// USB bus number to keep (decimal, or `0x`-prefixed hex). Pairs with `--addr`.
    #[arg(long, value_name = "NUM")]
    pub bus: Option<String>,
    /// USB device address to keep (decimal, or `0x`-prefixed hex). Pairs with `--bus`.
    #[arg(long, value_name = "NUM")]
    pub addr: Option<String>,
    /// Write the cassette here instead of next to the input.
    #[arg(short = 'o', long, value_name = "CASSETTE")]
    pub out: Option<PathBuf>,
    /// The capture's on-disk format.
    #[arg(long, value_enum, default_value_t = ImportFormat::Auto)]
    pub format: ImportFormat,
}

/// The capture formats `fpdev import` understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ImportFormat {
    /// Sniff the format from the file's magic bytes.
    Auto,
    /// A `.pcapng` next-generation capture.
    Pcapng,
    /// A Linux `usbmon` text or binary-pcap trace.
    Usbmon,
    /// A Windows USBPcap trace.
    Usbpcap,
}

/// A failure while importing a capture.
#[derive(Debug)]
pub enum ImportError {
    /// The capture file could not be read.
    Io(std::io::Error),
    /// The container framing (pcapng or classic pcap) was malformed.
    Pcap(pcap_file::PcapError),
    /// The capture's shape or contents could not be interpreted.
    Format(String),
    /// The resulting session could not be written as a cassette.
    Cassette(CassetteError),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "reading capture: {e}"),
            Self::Pcap(e) => write!(f, "capture container: {e}"),
            Self::Format(m) => write!(f, "capture format: {m}"),
            Self::Cassette(e) => write!(f, "writing cassette: {e}"),
        }
    }
}

impl std::error::Error for ImportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Pcap(e) => Some(e),
            Self::Cassette(e) => Some(e),
            Self::Format(_) => None,
        }
    }
}

impl From<std::io::Error> for ImportError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<pcap_file::PcapError> for ImportError {
    fn from(e: pcap_file::PcapError) -> Self {
        Self::Pcap(e)
    }
}

impl From<CassetteError> for ImportError {
    fn from(e: CassetteError) -> Self {
        Self::Cassette(e)
    }
}

/// Which device's traffic to keep from a capture that may carry several.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeviceFilter {
    /// Keep every device's traffic.
    All,
    /// Keep the device whose captured descriptor reports this vendor/product id.
    VidPid(UsbId),
    /// Keep the device at this bus/address.
    BusAddr(DeviceKey),
}

impl DeviceFilter {
    /// Build the filter from the raw argument strings, rejecting a half-given pair or two conflicting
    /// pairs.
    fn from_args(args: &ImportArgs) -> Result<DeviceFilter, ImportError> {
        let vidpid = match (&args.vid, &args.pid) {
            (Some(vid), Some(pid)) => Some(UsbId {
                vid: parse_hex_u16(vid, "vid")?,
                pid: parse_hex_u16(pid, "pid")?,
            }),
            (None, None) => None,
            _ => {
                return Err(ImportError::Format(
                    "--vid and --pid must be given together".into(),
                ));
            }
        };
        let busaddr = match (&args.bus, &args.addr) {
            (Some(bus), Some(addr)) => Some(DeviceKey {
                bus: parse_num_u16(bus, "bus")?,
                address: parse_num_u16(addr, "addr")?,
            }),
            (None, None) => None,
            _ => {
                return Err(ImportError::Format(
                    "--bus and --addr must be given together".into(),
                ));
            }
        };
        match (vidpid, busaddr) {
            (Some(_), Some(_)) => Err(ImportError::Format(
                "give either --vid/--pid or --bus/--addr, not both".into(),
            )),
            (Some(id), None) => Ok(DeviceFilter::VidPid(id)),
            (None, Some(key)) => Ok(DeviceFilter::BusAddr(key)),
            (None, None) => Ok(DeviceFilter::All),
        }
    }
}

/// The container shape a capture's bytes carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Detected {
    Pcapng,
    Pcap,
    UsbmonText,
}

/// Sniff the container shape from a capture's leading bytes: the pcapng and classic-pcap magics, or
/// a first line that reads as usbmon text.
fn detect(bytes: &[u8]) -> Option<Detected> {
    if bytes.len() >= 4 {
        let magic = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        match magic {
            0x0A0D_0D0A => return Some(Detected::Pcapng),
            0xA1B2_C3D4 | 0xA1B2_3C4D | 0xD4C3_B2A1 | 0x4D3C_B2A1 => return Some(Detected::Pcap),
            _ => {}
        }
    }
    looks_like_usbmon_text(bytes).then_some(Detected::UsbmonText)
}

/// A capture reads as usbmon text when its first non-blank line has the event-type marker (`S`, `C`,
/// or `E`) as its third whitespace token.
fn looks_like_usbmon_text(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let Some(line) = text.lines().find(|l| !l.trim().is_empty()) else {
        return false;
    };
    matches!(
        line.split_whitespace().nth(2),
        Some("S") | Some("C") | Some("E")
    )
}

/// Resolve the requested format against the file's magic into a concrete container shape.
fn resolve(bytes: &[u8], format: ImportFormat) -> Result<Detected, ImportError> {
    match format {
        ImportFormat::Auto => detect(bytes).ok_or(ImportError::Format(
            "could not recognize the capture — expected pcapng, classic pcap, or usbmon text"
                .into(),
        )),
        ImportFormat::Pcapng => Ok(Detected::Pcapng),
        ImportFormat::Usbpcap => match detect(bytes) {
            Some(Detected::Pcapng) => Ok(Detected::Pcapng),
            Some(Detected::Pcap) => Ok(Detected::Pcap),
            _ => Err(ImportError::Format(
                "--format usbpcap expects a pcapng or classic pcap file".into(),
            )),
        },
        ImportFormat::Usbmon => match detect(bytes) {
            Some(Detected::Pcapng) => Ok(Detected::Pcapng),
            Some(Detected::Pcap) => Ok(Detected::Pcap),
            _ => Ok(Detected::UsbmonText),
        },
    }
}

/// Read and decode a capture into its format-neutral events.
fn collect_events(bytes: &[u8], format: ImportFormat) -> Result<Vec<UsbEvent>, ImportError> {
    match resolve(bytes, format)? {
        Detected::Pcapng => {
            let (endian, packets) = container::read_pcapng(bytes)?;
            decode_packets(&packets, endian)
        }
        Detected::Pcap => {
            let (endian, packets) = container::read_pcap(bytes)?;
            decode_packets(&packets, endian)
        }
        Detected::UsbmonText => {
            let text = std::str::from_utf8(bytes)
                .map_err(|_| ImportError::Format("usbmon text is not valid UTF-8".into()))?;
            Ok(usbmon::parse_text(text))
        }
    }
}

/// Decode each container packet by its USB link type into an event, skipping stages that carry no
/// transfer.
fn decode_packets(
    packets: &[container::Packet],
    endian: Endian,
) -> Result<Vec<UsbEvent>, ImportError> {
    let mut events = Vec::new();
    for packet in packets {
        let event = match packet.linktype {
            LINKTYPE_USB_LINUX => usbmon::decode_binary(&packet.data, endian, false),
            LINKTYPE_USB_LINUX_MMAPPED => usbmon::decode_binary(&packet.data, endian, true),
            LINKTYPE_USBPCAP => usbpcap::decode(&packet.data, endian),
            other => {
                return Err(ImportError::Format(format!(
                    "unsupported USB link type {other}; expected usbmon (189/220) or USBPcap (249)"
                )));
            }
        };
        if let Some(event) = event {
            events.push(event);
        }
    }
    Ok(events)
}

/// Apply the device filter to the assembled transfers, producing the session to save.
fn build_session(assembled: Assembled, filter: DeviceFilter) -> Result<Session, ImportError> {
    let Assembled {
        transfers,
        descriptors,
    } = assembled;

    let (kept, device): (Vec<_>, Option<UsbId>) = match filter {
        DeviceFilter::All => {
            let keys: BTreeSet<DeviceKey> = transfers.iter().map(|t| t.key).collect();
            let device = if keys.len() == 1 {
                keys.iter().next().and_then(|k| descriptors.get(k).copied())
            } else {
                None
            };
            (transfers, device)
        }
        DeviceFilter::VidPid(id) => {
            let keys: BTreeSet<DeviceKey> = descriptors
                .iter()
                .filter(|(_, found)| **found == id)
                .map(|(key, _)| *key)
                .collect();
            if keys.is_empty() {
                return Err(ImportError::Format(format!(
                    "no device with vid {:04x} pid {:04x} in the capture — its device descriptor \
                     was not captured; filter by --bus/--addr instead",
                    id.vid, id.pid
                )));
            }
            let kept = transfers
                .into_iter()
                .filter(|t| keys.contains(&t.key))
                .collect();
            (kept, Some(id))
        }
        DeviceFilter::BusAddr(key) => {
            let kept: Vec<_> = transfers.into_iter().filter(|t| t.key == key).collect();
            if kept.is_empty() {
                return Err(ImportError::Format(format!(
                    "no traffic for bus {} address {} in the capture",
                    key.bus, key.address
                )));
            }
            (kept, descriptors.get(&key).copied())
        }
    };

    let mut session = match device {
        Some(id) => Session::for_device(id),
        None => Session::new(),
    };
    for transfer in kept {
        session.push(transfer.transfer);
    }
    Ok(session)
}

/// The default cassette path for an input: the input with its extension replaced by `.cassette`.
fn default_out(input: &Path) -> PathBuf {
    input.with_extension("cassette")
}

/// Import a captured trace into a `.cassette`.
///
/// # Errors
/// Returns an error if the capture cannot be read, its format is unrecognized, the requested device
/// is absent, or the cassette cannot be written.
pub fn run(args: ImportArgs) -> Result<(), Box<dyn std::error::Error>> {
    let session = import(&args)?;
    let out = args.out.unwrap_or_else(|| default_out(&args.input));
    cassette::save(&session, &out)?;

    let device = match session.device {
        Some(id) => format!("{:04x}:{:04x}", id.vid, id.pid),
        None => "unknown".to_string(),
    };
    println!(
        "fpdev import: {} transfer(s) from device {} -> {}",
        session.transfers.len(),
        device,
        out.display()
    );
    Ok(())
}

/// Decode raw capture bytes into a [`Session`], the byte-driven core of `import` with no
/// filesystem read and no device filter.
///
/// The single input to `fpdev import` an attacker writes is the capture file, so this is the fuzz
/// entrypoint (`fuzz/fuzz_targets/capture_import.rs`): it runs the whole
/// decode/assemble/build path over arbitrary bytes and must answer `Ok` or `Err`, never panic.
/// Keeping every device's traffic (`DeviceFilter::All`) drives the assembler over whatever the
/// parser emits, unfiltered.
///
/// # Errors
/// Returns an error if the bytes are not a recognized capture, a packet cannot be decoded, or the
/// framing is malformed.
pub fn parse_bytes(bytes: &[u8], format: ImportFormat) -> Result<Session, ImportError> {
    let events = collect_events(bytes, format)?;
    let assembled = event::assemble(&events);
    build_session(assembled, DeviceFilter::All)
}

/// The importer's core, split out so it can be driven directly by tests: read, decode, assemble,
/// and filter, returning the session without touching the filesystem for output.
fn import(args: &ImportArgs) -> Result<Session, ImportError> {
    let bytes = std::fs::read(&args.input)?;
    let events = collect_events(&bytes, args.format)?;
    let assembled = event::assemble(&events);
    let filter = DeviceFilter::from_args(args)?;
    build_session(assembled, filter)
}

/// Parse a `0x`-prefixed or bare hexadecimal 16-bit id.
fn parse_hex_u16(text: &str, what: &str) -> Result<u16, ImportError> {
    let trimmed = text.strip_prefix("0x").unwrap_or(text);
    u16::from_str_radix(trimmed, 16)
        .map_err(|_| ImportError::Format(format!("--{what} is not a 16-bit hex value: {text}")))
}

/// Parse a decimal (or `0x`-prefixed hex) 16-bit number.
fn parse_num_u16(text: &str, what: &str) -> Result<u16, ImportError> {
    let parsed = match text.strip_prefix("0x") {
        Some(hex) => u16::from_str_radix(hex, 16),
        None => text.parse(),
    };
    parsed.map_err(|_| ImportError::Format(format!("--{what} is not a 16-bit number: {text}")))
}

#[cfg(test)]
mod tests;
