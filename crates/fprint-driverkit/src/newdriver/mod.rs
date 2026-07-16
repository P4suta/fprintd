// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `fpdev new-driver`: scaffold a working host-image driver for an unsupported sensor.
//!
//! The generator renders a five-file tree — `mod.rs`, `proto.rs`, `source.rs`, `<name>.rs`, and
//! `mock_tests.rs` — that mirrors the `usb/` worked example exactly. What it produces is not an empty
//! stub: the mock test scripts a reference finger through a `ScriptedTransport` and drives an
//! `ImageDevice` to enroll and self-verify, so a freshly generated driver is green from minute one.
//! The living proof that a driver of this shape captures and matches on real bytes is the `vfs5011`
//! driver under `usb/`, which shares this exact layering; a second real driver need not be committed.
//!
//! The templates ([`include_str!`] of `templates/*.rs.tmpl`) are **original code** modeled on that
//! worked example, parameterized with `{{name}}` / `{{Name}}` / `{{vid}}` / `{{pid}}` / `{{from}}`.
//! Every device value they emit is tagged `HW-verified: required`, because the scaffold states only
//! interoperability facts and never asserts a byte it has not observed (see `docs/adding-a-driver.md`).
//!
//! [`--check`](NewDriverOptions::check) is the template-fidelity gate: it re-renders in memory and
//! diffs against the committed golden fixture, so a template edited without its golden (or the
//! reverse) fails loudly instead of drifting.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The driver archetype requested on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// A sensor that streams pixels to the host: the [`FrameSource`](fprint_backend_native::FrameSource) seam reaches it, and the scaffold is generated.
    HostImage,
    /// A sensor that matches on the chip: the host-image seam cannot reach it, so nothing is scaffolded.
    MatchOnChip,
}

/// A rendered file: its path relative to the driver directory, and its contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedFile {
    /// File name inside the `<name>/` directory (e.g. `proto.rs`).
    pub name: String,
    /// The fully rendered file body.
    pub contents: String,
}

/// What `fpdev new-driver` was asked to do, with the ids parsed and the name validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDriverOptions {
    /// The driver name: a lowercase snake identifier, used for the directory and module names.
    pub name: String,
    /// USB vendor id.
    pub vid: u16,
    /// USB product id.
    pub pid: u16,
    /// The archetype requested.
    pub family: Family,
    /// The worked example the scaffold is modeled on, recorded in the provenance note.
    pub from: String,
    /// Where to write the tree; `None` means the real driver location under `fprint-backend-native`.
    pub out: Option<PathBuf>,
    /// Diff a fresh render against the committed golden fixture instead of writing anything.
    pub check: bool,
}

impl NewDriverOptions {
    /// Build options from the raw CLI strings, parsing the hex ids and validating the name.
    ///
    /// # Errors
    /// Returns [`NewDriverError`] if the name is not a lowercase snake identifier or an id is not
    /// 16-bit hex.
    pub fn from_args(
        name: &str,
        vid: &str,
        pid: &str,
        family: Family,
        from: &str,
        out: Option<PathBuf>,
        check: bool,
    ) -> Result<Self, NewDriverError> {
        Ok(Self {
            name: validate_name(name)?,
            vid: parse_id("vid", vid)?,
            pid: parse_id("pid", pid)?,
            family,
            from: from.to_owned(),
            out,
            check,
        })
    }
}

/// A failure while scaffolding a driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewDriverError {
    /// `--name` was not a lowercase snake identifier (`^[a-z][a-z0-9_]*$`).
    BadName(String),
    /// A `--vid`/`--pid` value was not valid 16-bit hex.
    BadHex {
        /// Which argument (`"vid"` or `"pid"`).
        field: &'static str,
        /// The offending value, as given.
        value: String,
    },
    /// `--family match-on-chip` was asked to scaffold; the host-image seam does not reach it.
    MatchOnChipNotScaffolded,
    /// A `--check` run drifted from the golden fixture, or the fixture is missing.
    CheckFailed(String),
    /// An I/O failure while writing the tree or patching the module list.
    Io(String),
}

impl std::fmt::Display for NewDriverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadName(name) => write!(
                f,
                "--name `{name}` must be a lowercase snake identifier, e.g. `acme` or `acme_x`"
            ),
            Self::BadHex { field, value } => {
                write!(
                    f,
                    "--{field} `{value}` is not 16-bit hex (e.g. 1c7a or 0x1c7a)"
                )
            }
            Self::MatchOnChipNotScaffolded => write!(
                f,
                "match-on-chip is not scaffolded: the host-image FrameSource seam does not reach a \
                 sensor that matches on the chip and returns no frame.\n\
                 See docs/adding-a-driver.md — match-on-chip bring-up is a different path."
            ),
            Self::CheckFailed(diff) => {
                write!(f, "template drifted from the golden fixture:\n{diff}")
            }
            Self::Io(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for NewDriverError {}

/// Validate `name` as a lowercase snake identifier and return it owned.
fn validate_name(name: &str) -> Result<String, NewDriverError> {
    let mut chars = name.chars();
    let ok_first = chars.next().is_some_and(|c| c.is_ascii_lowercase());
    let ok_rest = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if ok_first && ok_rest {
        Ok(name.to_owned())
    } else {
        Err(NewDriverError::BadName(name.to_owned()))
    }
}

/// Parse one hex id, tolerating a `0x`/`0X` prefix, into a `u16`.
fn parse_id(field: &'static str, value: &str) -> Result<u16, NewDriverError> {
    let digits = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    if digits.is_empty() {
        return Err(NewDriverError::BadHex {
            field,
            value: value.to_owned(),
        });
    }
    u16::from_str_radix(digits, 16).map_err(|_| NewDriverError::BadHex {
        field,
        value: value.to_owned(),
    })
}

/// Convert a lowercase snake name into UpperCamelCase for type names (`acme_x` → `AcmeX`).
fn upper_camel(name: &str) -> String {
    name.split('_')
        .filter(|s| !s.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// One template and the file name it renders to (the device module carries the driver's own name).
struct Template {
    /// File name inside the `<name>/` directory; `None` means "use `<name>.rs`".
    name: Option<&'static str>,
    body: &'static str,
}

const TEMPLATES: [Template; 5] = [
    Template {
        name: Some("mod.rs"),
        body: include_str!("templates/mod.rs.tmpl"),
    },
    Template {
        name: Some("proto.rs"),
        body: include_str!("templates/proto.rs.tmpl"),
    },
    Template {
        name: Some("source.rs"),
        body: include_str!("templates/source.rs.tmpl"),
    },
    Template {
        name: None,
        body: include_str!("templates/device.rs.tmpl"),
    },
    Template {
        name: Some("mock_tests.rs"),
        body: include_str!("templates/mock_tests.rs.tmpl"),
    },
];

/// Substitute the placeholders in one template body.
fn render_body(body: &str, opts: &NewDriverOptions) -> String {
    body.replace("{{name}}", &opts.name)
        .replace("{{Name}}", &upper_camel(&opts.name))
        .replace("{{vid}}", &format!("0x{:04x}", opts.vid))
        .replace("{{pid}}", &format!("0x{:04x}", opts.pid))
        .replace("{{from}}", &opts.from)
}

/// Render the whole driver tree in a deterministic order.
///
/// The order is stable (`mod` → `proto` → `source` → device → `mock_tests`), so a golden comparison
/// and a real write see the files identically.
#[must_use]
pub fn render(opts: &NewDriverOptions) -> Vec<GeneratedFile> {
    TEMPLATES
        .iter()
        .map(|t| GeneratedFile {
            name: t
                .name
                .map_or_else(|| format!("{}.rs", opts.name), str::to_owned),
            contents: render_body(t.body, opts),
        })
        .collect()
}

/// The committed golden fixture directory for a driver of the given `name`.
fn golden_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join(format!("newdriver_{name}"))
}

/// The real driver location under `fprint-backend-native`, resolved from this crate's manifest so it
/// is independent of the caller's working directory.
fn default_out_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fprint-backend-native")
        .join("src")
        .join("usb")
        .join("drivers")
        .join(name)
}

/// Diff a fresh render against the committed golden fixture.
///
/// Returns `Ok(())` when every rendered file byte-matches its golden counterpart; otherwise a
/// readable, line-oriented report of the first divergence in each file.
fn check_against_golden(opts: &NewDriverOptions) -> Result<(), NewDriverError> {
    let dir = golden_dir(&opts.name);
    if !dir.is_dir() {
        return Err(NewDriverError::CheckFailed(format!(
            "no golden fixture at {} — generate one with `fpdev new-driver --out <dir>` and commit it",
            dir.display()
        )));
    }

    let mut report = String::new();
    for file in render(opts) {
        let path = dir.join(&file.name);
        match std::fs::read_to_string(&path) {
            Ok(golden) if golden == file.contents => {}
            Ok(golden) => {
                let _ = write!(
                    report,
                    "{}",
                    first_divergence(&file.name, &golden, &file.contents)
                );
            }
            Err(_) => {
                let _ = writeln!(report, "  {} — missing from the golden fixture", file.name);
            }
        }
    }

    if report.is_empty() {
        Ok(())
    } else {
        Err(NewDriverError::CheckFailed(report))
    }
}

/// Render the first line at which `golden` and `fresh` differ, with a little context.
fn first_divergence(name: &str, golden: &str, fresh: &str) -> String {
    let mut out = format!("  {name}\n");
    let golden_lines: Vec<&str> = golden.lines().collect();
    let fresh_lines: Vec<&str> = fresh.lines().collect();
    let max = golden_lines.len().max(fresh_lines.len());
    for i in 0..max {
        let g = golden_lines.get(i).copied();
        let f = fresh_lines.get(i).copied();
        if g != f {
            let _ = writeln!(out, "    line {}:", i + 1);
            let _ = writeln!(out, "      golden: {}", g.unwrap_or("<end of file>"));
            let _ = writeln!(out, "      fresh:  {}", f.unwrap_or("<end of file>"));
            return out;
        }
    }
    out
}

/// Write the rendered tree to `dir`, creating it, then patch the module list so `cargo test` finds it.
fn write_tree(opts: &NewDriverOptions, dir: &Path) -> Result<Vec<PathBuf>, NewDriverError> {
    std::fs::create_dir_all(dir)
        .map_err(|e| NewDriverError::Io(format!("create {}: {e}", dir.display())))?;

    let mut written = Vec::new();
    for file in render(opts) {
        let path = dir.join(&file.name);
        std::fs::write(&path, &file.contents)
            .map_err(|e| NewDriverError::Io(format!("write {}: {e}", path.display())))?;
        written.push(path);
    }

    // Only patch the backend's module list when writing to the real driver location. An `--out`
    // scaffold is a standalone tree the author places themselves, so it patches nothing.
    if opts.out.is_none() {
        patch_module_lists(&opts.name, dir)?;
    }
    Ok(written)
}

/// Declare the new driver in the backend's module tree: `usb/mod.rs` gets `pub mod drivers;`, and
/// `usb/drivers/mod.rs` gets `pub mod <name>;`.
fn patch_module_lists(name: &str, driver_dir: &Path) -> Result<(), NewDriverError> {
    let drivers_dir = driver_dir
        .parent()
        .ok_or_else(|| NewDriverError::Io(format!("{} has no parent", driver_dir.display())))?;
    let usb_dir = drivers_dir
        .parent()
        .ok_or_else(|| NewDriverError::Io(format!("{} has no parent", drivers_dir.display())))?;

    ensure_mod_line(&usb_dir.join("mod.rs"), "pub mod drivers;")?;
    ensure_drivers_mod(&drivers_dir.join("mod.rs"), name)?;
    Ok(())
}

/// Ensure `path` (an existing module file) contains `line` among its module declarations.
fn ensure_mod_line(path: &Path, line: &str) -> Result<(), NewDriverError> {
    let body = std::fs::read_to_string(path)
        .map_err(|e| NewDriverError::Io(format!("read {}: {e}", path.display())))?;
    if body.lines().any(|l| l.trim() == line) {
        return Ok(());
    }
    let mut lines: Vec<String> = body.lines().map(str::to_owned).collect();
    let at = lines
        .iter()
        .position(|l| l.starts_with("mod ") || l.starts_with("pub mod "))
        .unwrap_or(lines.len());
    lines.insert(at, line.to_owned());
    std::fs::write(path, lines.join("\n") + "\n")
        .map_err(|e| NewDriverError::Io(format!("write {}: {e}", path.display())))
}

/// Ensure `usb/drivers/mod.rs` exists and declares `pub mod <name>;`, sorted.
fn ensure_drivers_mod(path: &Path, name: &str) -> Result<(), NewDriverError> {
    let decl = format!("pub mod {name};");
    if !path.exists() {
        // REUSE-IgnoreStart
        let body = format!(
            "// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors\n\
             //\n\
             // SPDX-License-Identifier: MIT OR Apache-2.0\n\n\
             //! Contributed native drivers, each a self-contained [`crate::FrameSource`] tree \
             generated by\n\
             //! `fpdev new-driver` and finalized against hardware. See `docs/adding-a-driver.md`.\n\n\
             {decl}\n"
        );
        // REUSE-IgnoreEnd
        return std::fs::write(path, body)
            .map_err(|e| NewDriverError::Io(format!("write {}: {e}", path.display())));
    }

    let body = std::fs::read_to_string(path)
        .map_err(|e| NewDriverError::Io(format!("read {}: {e}", path.display())))?;
    if body.lines().any(|l| l.trim() == decl) {
        return Ok(());
    }
    let mut decls: Vec<String> = body
        .lines()
        .filter(|l| l.starts_with("pub mod "))
        .map(str::to_owned)
        .collect();
    decls.push(decl);
    decls.sort();
    let header: Vec<&str> = body
        .lines()
        .take_while(|l| !l.starts_with("pub mod "))
        .collect();
    let rebuilt = format!("{}\n{}\n", header.join("\n"), decls.join("\n"));
    std::fs::write(path, rebuilt)
        .map_err(|e| NewDriverError::Io(format!("write {}: {e}", path.display())))
}

/// Run the scaffold command, printing to stdout.
///
/// # Errors
/// Returns [`NewDriverError`] for a match-on-chip request, a `--check` drift, or an I/O failure.
pub fn run(opts: &NewDriverOptions) -> Result<(), NewDriverError> {
    if opts.family == Family::MatchOnChip {
        return Err(NewDriverError::MatchOnChipNotScaffolded);
    }

    if opts.check {
        check_against_golden(opts)?;
        println!(
            "fpdev new-driver: `{}` matches its golden fixture — templates are in sync.",
            opts.name
        );
        return Ok(());
    }

    let dir = opts
        .out
        .clone()
        .unwrap_or_else(|| default_out_dir(&opts.name));
    let written = write_tree(opts, &dir)?;

    println!(
        "fpdev new-driver: scaffolded `{}` ({:04x}:{:04x}) — {} files under {}",
        opts.name,
        opts.vid,
        opts.pid,
        written.len(),
        dir.display()
    );
    for path in &written {
        println!("  {}", path.display());
    }
    println!(
        "\nEvery device value is tagged `HW-verified: required`: confirm each against the sensor \
         before real capture.\nThe scaffold's mock_tests already enroll and self-verify a reference \
         finger, so `cargo test` is green now."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(name: &str) -> NewDriverOptions {
        NewDriverOptions::from_args(
            name,
            "1c7a",
            "0570",
            Family::HostImage,
            "vfs5011",
            None,
            false,
        )
        .unwrap()
    }

    #[test]
    fn parses_ids_and_validates_name() {
        let o = opts("acme");
        assert_eq!(o.vid, 0x1c7a);
        assert_eq!(o.pid, 0x0570);
        assert!(matches!(
            NewDriverOptions::from_args("Acme", "1", "2", Family::HostImage, "x", None, false),
            Err(NewDriverError::BadName(_))
        ));
        assert!(matches!(
            NewDriverOptions::from_args("acme", "zz", "2", Family::HostImage, "x", None, false),
            Err(NewDriverError::BadHex { field: "vid", .. })
        ));
    }

    #[test]
    fn upper_camel_handles_snake() {
        assert_eq!(upper_camel("acme"), "Acme");
        assert_eq!(upper_camel("acme_x"), "AcmeX");
    }

    #[test]
    fn render_produces_the_five_file_tree() {
        let files = render(&opts("acme"));
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "mod.rs",
                "proto.rs",
                "source.rs",
                "acme.rs",
                "mock_tests.rs"
            ]
        );
    }

    // REUSE-IgnoreStart
    #[test]
    fn every_file_carries_the_spdx_header_and_substitutes_placeholders() {
        for file in render(&opts("acme")) {
            assert!(
                file.contents
                    .contains("SPDX-License-Identifier: MIT OR Apache-2.0"),
                "{} is missing the SPDX header",
                file.name
            );
            assert!(
                !file.contents.contains("{{"),
                "{} still has an unsubstituted placeholder",
                file.name
            );
        }
    }
    // REUSE-IgnoreEnd

    #[test]
    fn device_file_tags_every_value_hw_verified() {
        let files = render(&opts("acme"));
        let device = files.iter().find(|f| f.name == "acme.rs").unwrap();
        assert!(device.contents.contains("HW-verified: required"));
        assert!(device.contents.contains("VENDOR_ID: u16 = 0x1c7a"));
        assert!(device.contents.contains("PRODUCT_ID: u16 = 0x0570"));
    }

    #[test]
    fn mock_tests_reference_the_offline_harness_types() {
        let files = render(&opts("acme"));
        let mock = files.iter().find(|f| f.name == "mock_tests.rs").unwrap();
        assert!(mock.contents.contains("SyntheticFrameSource"));
        assert!(mock.contents.contains("ScriptedTransport"));
        assert!(mock.contents.contains("AcmeFrameSource"));
    }

    #[test]
    fn match_on_chip_is_not_scaffolded() {
        let o = NewDriverOptions::from_args(
            "acme",
            "1c7a",
            "0570",
            Family::MatchOnChip,
            "vfs5011",
            None,
            false,
        )
        .unwrap();
        assert!(matches!(
            run(&o),
            Err(NewDriverError::MatchOnChipNotScaffolded)
        ));
    }
}
