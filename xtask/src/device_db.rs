// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Regenerating the native backend's device database from the libfprint driver id-tables.
//!
//! The generated table is a set of interoperability facts — which `(vid, pid)` pairs a given
//! libfprint driver claims, and whether that driver produces host-side images or matches on the
//! chip. Those facts let `fpdev probe` tell a user, before they write a line of driver code,
//! whether their sensor is reachable by the host-image `FrameSource` seam.
//!
//! Only facts cross the boundary. The vid/pid numbers, the owning driver name, and each driver's
//! archetype are extracted from the reference tree; no C code or expression is copied, and the
//! emitted Rust is original under this crate's license.
//!
//! Regeneration is deliberate: it overwrites a committed file whose whole point is to be stable
//! between runs. On a clean tree, running it must leave no diff.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The driver tree we read the id-tables out of, relative to the repository root.
const DRIVERS_DIR: &str = "reference/libfprint/libfprint/drivers";

/// Where the generated table lands.
const OUT_FILE: &str = "crates/fprint-backend-native/src/device_db.rs";

/// The xtask command that regenerates the table, named in the generated header.
const COMMAND: &str = "cargo xtask device-db";

/// Match-on-chip drivers: comparison happens on the sensor, so no image ever reaches the host and
/// the `FrameSource` seam cannot drive them. This is every driver under a `*moc/` subdirectory
/// (`egismoc`/`egis_etu905`, `elanmoc`, `focaltech_moc`, `fpcmoc`, `goodixmoc`, `mafpmoc`) plus
/// synaptics, whose `bmkt` protocol enrolls and matches on the device.
const MATCH_ON_CHIP: &[&str] = &[
    "egis_etu905",
    "egismoc",
    "elanmoc",
    "focaltech_moc",
    "fpcmoc",
    "goodixmoc",
    "mafpmoc",
    "synaptics",
];

/// Host-image drivers: the sensor streams pixels and all matching runs on the host, so the
/// `FrameSource` seam can carry them. These are the `FpImageDevice`-family drivers (the aes*
/// series, elan, elanspi, etes603, nb1010, secugen, uru4000, vcom5s, and the upek*/vfs* families).
/// `upekts` and `elanspi` derive from `FP_TYPE_DEVICE` rather than `FpImageDevice` but still hand a
/// host-side image to the matcher, so they belong here.
const HOST_IMAGE: &[&str] = &[
    "aes1610",
    "aes1660",
    "aes2501",
    "aes2550",
    "aes2660",
    "aes3500",
    "aes4000",
    "egis0570",
    "elan",
    "elanspi",
    "etes603",
    "nb1010",
    "secugen",
    "upeksonly",
    "upektc",
    "upektc_img",
    "upekts",
    "uru4000",
    "vcom5s",
    "vfs0050",
    "vfs101",
    "vfs301",
    "vfs5011",
    "vfs7552",
];

/// A driver's archetype, decided by its position in the two lists above.
///
/// Anything in neither list is `Other`: a driver we will not silently pin to a seam it may not
/// serve. Today that is `realtek`, which derives from `FP_TYPE_DEVICE` and does its own on-chip
/// enroll/identify but is not in the enumerated match-on-chip set.
fn family_of(driver: &str) -> &'static str {
    if MATCH_ON_CHIP.contains(&driver) {
        "MatchOnChip"
    } else if HOST_IMAGE.contains(&driver) {
        "HostImage"
    } else {
        "Other"
    }
}

/// One extracted fact: a `(vid, pid)` a driver claims.
struct Record {
    vid: u16,
    pid: u16,
    driver: String,
    family: &'static str,
}

pub fn regenerate(root: &Path) -> Result<(), String> {
    let drivers = root.join(DRIVERS_DIR);
    if !drivers.is_dir() {
        return Err(format!(
            "{} not found — clone the reference tree first (`mise run clone-ref`)",
            drivers.display()
        ));
    }

    let files = source_files(&drivers)?;

    // Symbolic vids (ELAN_VEND_ID, SYNAPTICS_VENDOR_ID, …) are `#define`d near their tables. Read
    // every simple numeric define across the tree once, then resolve names against it.
    let mut constants: BTreeMap<String, u16> = BTreeMap::new();
    for file in &files {
        let text = read(file)?;
        collect_defines(&text, &mut constants);
    }

    // Keyed by (vid, pid) so a pair claimed by two drivers keeps exactly one row. The key order is
    // also the emit order, which makes the output deterministic.
    let mut records: BTreeMap<(u16, u16), Record> = BTreeMap::new();
    for file in &files {
        let text = read(file)?;
        let component = fp_component(&text);
        for (name, body) in id_tables(&text) {
            // Only real driver tables: the plain `id_table` and the named `*_id_table`. The
            // virtual test devices declare a `driver_ids` array, which is not hardware.
            let Some(driver) = driver_name(&name, component.as_deref()) else {
                continue;
            };
            let family = family_of(&driver);
            for (vid, pid) in entries(&body, &constants, file)? {
                let record = Record {
                    vid,
                    pid,
                    driver: driver.clone(),
                    family,
                };
                // First driver wins for a shared pair; the tie-break is driver name so the choice
                // is stable. The only collision today is 147e:2016 (upeksonly vs upektc_img), both
                // host-image, so the family is unambiguous either way.
                records
                    .entry((vid, pid))
                    .and_modify(|existing| {
                        if record.driver < existing.driver {
                            *existing = Record {
                                vid,
                                pid,
                                driver: record.driver.clone(),
                                family,
                            };
                        }
                    })
                    .or_insert(record);
            }
        }
    }

    if records.is_empty() {
        return Err(format!(
            "no id-table entries found under {} — is the reference tree the one we read?",
            drivers.display()
        ));
    }

    let rows: Vec<&Record> = records.values().collect();
    let out = root.join(OUT_FILE);
    std::fs::write(&out, render(&rows)).map_err(|e| format!("write {}: {e}", out.display()))?;

    let families = |f: &str| rows.iter().filter(|r| r.family == f).count();
    println!(
        "xtask: wrote {} ({} devices: {} host-image, {} match-on-chip, {} other)",
        out.display(),
        rows.len(),
        families("HostImage"),
        families("MatchOnChip"),
        families("Other"),
    );
    Ok(())
}

/// Every `.c`/`.h` under the driver tree, sorted so the walk order is not the filesystem's.
fn source_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    walk(dir, &mut out)?;
    out.retain(|p| {
        matches!(
            p.extension().and_then(|e| e.to_str()),
            Some("c") | Some("h")
        )
    });
    out.sort();
    Ok(out)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
        let path = entry
            .map_err(|e| format!("read {}: {e}", dir.display()))?
            .path();
        if path.is_dir() {
            walk(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

fn read(file: &Path) -> Result<String, String> {
    std::fs::read_to_string(file).map_err(|e| format!("read {}: {e}", file.display()))
}

/// Collect `#define NAME <number>` pairs whose value is a bare hex or decimal literal.
///
/// That is all the id-tables reach for — a handful of vendor-id defines. Anything with an
/// expression on the right is not a vid/pid and is skipped.
fn collect_defines(text: &str, into: &mut BTreeMap<String, u16>) {
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("#define ") else {
            continue;
        };
        let mut it = rest.split_whitespace();
        let (Some(name), Some(value)) = (it.next(), it.next()) else {
            continue;
        };
        if it.next().is_some() {
            continue; // more than one token: an expression, not a literal.
        }
        if let Some(n) = parse_u16(value) {
            into.insert(name.to_string(), n);
        }
    }
}

/// The `FP_COMPONENT` string of a driver file, which is its driver id when its table is the
/// unadorned `id_table`.
fn fp_component(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("#define FP_COMPONENT ") {
            return rest
                .trim()
                .trim_matches('"')
                .split('"')
                .next()
                .map(str::to_string);
        }
    }
    None
}

/// The driver a table belongs to: the file's `FP_COMPONENT` for a plain `id_table`, or the prefix
/// before `_id_table` for a named one (`elan_id_table` → `elan`).
fn driver_name(table: &str, component: Option<&str>) -> Option<String> {
    if table == "id_table" {
        component.map(str::to_string)
    } else {
        table.strip_suffix("_id_table").map(str::to_string)
    }
}

/// Every `static const FpIdEntry <name>[] = { … }` block: its name and its brace-delimited body.
fn id_tables(text: &str) -> Vec<(String, String)> {
    const MARKER: &str = "FpIdEntry";
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut search = 0;
    while let Some(rel) = text[search..].find(MARKER) {
        let after = search + rel + MARKER.len();
        search = after;
        let mut i = after;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let name_start = i;
        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
            i += 1;
        }
        if i == name_start {
            continue;
        }
        let name = &text[name_start..i];
        // Expect `[]` then the opening brace, tolerating whitespace between the tokens.
        let rest = text[i..].trim_start();
        let Some(rest) = rest.strip_prefix("[]") else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start();
        if !rest.starts_with('{') {
            continue;
        }
        let brace = text.len() - rest.len();
        if let Some(body) = brace_body(text, brace) {
            out.push((name.to_string(), body));
        }
    }
    out
}

/// The text between `{` at `open` and its matching `}`.
fn brace_body(text: &str, open: usize) -> Option<String> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(text[open + 1..i].to_string());
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Parse the `(vid, pid)` pairs out of a table body.
///
/// One entry per line in every table we read. Lines that carry `hid_id`/`spi_acpi_id`/`udev_types`
/// are SPI companions (elanspi), whose vid/pid name the touchpad rather than a USB sensor, so they
/// are skipped — that driver contributes no USB record. The terminating `{0, 0}` entry is dropped.
fn entries(
    body: &str,
    constants: &BTreeMap<String, u16>,
    file: &Path,
) -> Result<Vec<(u16, u16)>, String> {
    let mut out = Vec::new();
    for line in body.lines() {
        if line.contains("hid_id") || line.contains("spi_acpi_id") || line.contains("udev_types") {
            continue;
        }
        let (Some(vid), Some(pid)) = (field(line, ".vid"), field(line, ".pid")) else {
            continue;
        };
        let vid = resolve(vid, constants)
            .ok_or_else(|| format!("{}: cannot resolve vid `{vid}`", file.display()))?;
        let pid = resolve(pid, constants)
            .ok_or_else(|| format!("{}: cannot resolve pid `{pid}`", file.display()))?;
        if vid == 0 && pid == 0 {
            continue; // terminating entry
        }
        out.push((vid, pid));
    }
    Ok(out)
}

/// The token assigned to `key` on a line: the text after `key =` up to the next `,` or `}`.
fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = line.split_once(key)?.1.trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let end = rest.find([',', '}']).unwrap_or(rest.len());
    Some(rest[..end].trim())
}

/// A literal token, or a named constant looked up in the define map.
fn resolve(token: &str, constants: &BTreeMap<String, u16>) -> Option<u16> {
    parse_u16(token).or_else(|| constants.get(token).copied())
}

/// A bare `0x…` hex or decimal `u16` literal, ignoring any C integer suffix.
fn parse_u16(token: &str) -> Option<u16> {
    let token = token.trim().trim_end_matches(['u', 'U', 'l', 'L']);
    if let Some(hex) = token
        .strip_prefix("0x")
        .or_else(|| token.strip_prefix("0X"))
    {
        u16::from_str_radix(hex, 16).ok()
    } else {
        token.parse().ok()
    }
}

/// Render the generated `device_db.rs`.
fn render(rows: &[&Record]) -> String {
    let mut s = String::new();
    // REUSE-IgnoreStart
    s.push_str(
        "// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors\n\
         //\n\
         // SPDX-License-Identifier: MIT OR Apache-2.0\n\n",
    );
    // REUSE-IgnoreEnd
    s.push_str(&format!(
        "//! A classification of known USB fingerprint sensors, generated from the libfprint driver\n\
         //! id-tables — do not edit by hand; regenerate with `{COMMAND}`.\n\
         //!\n\
         //! Each row is an interoperability fact: a `(vid, pid)` a libfprint driver claims, the\n\
         //! driver that claims it, and whether that driver produces a host-side image (so the\n\
         //! [`crate::FrameSource`] seam can reach it) or matches on the chip. `fpdev probe` reads\n\
         //! it to tell a user whether their sensor is reachable before they start on a driver.\n\
         //!\n\
         //! Rows are sorted by `(vid, pid)` and each pair appears once.\n\n",
    ));
    s.push_str(
        "/// How a sensor's driver does matching, which decides whether a host-image seam can reach it.\n\
         #[derive(Debug, Clone, Copy, PartialEq, Eq)]\n\
         pub enum Family {\n\
         \x20   /// Streams pixels to the host; all matching runs here, so [`crate::FrameSource`] can drive it.\n\
         \x20   HostImage,\n\
         \x20   /// Enrolls and matches on the sensor; no image reaches the host.\n\
         \x20   MatchOnChip,\n\
         \x20   /// Neither archetype fits with confidence; not pinned to a seam.\n\
         \x20   Other,\n\
         }\n\n\
         /// One known device: the USB ids it presents, the libfprint driver that claims it, and how\n\
         /// that driver matches.\n\
         #[derive(Debug, Clone, Copy, PartialEq, Eq)]\n\
         pub struct DeviceRecord {\n\
         \x20   pub vid: u16,\n\
         \x20   pub pid: u16,\n\
         \x20   /// The libfprint driver id that claims this device.\n\
         \x20   pub driver: &'static str,\n\
         \x20   pub family: Family,\n\
         }\n\n",
    );
    s.push_str(&format!(
        "/// Every classified device, sorted by `(vid, pid)`; {} rows, no duplicate keys.\n",
        rows.len()
    ));
    s.push_str("pub static DEVICES: &[DeviceRecord] = &[\n");
    // The multi-line record form is what rustfmt settles on, so the emitted file is already
    // formatted — regeneration on a checked-in tree leaves no `cargo fmt` diff.
    for r in rows {
        s.push_str(&format!(
            "    DeviceRecord {{\n\
             \x20       vid: 0x{:04x},\n\
             \x20       pid: 0x{:04x},\n\
             \x20       driver: {:?},\n\
             \x20       family: Family::{},\n\
             \x20   }},\n",
            r.vid, r.pid, r.driver, r.family
        ));
    }
    s.push_str("];\n\n");
    s.push_str(
        "/// The record for a `(vid, pid)`, if one is known.\n\
         ///\n\
         /// A binary search over [`DEVICES`], which is sorted by key.\n\
         #[must_use]\n\
         pub fn lookup(vid: u16, pid: u16) -> Option<&'static DeviceRecord> {\n\
         \x20   DEVICES\n\
         \x20       .binary_search_by(|r| (r.vid, r.pid).cmp(&(vid, pid)))\n\
         \x20       .ok()\n\
         \x20       .map(|i| &DEVICES[i])\n\
         }\n\n\
         /// The whole table.\n\
         #[must_use]\n\
         pub fn all() -> &'static [DeviceRecord] {\n\
         \x20   DEVICES\n\
         }\n",
    );
    s
}
