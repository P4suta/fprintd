// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `fpdev shell`: a line-oriented control/bulk REPL for poking an unknown sensor in this project's
//! own vocabulary.
//!
//! One command is one USB exchange — `control`, `bulkout`, `bulkin` — issued over the same
//! [`UsbTransport`] seam the drivers speak. Two backends sit behind that seam: an offline, in-memory
//! [`ScriptedTransport`] (the default, seeded from a `--replay` file) and, behind the `usb` feature,
//! the live `NusbTransport`. Because both implement the one trait, the whole REPL is generic over
//! it, so a scripted dry-run and real hardware run identical code.
//!
//! The engine is [`run_script`]: a pure `(transport, input) -> transcript` function with no terminal
//! and no async runtime of its own, so a fixed list of input lines renders a deterministic transcript
//! a test can assert on. The interactive loop ([`run`]) is a thin reader over the same per-line
//! executor.
//!
//! A `bulkin` read is auto-annotated: the bytes are sniffed for the driver's frame-header layout,
//! hex-dumped, and scored for entropy and printability, so an unknown stream tells you at a glance
//! whether it looks like an image header or opaque payload. The sniffer is a heuristic that mirrors
//! the backend's documented header shape (`usb::proto`); it never claims to be the authoritative
//! parser, and the golden test seeds real header bytes through [`ScriptedTransport::push_frame`] so
//! the two cannot silently drift.

use std::fmt::Write as _;
use std::io::{BufRead as _, IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};

use fprint_backend_native::{
    ScriptedTransport, UsbId, UsbTransport, FRAME_HEADER_LEN, FRAME_MAGIC,
};
use owo_colors::{OwoColorize as _, Style};
use pollster::block_on;

/// Cap on the bytes a single hexdump prints, so a full-image read stays readable.
const HEXDUMP_LIMIT: usize = 256;

/// What `fpdev shell` was asked to do, with the ids parsed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShellOptions {
    /// The live device to open, as a parsed `(vid, pid)`. `None` means no selector was given.
    pub selector: Option<(u16, u16)>,
    /// A recording of device-to-host bytes to seed the offline dry-run transport.
    pub replay: Option<PathBuf>,
}

impl ShellOptions {
    /// Build options from the raw CLI strings, parsing the hex ids.
    ///
    /// `vid`/`pid` accept `0x`-prefixed or bare hex and must be supplied together; a lone one is a
    /// usage error.
    ///
    /// # Errors
    /// Returns [`ShellError`] if an id fails to parse or only one of `vid`/`pid` is given.
    pub fn from_args(
        vid: Option<&str>,
        pid: Option<&str>,
        replay: Option<PathBuf>,
    ) -> Result<Self, ShellError> {
        let selector = match (vid, pid) {
            (Some(v), Some(p)) => Some((parse_id("vid", v)?, parse_id("pid", p)?)),
            (None, None) => None,
            (Some(_), None) => return Err(ShellError::LoneSelector("pid")),
            (None, Some(_)) => return Err(ShellError::LoneSelector("vid")),
        };
        Ok(Self { selector, replay })
    }
}

/// A failure while parsing shell arguments or loading a replay file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellError {
    /// A `--vid`/`--pid` value was not valid 16-bit hex.
    BadHex {
        /// Which argument (`"vid"` or `"pid"`).
        field: &'static str,
        /// The offending value, as given.
        value: String,
    },
    /// Only one of `--vid`/`--pid` was supplied; both or neither are required.
    LoneSelector(&'static str),
    /// A `--replay` file could not be read, or a line was not valid hex.
    Replay(String),
}

impl std::fmt::Display for ShellError {
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
            Self::Replay(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ShellError {}

/// Parse one hex id, tolerating a `0x`/`0X` prefix, into a `u16`.
fn parse_id(field: &'static str, value: &str) -> Result<u16, ShellError> {
    let digits = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    if digits.is_empty() {
        return Err(ShellError::BadHex {
            field,
            value: value.to_owned(),
        });
    }
    u16::from_str_radix(digits, 16).map_err(|_| ShellError::BadHex {
        field,
        value: value.to_owned(),
    })
}

// --- The REPL engine ----------------------------------------------------------------------------

/// Whether the loop should keep reading or stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Continue,
    Quit,
}

/// Run a whole script of input lines against `transport`, returning the rendered transcript.
///
/// This is the pure heart of the REPL: no terminal, no stdin, no global state — a fixed input
/// renders a byte-identical transcript, which is what makes the shell testable and scriptable.
/// `color` gates ANSI styling so pipes and tests stay plain.
#[must_use]
pub fn run_script<T: UsbTransport>(transport: &mut T, input: &str, color: bool) -> String {
    let p = Palette::new(color);
    let mut out = String::new();
    for line in input.lines() {
        if exec_line(transport, line, &p, &mut out) == Flow::Quit {
            break;
        }
    }
    out
}

/// Execute one input line, appending its rendered output to `out`.
fn exec_line<T: UsbTransport>(
    transport: &mut T,
    line: &str,
    p: &Palette,
    out: &mut String,
) -> Flow {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Flow::Continue;
    }
    let mut words = trimmed.split_whitespace();
    let cmd = words.next().unwrap_or_default();
    let args: Vec<&str> = words.collect();

    match cmd {
        "help" | "?" => {
            out.push_str(&help_text(p));
            Flow::Continue
        }
        "quit" | "exit" | "q" => Flow::Quit,
        "control" => {
            cmd_control(transport, &args, p, out);
            Flow::Continue
        }
        "bulkout" => {
            cmd_bulkout(transport, &args, p, out);
            Flow::Continue
        }
        "bulkin" => {
            cmd_bulkin(transport, &args, p, out);
            Flow::Continue
        }
        other => {
            let _ = writeln!(
                out,
                "{} unknown command `{other}` — try `help`",
                "error:".style(p.err)
            );
            Flow::Continue
        }
    }
}

/// `control <reqType> <req> <wValue> <wIndex> [hex-data]`.
fn cmd_control<T: UsbTransport>(transport: &mut T, args: &[&str], p: &Palette, out: &mut String) {
    if args.len() < 4 {
        return usage(
            out,
            p,
            "control <reqType> <req> <wValue> <wIndex> [hex-data]",
        );
    }
    let (Some(rt), Some(req), Some(value), Some(index)) = (
        parse_int::<u8>(args[0]),
        parse_int::<u8>(args[1]),
        parse_int::<u16>(args[2]),
        parse_int::<u16>(args[3]),
    ) else {
        return error(
            out,
            p,
            "reqType/req are u8 and wValue/wIndex are u16 (hex or decimal)",
        );
    };
    let data = match args.get(4) {
        Some(hex) => match parse_hex_bytes(hex) {
            Ok(bytes) => bytes,
            Err(e) => return error(out, p, &e),
        },
        None => Vec::new(),
    };

    let _ = writeln!(
        out,
        "{} type=0x{rt:02x} req=0x{req:02x} value=0x{value:04x} index=0x{index:04x} ({} bytes)",
        "control".style(p.head),
        data.len(),
    );
    match block_on(transport.control(rt, req, value, index, &data)) {
        Ok(resp) if resp.is_empty() => {
            let _ = writeln!(out, "  {}", "ok".style(p.ok));
        }
        Ok(resp) => {
            let _ = writeln!(out, "  {} <- {} bytes", "ok".style(p.ok), resp.len());
            out.push_str(&hexdump(&resp));
        }
        Err(e) => error(out, p, &e.to_string()),
    }
}

/// `bulkout <ep> <hex-data>`.
fn cmd_bulkout<T: UsbTransport>(transport: &mut T, args: &[&str], p: &Palette, out: &mut String) {
    if args.len() < 2 {
        return usage(out, p, "bulkout <ep> <hex-data>");
    }
    let Some(ep) = parse_int::<u8>(args[0]) else {
        return error(out, p, "ep is a u8 (hex or decimal)");
    };
    let data = match parse_hex_bytes(args[1]) {
        Ok(bytes) => bytes,
        Err(e) => return error(out, p, &e),
    };

    let _ = writeln!(
        out,
        "{} ep=0x{ep:02x} ({} bytes)",
        "bulkout".style(p.head),
        data.len()
    );
    match block_on(transport.bulk_out(ep, &data)) {
        Ok(()) => {
            let _ = writeln!(out, "  {}", "ok".style(p.ok));
        }
        Err(e) => error(out, p, &e.to_string()),
    }
}

/// `bulkin <ep> <len>` — read, then auto-annotate the bytes.
fn cmd_bulkin<T: UsbTransport>(transport: &mut T, args: &[&str], p: &Palette, out: &mut String) {
    if args.len() < 2 {
        return usage(out, p, "bulkin <ep> <len>");
    }
    let (Some(ep), Some(len)) = (parse_int::<u8>(args[0]), parse_int::<usize>(args[1])) else {
        return error(
            out,
            p,
            "ep is a u8 and len is a byte count (hex or decimal)",
        );
    };

    let _ = writeln!(out, "{} ep=0x{ep:02x} len={len}", "bulkin".style(p.head));
    match block_on(transport.bulk_in(ep, len)) {
        Ok(bytes) => {
            let _ = writeln!(out, "  {} {} bytes", "read".style(p.ok), bytes.len());
            out.push_str(&hexdump(&bytes));
            annotate(&bytes, p, out);
        }
        Err(e) => error(out, p, &e.to_string()),
    }
}

/// Auto-annotate a just-read buffer: frame-header sniff, then entropy / printability.
fn annotate(bytes: &[u8], p: &Palette, out: &mut String) {
    match sniff_frame_header(bytes) {
        Ok((w, h)) => {
            let valid = w > 0 && h > 0;
            let geometry = if valid {
                "valid geometry".style(p.ok).to_string()
            } else {
                "invalid geometry".style(p.warn).to_string()
            };
            let _ = writeln!(
                out,
                "  frame header? {}  width={w} height={h}  ({geometry})",
                "yes".style(p.ok),
            );
        }
        Err(reason) => {
            let _ = writeln!(out, "  frame header? {}  ({reason})", "no".style(p.warn));
        }
    }

    if bytes.is_empty() {
        return;
    }
    let entropy = shannon_entropy(bytes);
    let printable = bytes
        .iter()
        .filter(|&&b| (0x20..=0x7e).contains(&b))
        .count();
    let pct = printable * 100 / bytes.len();
    let _ = writeln!(
        out,
        "  {}",
        format!("entropy {entropy:.2} bits/byte  {pct}% printable").style(p.dim),
    );
}

/// Sniff `bytes` for the backend's frame-header layout, returning `(width, height)` or why not.
///
/// A heuristic, not a parser: it mirrors `usb::proto`'s documented header (`MAGIC | width LE |
/// height LE`) to answer "does this look like a frame header?". The authoritative parse lives in the
/// backend; this only guesses.
fn sniff_frame_header(bytes: &[u8]) -> Result<(u16, u16), &'static str> {
    if bytes.len() < FRAME_HEADER_LEN {
        return Err("fewer than 6 header bytes");
    }
    if bytes[0..2] != FRAME_MAGIC {
        return Err("magic mismatch");
    }
    let width = u16::from_le_bytes([bytes[2], bytes[3]]);
    let height = u16::from_le_bytes([bytes[4], bytes[5]]);
    Ok((width, height))
}

/// Shannon entropy of a byte buffer in bits per byte (0.0 for an empty buffer).
fn shannon_entropy(bytes: &[u8]) -> f64 {
    if bytes.is_empty() {
        return 0.0;
    }
    let mut counts = [0u32; 256];
    for &b in bytes {
        counts[b as usize] += 1;
    }
    let len = bytes.len() as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let prob = f64::from(c) / len;
            -prob * prob.log2()
        })
        .sum()
}

/// A classic `offset  hex  |ascii|` hexdump, capped at [`HEXDUMP_LIMIT`] bytes.
fn hexdump(bytes: &[u8]) -> String {
    let mut out = String::new();
    let shown = bytes.len().min(HEXDUMP_LIMIT);
    for (row, chunk) in bytes[..shown].chunks(16).enumerate() {
        let mut hex = String::new();
        for b in chunk {
            let _ = write!(hex, "{b:02x} ");
        }
        let ascii: String = chunk
            .iter()
            .map(|&b| {
                if (0x20..=0x7e).contains(&b) {
                    b as char
                } else {
                    '.'
                }
            })
            .collect();
        let _ = writeln!(out, "  {:04x}  {hex:<48}|{ascii}|", row * 16);
    }
    if bytes.len() > shown {
        let _ = writeln!(out, "  ... ({} more bytes)", bytes.len() - shown);
    }
    out
}

/// Parse an integer argument: `0x`/`0X`-prefixed hex, otherwise decimal.
fn parse_int<T: TryFrom<u64>>(s: &str) -> Option<T> {
    let value = if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()?
    } else {
        s.parse::<u64>().ok()?
    };
    T::try_from(value).ok()
}

/// Parse a hex-byte payload, ignoring whitespace and `:`, `_`, `,` separators.
fn parse_hex_bytes(s: &str) -> Result<Vec<u8>, String> {
    let cleaned: String = s
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, ':' | '_' | ','))
        .collect();
    if cleaned.len() % 2 != 0 {
        return Err(format!(
            "hex payload `{s}` has an odd digit count — bytes are two hex digits each"
        ));
    }
    (0..cleaned.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&cleaned[i..i + 2], 16)
                .map_err(|_| format!("`{}` is not a hex byte", &cleaned[i..i + 2]))
        })
        .collect()
}

/// Append a `usage:` line for an unrecognized command.
fn usage(out: &mut String, p: &Palette, form: &str) {
    let _ = writeln!(out, "{} {form}", "usage:".style(p.key));
}

/// Append an `error:` line.
fn error(out: &mut String, p: &Palette, msg: &str) {
    let _ = writeln!(out, "{} {msg}", "error:".style(p.err));
}

/// The `help` command's body.
fn help_text(p: &Palette) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{}", "fpdev shell — commands".style(p.head));
    for (form, what) in [
        (
            "control <reqType> <req> <wValue> <wIndex> [hex]",
            "issue a control transfer",
        ),
        ("bulkout <ep> <hex>", "write bytes to a bulk-out endpoint"),
        ("bulkin <ep> <len>", "read bytes, then auto-annotate them"),
        ("help", "show this list"),
        ("quit", "leave the shell"),
    ] {
        let _ = writeln!(out, "  {:<48}{}", form.style(p.key), what.style(p.dim));
    }
    let _ = writeln!(
        out,
        "  {}",
        "numbers take 0x-hex or decimal; hex payloads ignore spaces and `:`".style(p.dim),
    );
    out
}

/// Styles for one render pass; every field is a no-op [`Style`] when color is off, so the same
/// rendering path serves terminals, pipes, and tests.
struct Palette {
    head: Style,
    key: Style,
    dim: Style,
    ok: Style,
    warn: Style,
    err: Style,
}

impl Palette {
    fn new(color: bool) -> Self {
        if !color {
            let plain = Style::new();
            return Self {
                head: plain,
                key: plain,
                dim: plain,
                ok: plain,
                warn: plain,
                err: plain,
            };
        }
        Self {
            head: Style::new().bold(),
            key: Style::new().cyan(),
            dim: Style::new().dimmed(),
            ok: Style::new().green().bold(),
            warn: Style::new().yellow().bold(),
            err: Style::new().red().bold(),
        }
    }
}

// --- Backends and the interactive entry point ---------------------------------------------------

/// Load a `--replay` file into a dry-run transport.
///
/// The format is deliberately tiny and offline: each non-blank, non-`#` line is one device-to-host
/// bulk-in payload as hex (spaces and `:` ignored), replayed in file order by successive `bulkin`
/// reads. It seeds nothing else — the host's own `control`/`bulkout` writes are recorded as they
/// happen.
fn load_replay(path: &Path) -> Result<ScriptedTransport, ShellError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ShellError::Replay(format!("read {}: {e}", path.display())))?;
    let mut transport = ScriptedTransport::new();
    for (n, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let bytes = parse_hex_bytes(trimmed)
            .map_err(|e| ShellError::Replay(format!("{}:{}: {e}", path.display(), n + 1)))?;
        transport.push_bulk_in(bytes);
    }
    Ok(transport)
}

/// Run the shell for the given options, reading commands from stdin.
///
/// # Errors
/// Returns [`ShellError`] for a bad `--replay` file, or an I/O error while reading stdin.
pub fn run(options: &ShellOptions) -> Result<(), Box<dyn std::error::Error>> {
    let color = std::io::stdout().is_terminal();

    if let Some(path) = &options.replay {
        let mut transport = load_replay(path)?;
        banner(color, &format!("dry-run · replay {}", path.display()));
        repl_stdin(&mut transport, color)?;
        return Ok(());
    }

    if let Some((vid, pid)) = options.selector {
        return run_live(UsbId { vid, pid }, color);
    }

    // No replay and no device: an empty dry-run transport. `control`/`bulkout` record; a `bulkin`
    // reports an exhausted inbox until a `--replay` file seeds device bytes.
    let mut transport = ScriptedTransport::new();
    banner(color, "dry-run · empty transport (seed one with --replay)");
    repl_stdin(&mut transport, color)?;
    Ok(())
}

/// Drive `transport` from stdin, printing each command's transcript as it runs.
fn repl_stdin<T: UsbTransport>(
    transport: &mut T,
    color: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let p = Palette::new(color);
    let stdin = std::io::stdin();
    let interactive = stdin.is_terminal();
    let mut reader = stdin.lock();
    let mut stdout = std::io::stdout().lock();
    let mut line = String::new();
    loop {
        if interactive {
            eprint!("{} ", "fpdev>".style(p.key));
            let _ = std::io::stderr().flush();
        }
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break; // EOF
        }
        let mut out = String::new();
        let flow = exec_line(transport, &line, &p, &mut out);
        stdout.write_all(out.as_bytes())?;
        stdout.flush()?;
        if flow == Flow::Quit {
            break;
        }
    }
    Ok(())
}

/// Print a one-line mode banner to stderr, so a piped stdout transcript stays clean.
fn banner(color: bool, mode: &str) {
    let p = Palette::new(color);
    eprintln!(
        "{} {}  —  type `help`",
        "fpdev shell".style(p.head),
        mode.style(p.dim)
    );
}

/// Open the live device and run the REPL over its real transport.
#[cfg(feature = "usb")]
fn run_live(id: UsbId, color: bool) -> Result<(), Box<dyn std::error::Error>> {
    use fprint_backend_native::NusbTransport;
    let mut transport = NusbTransport::open(id)?;
    banner(color, &format!("live · {:04x}:{:04x}", id.vid, id.pid));
    repl_stdin(&mut transport, color)?;
    Ok(())
}

/// Without the `usb` feature there is no live transport, so say so plainly rather than fake it.
#[cfg(not(feature = "usb"))]
fn run_live(id: UsbId, _color: bool) -> Result<(), Box<dyn std::error::Error>> {
    eprintln!(
        "fpdev shell: live USB not wired (HW-verified: required).\n\
         Build with `--features usb` to drive {:04x}:{:04x} over real hardware, or pass --replay \
         <file> to poke a recorded session offline.",
        id.vid, id.pid
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fprint_backend_native::Frame;

    /// A dry-run transport pre-seeded with `frames` reference captures (header + payload each).
    fn scripted_with(frames: &[Frame]) -> ScriptedTransport {
        let mut t = ScriptedTransport::new();
        for f in frames {
            t.push_frame(f); // real header bytes + payload, exactly as the wire carries them
        }
        t
    }

    fn ref_frame() -> Frame {
        Frame {
            data: vec![0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80],
            width: 4,
            height: 2,
            ppi: 500,
        }
    }

    #[test]
    fn parses_ids_together_or_not_at_all() {
        assert_eq!(
            ShellOptions::from_args(Some("138a"), Some("0x0011"), None).unwrap(),
            ShellOptions {
                selector: Some((0x138a, 0x0011)),
                replay: None
            }
        );
        assert!(matches!(
            ShellOptions::from_args(Some("138a"), None, None),
            Err(ShellError::LoneSelector("pid"))
        ));
        assert!(matches!(
            ShellOptions::from_args(Some("zz"), Some("11"), None),
            Err(ShellError::BadHex { field: "vid", .. })
        ));
    }

    #[test]
    fn parse_int_takes_hex_and_decimal() {
        assert_eq!(parse_int::<u8>("0xc0"), Some(0xc0));
        assert_eq!(parse_int::<u8>("192"), Some(192));
        assert_eq!(parse_int::<u16>("0x0100"), Some(0x0100));
        assert_eq!(parse_int::<u8>("0x100"), None); // overflows u8
        assert_eq!(parse_int::<u8>("nope"), None);
    }

    #[test]
    fn parse_hex_bytes_ignores_separators_and_rejects_odd() {
        assert_eq!(parse_hex_bytes("01fe").unwrap(), vec![0x01, 0xfe]);
        assert_eq!(parse_hex_bytes("01:fe 04").unwrap(), vec![0x01, 0xfe, 0x04]);
        assert!(parse_hex_bytes("012").is_err());
        assert!(parse_hex_bytes("zz").is_err());
    }

    #[test]
    fn bulkin_annotates_a_scripted_frame_header() {
        // The wire delivers a header read then a payload read; the shell must call the first a frame
        // header (with the seeded geometry) and the second opaque payload.
        let frame = ref_frame();
        let mut t = scripted_with(std::slice::from_ref(&frame));

        let script = "bulkin 0x81 6\nbulkin 0x81 8\n";
        let transcript = run_script(&mut t, script, false);

        // No ANSI in a plain render.
        assert!(
            !transcript.contains('\u{1b}'),
            "plain render must not color"
        );

        // Header read: sniffed as a frame header with the exact seeded geometry.
        assert!(
            transcript.contains("frame header? yes  width=4 height=2  (valid geometry)"),
            "header annotation missing:\n{transcript}"
        );
        // The header hexdump shows the magic bytes and their ascii gutter.
        assert!(transcript.contains("0000  01 fe 04 00 02 00"));
        // Both reads carry the entropy / printability note.
        assert_eq!(transcript.matches("bits/byte").count(), 2);
        // Payload read: not a header (its first bytes are pixels, not the magic).
        assert!(transcript.contains("frame header? no  (magic mismatch)"));
        // The payload's own bytes are dumped with their ascii view.
        assert!(transcript.contains("0000  10 20 30 40 50 60 70 80"));
    }

    #[test]
    fn control_and_bulkout_are_recorded_and_acknowledged() {
        let mut t = ScriptedTransport::new();
        let script = "control 0xc0 0x06 0x0100 0x0000 12ff\nbulkout 0x02 50\n";
        let transcript = run_script(&mut t, script, false);

        assert!(transcript.contains("type=0xc0 req=0x06 value=0x0100 index=0x0000 (2 bytes)"));
        assert!(transcript.contains("ep=0x02 (1 bytes)"));
        assert_eq!(transcript.matches("ok").count(), 2);
        // Both writes reached the transport's record.
        assert_eq!(t.sent(), &[vec![0x12, 0xff], vec![0x50]]);
    }

    #[test]
    fn exhausted_inbox_is_a_clean_error_not_a_panic() {
        let mut t = ScriptedTransport::new();
        let transcript = run_script(&mut t, "bulkin 0x81 6\n", false);
        assert!(transcript.contains("error:"));
        assert!(transcript.contains("inbox exhausted"));
    }

    #[test]
    fn quit_stops_reading() {
        let frame = ref_frame();
        let mut t = scripted_with(std::slice::from_ref(&frame));
        // The `bulkin` after `quit` must never run, so the header stays unread.
        let transcript = run_script(&mut t, "help\nquit\nbulkin 0x81 6\n", false);
        assert!(transcript.contains("fpdev shell — commands"));
        assert!(!transcript.contains("frame header?"));
    }

    #[test]
    fn unknown_and_misused_commands_report_without_stopping() {
        let mut t = ScriptedTransport::new();
        let transcript = run_script(&mut t, "frobnicate\nbulkin 0x81\ncontrol 1 2\n", false);
        assert!(transcript.contains("unknown command `frobnicate`"));
        assert!(transcript.contains("usage: bulkin <ep> <len>"));
        assert!(transcript.contains("usage: control"));
    }

    #[test]
    fn colored_render_carries_ansi() {
        let frame = ref_frame();
        let mut t = scripted_with(std::slice::from_ref(&frame));
        let transcript = run_script(&mut t, "bulkin 0x81 6\n", true);
        assert!(transcript.contains('\u{1b}'));
    }
}
