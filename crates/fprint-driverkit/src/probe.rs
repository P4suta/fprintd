// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Phase 0 — identify: inspect a sensor and report what it is, so a driver author knows what they
//! are writing against before they start.
//!
//! The probe is deliberately offline and deterministic: it classifies a `(vid, pid)` against the
//! [`device_db`] interoperability table and states whether the
//! host-image capture seam ([`ImageDevice<UsbFrameSource<_>>`]) can reach the device. Report
//! building is pure (inputs to a [`ProbeReport`] and its renderers), so the classification and the
//! reachability verdict are unit-testable without a terminal or hardware.
//!
//! [`ImageDevice<UsbFrameSource<_>>`]: fprint_backend_native::ImageDevice

use std::fmt::Write as _;
use std::io::IsTerminal as _;

use fprint_backend_native::device_db::{self, DeviceRecord, Family};
use owo_colors::{OwoColorize as _, Style};

/// What the `fpdev probe` subcommand was asked to do.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ProbeOptions {
    /// The device to classify, as a parsed `(vid, pid)`. `None` means no selector was given.
    pub selector: Option<(u16, u16)>,
    /// Dump the whole device database, grouped by family, instead of one device.
    pub all: bool,
    /// Emit structured JSON instead of the human report.
    pub json: bool,
}

impl ProbeOptions {
    /// Build options from the raw CLI strings, parsing the hex ids.
    ///
    /// `vid`/`pid` accept `0x`-prefixed or bare hex. They must be supplied together; a lone one is a
    /// usage error. `--all` ignores any selector.
    ///
    /// # Errors
    /// Returns [`ProbeError`] if an id fails to parse or only one of `vid`/`pid` is given.
    pub fn from_args(
        vid: Option<&str>,
        pid: Option<&str>,
        all: bool,
        json: bool,
    ) -> Result<Self, ProbeError> {
        let selector = match (vid, pid) {
            (Some(v), Some(p)) => Some((parse_id("vid", v)?, parse_id("pid", p)?)),
            (None, None) => None,
            (Some(_), None) => return Err(ProbeError::LoneSelector("pid")),
            (None, Some(_)) => return Err(ProbeError::LoneSelector("vid")),
        };
        Ok(Self {
            selector,
            all,
            json,
        })
    }
}

/// A failure while parsing or validating probe arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeError {
    /// A `--vid`/`--pid` value was not valid 16-bit hex.
    BadHex {
        /// Which argument (`"vid"` or `"pid"`).
        field: &'static str,
        /// The offending value, as given.
        value: String,
    },
    /// Only one of `--vid`/`--pid` was supplied; both or neither are required.
    LoneSelector(&'static str),
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadHex { field, value } => {
                write!(
                    f,
                    "--{field} `{value}` is not 16-bit hex (e.g. 138a or 0x138a)"
                )
            }
            Self::LoneSelector(missing) => {
                write!(
                    f,
                    "--{missing} is also required: pass both --vid and --pid, or neither"
                )
            }
        }
    }
}

impl std::error::Error for ProbeError {}

/// Parse one hex id, tolerating a `0x`/`0X` prefix, into a `u16`.
fn parse_id(field: &'static str, value: &str) -> Result<u16, ProbeError> {
    let digits = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    if digits.is_empty() {
        return Err(ProbeError::BadHex {
            field,
            value: value.to_owned(),
        });
    }
    u16::from_str_radix(digits, 16).map_err(|_| ProbeError::BadHex {
        field,
        value: value.to_owned(),
    })
}

/// How a device is classified for the purpose of the host-image seam.
///
/// Mirrors [`device_db::Family`] and adds [`FamilyClass::Unknown`] for a `(vid, pid)` no libfprint
/// driver claims — the case a driver author most often starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FamilyClass {
    /// A driver that streams pixels to the host; the [`FrameSource`](fprint_backend_native::FrameSource) seam reaches it.
    HostImage,
    /// A driver that matches on the sensor; no frame reaches the host.
    MatchOnChip,
    /// A known driver that fits neither archetype (e.g. a composite bridge).
    Other,
    /// No known driver claims this `(vid, pid)`.
    Unknown,
}

impl FamilyClass {
    fn of(record: Option<&DeviceRecord>) -> Self {
        match record.map(|r| r.family) {
            Some(Family::HostImage) => Self::HostImage,
            Some(Family::MatchOnChip) => Self::MatchOnChip,
            Some(Family::Other) => Self::Other,
            None => Self::Unknown,
        }
    }

    /// The screaming-case label shown in the report and JSON.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::HostImage => "HOST-IMAGE",
            Self::MatchOnChip => "MATCH-ON-CHIP",
            Self::Other => "OTHER",
            Self::Unknown => "UNKNOWN",
        }
    }

    /// The reachability verdict this classification implies.
    #[must_use]
    pub fn reach(self) -> Reach {
        match self {
            Self::HostImage => Reach::HostImageSeam,
            Self::MatchOnChip => Reach::MatchOnChip,
            Self::Other | Self::Unknown => Reach::Unknown,
        }
    }
}

/// Whether the host-image capture seam can drive a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// `ImageDevice<UsbFrameSource<_>>` applies: write a `FrameSource` and the rest is built.
    HostImageSeam,
    /// Match-on-chip: the sensor matches internally, so the host-image seam cannot reach it.
    MatchOnChip,
    /// Not decidable from the id alone.
    Unknown,
}

impl Reach {
    /// A stable machine token for JSON output.
    fn token(self) -> &'static str {
        match self {
            Self::HostImageSeam => "host-image-seam",
            Self::MatchOnChip => "match-on-chip",
            Self::Unknown => "unknown",
        }
    }
}

/// A classified device, ready to render. Pure: [`ProbeReport::new`] is the whole computation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeReport {
    /// USB vendor id.
    pub vid: u16,
    /// USB product id.
    pub pid: u16,
    /// The libfprint driver that claims this id, if any.
    pub driver: Option<&'static str>,
    /// The seam classification for this device.
    pub family: FamilyClass,
}

impl ProbeReport {
    /// Classify a `(vid, pid)` against the device database.
    #[must_use]
    pub fn new(vid: u16, pid: u16) -> Self {
        let record = device_db::lookup(vid, pid);
        Self {
            vid,
            pid,
            driver: record.map(|r| r.driver),
            family: FamilyClass::of(record),
        }
    }

    /// `"138a:0011"`.
    #[must_use]
    pub fn id(&self) -> String {
        format!("{:04x}:{:04x}", self.vid, self.pid)
    }

    /// The reachability verdict.
    #[must_use]
    pub fn reach(&self) -> Reach {
        self.family.reach()
    }

    /// The `next:` hint — the command to try after identifying the device.
    ///
    /// The named subcommands are future work; the hint is honest about the direction each family
    /// points a driver author.
    #[must_use]
    pub fn next_hint(&self) -> String {
        match self.family {
            FamilyClass::HostImage => {
                let from = self.driver.unwrap_or("<closest>");
                format!("try `fpdev new-driver --from {from}`")
            }
            FamilyClass::MatchOnChip => {
                "read docs/adding-a-driver.md — match-on-chip bring-up is a different path"
                    .to_owned()
            }
            FamilyClass::Other | FamilyClass::Unknown => {
                format!(
                    "`fpdev shell --vid {:04x} --pid {:04x}` to poke it",
                    self.vid, self.pid
                )
            }
        }
    }

    /// Render the one-screen human report. `color` gates ANSI styling so pipes and tests stay plain.
    #[must_use]
    pub fn render(&self, color: bool) -> String {
        let p = Palette::new(color);
        let mut out = String::new();

        let _ = writeln!(
            out,
            "{} {}",
            "fpdev probe ·".style(p.head),
            self.id().style(p.id)
        );
        let _ = writeln!(out);

        let driver = match self.driver {
            Some(d) => format!("{}  {}", d.style(p.strong), "(libfprint)".style(p.dim)),
            None => "(unknown to libfprint)".style(p.dim).to_string(),
        };
        let _ = writeln!(
            out,
            "  {}     {}",
            "device".style(p.key),
            self.id().style(p.id)
        );
        let _ = writeln!(out, "  {}     {driver}", "driver".style(p.key));
        let _ = writeln!(
            out,
            "  {}     {}",
            "family".style(p.key),
            self.family.label().style(p.family(self.family))
        );

        let _ = write!(out, "  {}  ", "reach".style(p.key));
        match self.reach() {
            Reach::HostImageSeam => {
                let _ = writeln!(
                    out,
                    "{}   {}",
                    "ImageDevice<UsbFrameSource<_>>".style(p.strong),
                    "✓ this seam applies".style(p.ok),
                );
            }
            Reach::MatchOnChip => {
                let _ = writeln!(out, "{}", "✗ out of reach — match-on-chip".style(p.warn));
                let _ = writeln!(
                    out,
                    "         the sensor matches internally and returns no frame, so the"
                );
                let _ = writeln!(out, "         host-image FrameSource seam cannot drive it.");
                let _ = writeln!(
                    out,
                    "         see {}",
                    "docs/adding-a-driver.md".style(p.link)
                );
            }
            Reach::Unknown => {
                let _ = writeln!(
                    out,
                    "{}",
                    "host-image if it streams grayscale frames,".style(p.warn)
                );
                let _ = writeln!(out, "         otherwise unknown");
            }
        }

        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "  {}  {}",
            "next".style(p.key),
            self.next_hint().style(p.dim)
        );
        out
    }

    /// The structured view: a stable, machine-readable object.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "vid": format!("0x{:04x}", self.vid),
            "pid": format!("0x{:04x}", self.pid),
            "id": self.id(),
            "driver": self.driver,
            "family": self.family.label(),
            "reach": self.reach().token(),
            "next": self.next_hint(),
        })
    }
}

/// Styles for one render pass; every field is a no-op [`Style`] when color is off, so the same
/// rendering code path serves terminals, pipes, and tests.
struct Palette {
    head: Style,
    key: Style,
    id: Style,
    strong: Style,
    dim: Style,
    ok: Style,
    warn: Style,
    link: Style,
    host: Style,
    moc: Style,
    other: Style,
}

impl Palette {
    fn new(color: bool) -> Self {
        if !color {
            let plain = Style::new();
            return Self {
                head: plain,
                key: plain,
                id: plain,
                strong: plain,
                dim: plain,
                ok: plain,
                warn: plain,
                link: plain,
                host: plain,
                moc: plain,
                other: plain,
            };
        }
        Self {
            head: Style::new().bold(),
            key: Style::new().dimmed(),
            id: Style::new().cyan().bold(),
            strong: Style::new().bold(),
            dim: Style::new().dimmed(),
            ok: Style::new().green().bold(),
            warn: Style::new().yellow().bold(),
            link: Style::new().cyan().underline(),
            host: Style::new().green().bold(),
            moc: Style::new().yellow().bold(),
            other: Style::new().magenta().bold(),
        }
    }

    fn family(&self, family: FamilyClass) -> Style {
        match family {
            FamilyClass::HostImage => self.host,
            FamilyClass::MatchOnChip => self.moc,
            FamilyClass::Other => self.other,
            FamilyClass::Unknown => self.warn,
        }
    }
}

/// Render the whole device database, grouped by family, as a table.
#[must_use]
pub fn render_all(color: bool) -> String {
    let p = Palette::new(color);
    let mut out = String::new();

    let records = device_db::all();
    let _ = writeln!(
        out,
        "{} {} known devices",
        "fpdev probe ·".style(p.head),
        records.len().style(p.id),
    );

    for (family, heading) in [
        (Family::HostImage, FamilyClass::HostImage),
        (Family::MatchOnChip, FamilyClass::MatchOnChip),
        (Family::Other, FamilyClass::Other),
    ] {
        let group: Vec<&DeviceRecord> = records.iter().filter(|r| r.family == family).collect();
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "  {} {}",
            heading.label().style(p.family(heading)),
            format!("({})", group.len()).style(p.dim),
        );
        for r in group {
            let _ = writeln!(
                out,
                "    {}   {}",
                format!("{:04x}:{:04x}", r.vid, r.pid).style(p.id),
                r.driver.style(p.dim),
            );
        }
    }
    out
}

/// The whole database as a JSON object, grouped by family.
#[must_use]
pub fn all_json() -> serde_json::Value {
    let group = |family: Family| -> Vec<serde_json::Value> {
        device_db::all()
            .iter()
            .filter(|r| r.family == family)
            .map(|r| {
                serde_json::json!({
                    "id": format!("{:04x}:{:04x}", r.vid, r.pid),
                    "vid": format!("0x{:04x}", r.vid),
                    "pid": format!("0x{:04x}", r.pid),
                    "driver": r.driver,
                })
            })
            .collect()
    };
    serde_json::json!({
        "count": device_db::all().len(),
        "host-image": group(Family::HostImage),
        "match-on-chip": group(Family::MatchOnChip),
        "other": group(Family::Other),
    })
}

/// Run the identify phase for the given options, printing to stdout.
///
/// # Errors
/// Never fails today; returns `Result` so live enumeration (a later phase) can surface I/O errors
/// through the same seam.
pub fn run(options: &ProbeOptions) -> Result<(), Box<dyn std::error::Error>> {
    let color = std::io::stdout().is_terminal();

    if options.all {
        if options.json {
            println!("{}", serde_json::to_string_pretty(&all_json())?);
        } else {
            print!("{}", render_all(color));
        }
        return Ok(());
    }

    if let Some((vid, pid)) = options.selector {
        let report = ProbeReport::new(vid, pid);
        if options.json {
            println!("{}", serde_json::to_string_pretty(&report.to_json())?);
        } else {
            print!("{}", report.render(color));
        }
        return Ok(());
    }

    // No selector: enumerate the live bus and classify each device found. Enumeration opens no
    // endpoint, so it is verifiable on any host — but it needs the `usb` feature's transport.
    #[cfg(feature = "usb")]
    {
        run_live(options.json, color)
    }
    #[cfg(not(feature = "usb"))]
    {
        eprintln!(
            "fpdev probe: live USB enumeration needs the `usb` feature.\n\
             Rebuild with `--features usb`, or pass --vid <hex> --pid <hex> to classify a device\n\
             offline, or --all to list the known-device database."
        );
        Ok(())
    }
}

/// Enumerate the attached USB devices and classify each against the interoperability database.
///
/// A read-only bus listing (it opens nothing), so it runs on any host with USB devices — the one
/// live-USB path that needs no specific sensor.
#[cfg(feature = "usb")]
fn run_live(json: bool, color: bool) -> Result<(), Box<dyn std::error::Error>> {
    let devices = fprint_backend_native::list_usb_devices()?;
    if json {
        let rows: Vec<serde_json::Value> = devices.iter().map(live_json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!(rows))?
        );
    } else {
        print!("{}", render_live(&devices, color));
    }
    Ok(())
}

/// One enumerated device as JSON: its offline classification plus the descriptor strings the OS
/// reported.
#[cfg(feature = "usb")]
fn live_json(dev: &fprint_backend_native::UsbDeviceInfo) -> serde_json::Value {
    let mut row = ProbeReport::new(dev.id.vid, dev.id.pid).to_json();
    if let Some(obj) = row.as_object_mut() {
        obj.insert("manufacturer".into(), dev.manufacturer.clone().into());
        obj.insert("product".into(), dev.product.clone().into());
    }
    row
}

/// Render the live enumeration: one line per attached device, classified, plus a `next:` hint for
/// the first host-image candidate. `color` gates ANSI styling.
#[cfg(feature = "usb")]
fn render_live(devices: &[fprint_backend_native::UsbDeviceInfo], color: bool) -> String {
    let p = Palette::new(color);
    let mut out = String::new();
    let _ = writeln!(out, "{}", "fpdev probe · live bus".style(p.head));
    let _ = writeln!(out);

    if devices.is_empty() {
        let _ = writeln!(out, "  {}", "no USB devices found on the bus".style(p.dim));
        return out;
    }

    for dev in devices {
        let report = ProbeReport::new(dev.id.vid, dev.id.pid);
        let label = dev
            .product
            .as_deref()
            .or(report.driver)
            .unwrap_or("(no product string)");
        let mark = match report.reach() {
            Reach::HostImageSeam => "✓".style(p.ok).to_string(),
            _ => "·".style(p.dim).to_string(),
        };
        let _ = writeln!(
            out,
            "  {mark} {}   {}   {}",
            report.id().style(p.id),
            report.family.label().style(p.family(report.family)),
            label.style(p.dim),
        );
    }

    // Point the author at the first device the host-image seam can reach.
    if let Some(hit) = devices
        .iter()
        .map(|d| ProbeReport::new(d.id.vid, d.id.pid))
        .find(|r| matches!(r.reach(), Reach::HostImageSeam))
    {
        let _ = writeln!(out);
        let _ = writeln!(out, "  {}  {}", "next".style(p.key), hit.next_hint());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_prefixed_and_bare_hex() {
        assert_eq!(parse_id("vid", "138a"), Ok(0x138a));
        assert_eq!(parse_id("vid", "0x138a"), Ok(0x138a));
        assert_eq!(parse_id("vid", "0X138A"), Ok(0x138a));
        assert!(matches!(
            parse_id("pid", "nope"),
            Err(ProbeError::BadHex { .. })
        ));
        assert!(matches!(
            parse_id("pid", ""),
            Err(ProbeError::BadHex { .. })
        ));
        assert!(matches!(
            parse_id("pid", "10000"),
            Err(ProbeError::BadHex { .. })
        ));
    }

    #[test]
    fn lone_selector_is_an_error() {
        assert_eq!(
            ProbeOptions::from_args(Some("138a"), None, false, false),
            Err(ProbeError::LoneSelector("pid")),
        );
        assert_eq!(
            ProbeOptions::from_args(None, Some("0011"), false, false),
            Err(ProbeError::LoneSelector("vid")),
        );
        assert!(ProbeOptions::from_args(None, None, true, false).is_ok());
    }

    #[test]
    fn known_host_image_device_is_reachable() {
        // vfs5011 — a host-image sensor; the FrameSource seam reaches it.
        let report = ProbeReport::new(0x138a, 0x0011);
        assert_eq!(report.family, FamilyClass::HostImage);
        assert_eq!(report.driver, Some("vfs5011"));
        assert_eq!(report.reach(), Reach::HostImageSeam);

        let text = report.render(false);
        assert!(text.contains("138a:0011"));
        assert!(text.contains("vfs5011"));
        assert!(text.contains("HOST-IMAGE"));
        assert!(text.contains("ImageDevice<UsbFrameSource<_>>"));
        assert!(text.contains("this seam applies"));
        assert!(text.contains("new-driver --from vfs5011"));
        // A plain render carries no ANSI escapes.
        assert!(!text.contains('\u{1b}'));

        let json = report.to_json();
        assert_eq!(json["family"], "HOST-IMAGE");
        assert_eq!(json["reach"], "host-image-seam");
        assert_eq!(json["driver"], "vfs5011");
    }

    #[test]
    fn known_match_on_chip_device_is_out_of_reach() {
        // goodixmoc — matches on the chip; no frame reaches the host.
        let report = ProbeReport::new(0x27c6, 0x5840);
        assert_eq!(report.family, FamilyClass::MatchOnChip);
        assert_eq!(report.driver, Some("goodixmoc"));
        assert_eq!(report.reach(), Reach::MatchOnChip);

        let text = report.render(false);
        assert!(text.contains("MATCH-ON-CHIP"));
        assert!(text.contains("out of reach"));
        assert!(text.contains("returns no frame"));
        assert!(text.contains("docs/adding-a-driver.md"));
        assert!(!text.contains("ImageDevice<UsbFrameSource<_>>"));

        let json = report.to_json();
        assert_eq!(json["family"], "MATCH-ON-CHIP");
        assert_eq!(json["reach"], "match-on-chip");
    }

    #[test]
    fn unknown_device_is_classified_unknown() {
        let report = ProbeReport::new(0x1234, 0x5678);
        assert_eq!(report.family, FamilyClass::Unknown);
        assert_eq!(report.driver, None);
        assert_eq!(report.reach(), Reach::Unknown);

        let text = report.render(false);
        assert!(text.contains("1234:5678"));
        assert!(text.contains("(unknown to libfprint)"));
        assert!(text.contains("UNKNOWN"));
        assert!(text.contains("host-image if it streams grayscale frames"));
        assert!(text.contains("shell --vid 1234 --pid 5678"));

        let json = report.to_json();
        assert_eq!(json["family"], "UNKNOWN");
        assert_eq!(json["reach"], "unknown");
        assert_eq!(json["driver"], serde_json::Value::Null);
    }

    #[test]
    fn colored_render_carries_ansi() {
        let report = ProbeReport::new(0x138a, 0x0011);
        assert!(report.render(true).contains('\u{1b}'));
    }

    #[test]
    fn all_groups_cover_the_database() {
        let text = render_all(false);
        assert!(text.contains("HOST-IMAGE"));
        assert!(text.contains("MATCH-ON-CHIP"));
        assert!(text.contains("vfs5011"));
        assert!(text.contains("goodixmoc"));

        let json = all_json();
        let host = json["host-image"].as_array().unwrap().len();
        let moc = json["match-on-chip"].as_array().unwrap().len();
        let other = json["other"].as_array().unwrap().len();
        assert_eq!(host + moc + other, device_db::all().len());
    }
}
