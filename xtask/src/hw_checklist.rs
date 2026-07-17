// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The hardware-verification checklist: which device values a driver asserts without a sensor.
//!
//! A native driver's device constants (VID/PID, endpoints, frame geometry, init/deinit sequences)
//! are interoperability facts confirmable only against a physical sensor. Until then each carries a
//! marker in its doc-comment, and this task collects the unresolved ones so a bring-up sees what is
//! left to confirm.
//!
//! ## The marker convention
//!
//! A device value's state lives in a `// HW-verified:` marker on its declaration:
//!
//! * `// HW-verified: required` — **PENDING**. The value is a placeholder or an unconfirmed fact; no
//!   physical sensor has vouched for it. The scaffold emits this state.
//! * `// HW-verified: confirmed <evidence>` — **RESOLVED**. The value has been checked against
//!   hardware, and `<evidence>` records how (a capture, a descriptor dump, a datasheet reference).
//!
//! A marker is either pending or resolved; the word after the colon is which. A bare
//! `HW-verified: required` stays pending until someone confirms it and writes the evidence in.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Directories that hold no driver source of ours.
const SKIP_DIRS: [&str; 4] = ["target", "reference", ".git", "node_modules"];

/// The token every marker carries.
const MARKER: &str = "HW-verified:";

/// One `HW-verified:` marker on one source line.
struct Marker {
    /// Path from the repository root, forward-slashed.
    file: String,
    line: usize,
    /// The marker itself, from the token to end of line.
    text: String,
    /// `true` for a `confirmed` marker, `false` for a `required` one.
    resolved: bool,
}

/// Every marker of one driver, pending and resolved together.
struct DriverReport {
    driver: String,
    markers: Vec<Marker>,
}

impl DriverReport {
    fn pending(&self) -> usize {
        self.markers.iter().filter(|m| !m.resolved).count()
    }
    fn resolved(&self) -> usize {
        self.markers.iter().filter(|m| m.resolved).count()
    }
    fn total(&self) -> usize {
        self.markers.len()
    }
}

/// `hw-checklist [driver] [--json]`: collect the `HW-verified:` markers into a per-driver burndown,
/// optionally filtered to one driver and optionally as JSON.
pub fn run(root: &Path, driver: Option<String>, json: bool) -> Result<(), String> {
    let reports = collect(root, driver.as_deref())?;
    if json {
        println!("{}", render_json(&reports));
    } else {
        println!("{}", render_markdown(&reports));
    }
    Ok(())
}

/// Scan the tree and group the markers by driver, keeping only `filter`'s driver when it is set.
fn collect(root: &Path, filter: Option<&str>) -> Result<Vec<DriverReport>, String> {
    let mut by_driver: BTreeMap<String, Vec<Marker>> = BTreeMap::new();
    for path in walk(root)? {
        if !path.extension().is_some_and(|e| e == "rs") {
            continue;
        }
        let Some(driver) = driver_of(&path) else {
            continue;
        };
        if filter.is_some_and(|f| driver.as_str() != f) {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        let rel = path.strip_prefix(root).unwrap_or(&path);
        let rel = rel.display().to_string().replace('\\', "/");
        for (i, line) in body.lines().enumerate() {
            if let Some((resolved, text)) = classify(line) {
                by_driver.entry(driver.clone()).or_default().push(Marker {
                    file: rel.clone(),
                    line: i + 1,
                    text,
                    resolved,
                });
            }
        }
    }
    // BTreeMap yields drivers in name order; sort each driver's markers by file then line so the
    // checklist reads top-to-bottom through each file.
    Ok(by_driver
        .into_iter()
        .map(|(driver, mut markers)| {
            markers.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
            DriverReport { driver, markers }
        })
        .collect())
}

/// The driver a file belongs to, or `None` when the file is not driver source.
///
/// A driver is a `usb/drivers/<name>/` subtree; its files map to `<name>`. The worked example is the
/// backend's `usb/*.rs`, whose files map to `vfs5011`. Everything else (the toolkit's live-USB seam,
/// scaffolding templates, golden fixtures) is not a driver and maps to `None`.
fn driver_of(path: &Path) -> Option<String> {
    let comps: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    for i in 0..comps.len() {
        if comps[i] == "usb" && i + 2 < comps.len() && comps[i + 1] == "drivers" {
            return Some(comps[i + 2].clone());
        }
    }
    if comps.len() >= 2 && comps[comps.len() - 2] == "usb" {
        return Some("vfs5011".to_string());
    }
    None
}

/// Classify one line: `Some((resolved, marker_text))` for a genuine marker, `None` otherwise.
///
/// A genuine marker sits inside a comment and is not a quoted mention of the convention. The word
/// after the token decides the axis: `required` is pending, `confirmed` is resolved.
fn classify(line: &str) -> Option<(bool, String)> {
    let comment = line.find("//")?;
    let m = line.find(MARKER)?;
    if m < comment {
        return None;
    }
    // A `"` or backtick right before the token quotes it — prose about the convention, not a marker.
    if line[..m]
        .chars()
        .last()
        .is_some_and(|c| c == '"' || c == '`')
    {
        return None;
    }
    let rest = line[m + MARKER.len()..].trim_start();
    let resolved = if let Some(after) = rest.strip_prefix("required") {
        if !ends_word(after) {
            return None;
        }
        false
    } else {
        let after = rest.strip_prefix("confirmed")?;
        if !ends_word(after) {
            return None;
        }
        true
    };
    Some((resolved, line[m..].trim().to_string()))
}

/// Whether what follows the marker word ends it: not a letter or digit that would make it a longer
/// word, and not a quote or backtick that would mark a quoted mention.
fn ends_word(after: &str) -> bool {
    match after.chars().next() {
        None => true,
        Some(c) => !c.is_alphanumeric() && c != '"' && c != '`',
    }
}

/// A Markdown checklist grouped by driver: a header line of counts, then each pending marker.
fn render_markdown(reports: &[DriverReport]) -> String {
    let mut out = String::from("# HW-verified bring-up checklist\n");
    if reports.is_empty() {
        out.push_str("\nNo HW-verified markers found.\n");
    }
    let (mut tp, mut tr) = (0usize, 0usize);
    for r in reports {
        let (p, res, tot) = (r.pending(), r.resolved(), r.total());
        tp += p;
        tr += res;
        out.push_str(&format!(
            "\n## {} — {p} pending, {res} resolved ({tot} total)\n\n",
            r.driver
        ));
        if p == 0 {
            out.push_str("- (none pending)\n");
        }
        for m in r.markers.iter().filter(|m| !m.resolved) {
            out.push_str(&format!("- [ ] {}:{}  {}\n", m.file, m.line, m.text));
        }
        for m in r.markers.iter().filter(|m| m.resolved) {
            out.push_str(&format!("- [x] {}:{}  {}\n", m.file, m.line, m.text));
        }
    }
    out.push_str(&format!(
        "\n## Totals\n\n{tp} pending, {tr} resolved across {} driver(s)\n",
        reports.len()
    ));
    out
}

/// The same burndown as machine-readable JSON.
fn render_json(reports: &[DriverReport]) -> String {
    let mut s = String::from("{\n  \"drivers\": [");
    for (di, r) in reports.iter().enumerate() {
        if di > 0 {
            s.push(',');
        }
        s.push_str("\n    {\n");
        s.push_str(&format!("      \"driver\": \"{}\",\n", esc(&r.driver)));
        s.push_str(&format!("      \"pending\": {},\n", r.pending()));
        s.push_str(&format!("      \"resolved\": {},\n", r.resolved()));
        s.push_str(&format!("      \"total\": {},\n", r.total()));
        s.push_str("      \"pending_markers\": [");
        let pending: Vec<&Marker> = r.markers.iter().filter(|m| !m.resolved).collect();
        for (mi, m) in pending.iter().enumerate() {
            if mi > 0 {
                s.push(',');
            }
            s.push_str(&format!(
                "\n        {{ \"file\": \"{}\", \"line\": {}, \"text\": \"{}\" }}",
                esc(&m.file),
                m.line,
                esc(&m.text)
            ));
        }
        if !pending.is_empty() {
            s.push_str("\n      ");
        }
        s.push_str("]\n    }");
    }
    if !reports.is_empty() {
        s.push_str("\n  ");
    }
    s.push_str("],\n");
    let (tp, tr, tt) = reports
        .iter()
        .fold((0usize, 0usize, 0usize), |(p, r, t), d| {
            (p + d.pending(), r + d.resolved(), t + d.total())
        });
    s.push_str(&format!(
        "  \"totals\": {{ \"drivers\": {}, \"pending\": {tp}, \"resolved\": {tr}, \"total\": {tt} }}\n}}",
        reports.len()
    ));
    s
}

/// Escape a string for a JSON double-quoted value.
fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

/// Every file under `root`, skipping [`SKIP_DIRS`].
fn walk(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries =
            std::fs::read_dir(&dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                let name = entry.file_name();
                if !SKIP_DIRS.contains(&name.to_string_lossy().as_ref()) {
                    stack.push(path);
                }
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A fresh temp directory, unique per call within a process run.
    fn temp_dir() -> PathBuf {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("hw-checklist-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    /// A self-contained tree: a driver subtree, the worked example, quoted mentions to ignore, and a
    /// non-driver file to ignore.
    fn fixture() -> PathBuf {
        let root = temp_dir();
        write(
            &root,
            "usb/vfs5011.rs",
            "//! marked \"HW-verified: required\" is a mention, not a marker.\n\
             /// HW-verified: required. Placeholder vendor id.\n\
             /// accept the whole family. HW-verified: required to confirm the set.\n\
             /// see `HW-verified: confirmed` for the resolved shape.\n\
             /// HW-verified: required. Placeholder endpoint.\n",
        );
        write(
            &root,
            "usb/drivers/acme/acme.rs",
            "/// HW-verified: required. Placeholder vendor id.\n\
             /// HW-verified: confirmed descriptor dump 2026-07-16.\n\
             /// HW-verified: required. Placeholder endpoint.\n",
        );
        write(
            &root,
            "usb/drivers/acme/proto.rs",
            "/// HW-verified: required. Placeholder framing.\n",
        );
        // Outside any usb/ subtree: the live-USB seam, not a driver.
        write(
            &root,
            "src/record.rs",
            "// HW-verified: required. Reaches a physical sensor.\n",
        );
        root
    }

    #[test]
    fn groups_by_driver_and_classifies() {
        let root = fixture();
        let reports = collect(&root, None).unwrap();

        // Two drivers: the worked example and the subtree. The seam file is not a driver.
        let names: Vec<&str> = reports.iter().map(|r| r.driver.as_str()).collect();
        assert_eq!(names, ["acme", "vfs5011"]);

        let acme = &reports[0];
        assert_eq!(acme.pending(), 3);
        assert_eq!(acme.resolved(), 1);
        assert_eq!(acme.total(), 4);

        let vfs = &reports[1];
        // Two genuine `required` markers; the quoted and backticked lines are mentions.
        assert_eq!(vfs.pending(), 3);
        assert_eq!(vfs.resolved(), 0);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn driver_filter_narrows_to_one_subtree() {
        let root = fixture();
        let reports = collect(&root, Some("acme")).unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].driver, "acme");
        assert_eq!(reports[0].total(), 4);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn json_carries_the_documented_shape() {
        let root = fixture();
        let reports = collect(&root, None).unwrap();
        let json = render_json(&reports);
        assert!(json.contains("\"drivers\": ["));
        assert!(json.contains("\"driver\": \"acme\""));
        assert!(json.contains("\"pending_markers\": ["));
        assert!(json.contains(
            "\"totals\": { \"drivers\": 2, \"pending\": 6, \"resolved\": 1, \"total\": 7 }"
        ));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn worked_example_has_pending_and_no_confirmed_regressions() {
        // The real tree: the vfs5011 worked example must report a nonzero pending count and no
        // `confirmed` marker (none has been checked against hardware).
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let reports = collect(repo, Some("vfs5011")).unwrap();
        let vfs = reports
            .iter()
            .find(|r| r.driver == "vfs5011")
            .expect("the worked example reports markers");
        assert!(
            vfs.pending() > 0,
            "the worked example still has pending markers"
        );
        assert_eq!(
            vfs.resolved(),
            0,
            "no worked-example marker is confirmed yet"
        );
    }
}
