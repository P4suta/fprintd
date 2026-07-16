// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The driver acceptance gate: the checks `docs/adding-a-driver.md` asks of a native driver, run as
//! one command.
//!
//! The acceptance criteria are prose a human runs by hand. This mechanizes them into one green/red
//! scorecard: the `unsafe` quarantine, the black-box verification, REUSE cleanliness, the workspace
//! lints, the dependency boundary, and — for a driver that lives in its own crate — the registry's
//! publish rules. It answers "am I PR-ready?" before a contributor opens a bring-up.
//!
//! It **composes**, it does not re-derive. The `unsafe` quarantine and the dependency boundary are
//! read straight from [`crate::deps::check`], the graph oracle CI runs; the tests, REUSE, lints and
//! publish rules are the very commands CI and `mise` run, invoked as subprocesses. The gate cannot
//! disagree with CI because it is the same checks. It closes by asking `hw-checklist` what device
//! values still await a physical sensor — the remaining work no gate can pass on faith.

use std::path::Path;
use std::process::Command;

use cargo_metadata::MetadataCommand;

/// `driver-check [driver]`: run the acceptance checks, optionally scoped to one driver.
///
/// # Errors
/// Returns an error (a non-zero exit) when any hard gate is red.
pub fn run(root: &Path, driver: Option<String>) -> Result<(), String> {
    // `heavy` runs the real subprocess gates (clippy, tests, REUSE, publish); a unit test passes
    // `false` to exercise the composition without paying for minutes of cargo.
    let card = evaluate(root, driver.as_deref(), true);
    print!("{}", card.render());
    card.gate()
}

/// One acceptance criterion and how it came out.
struct Check {
    name: &'static str,
    status: Status,
    /// A single line: why it passed, or what to run to see why it failed.
    detail: String,
}

impl Check {
    fn pass(name: &'static str, detail: String) -> Self {
        Self {
            name,
            status: Status::Pass,
            detail,
        }
    }
    fn fail(name: &'static str, detail: String) -> Self {
        Self {
            name,
            status: Status::Fail,
            detail,
        }
    }
    fn skip(name: &'static str) -> Self {
        Self {
            name,
            status: Status::Skip,
            detail: "skipped in fast mode".to_owned(),
        }
    }
    fn na(name: &'static str, detail: String) -> Self {
        Self {
            name,
            status: Status::Na,
            detail,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Status {
    Pass,
    Fail,
    Skip,
    Na,
}

impl Status {
    fn label(&self) -> &'static str {
        match self {
            Status::Pass => "ok",
            Status::Fail => "FAIL",
            Status::Skip => "skip",
            Status::Na => "n/a",
        }
    }
}

/// The filled-in scorecard.
struct Scorecard {
    driver: Option<String>,
    checks: Vec<Check>,
    hw: HwSummary,
}

impl Scorecard {
    fn reds(&self) -> usize {
        self.checks
            .iter()
            .filter(|c| c.status == Status::Fail)
            .count()
    }

    /// A non-zero exit when any hard gate is red. `hw-checklist` is advisory and never gates: a
    /// bring-up is *expected* to have pending markers.
    fn gate(&self) -> Result<(), String> {
        match self.reds() {
            0 => Ok(()),
            n => Err(format!(
                "{n} acceptance gate(s) red — this driver is not PR-ready yet"
            )),
        }
    }

    fn render(&self) -> String {
        let scope = match &self.driver {
            Some(d) => format!("driver `{d}`"),
            None => "whole native backend".to_owned(),
        };
        let mut s = format!("driver-check — acceptance scorecard ({scope})\n\n");
        for c in &self.checks {
            s.push_str(&format!(
                "  {:<4}  {:<15}  {}\n",
                c.status.label(),
                c.name,
                c.detail
            ));
        }
        s.push_str(&format!("\n  hw-checklist       {}\n\n", self.hw.line()));
        match self.reds() {
            0 => s.push_str("  PR-ready: every acceptance gate is green.\n"),
            n => s.push_str(&format!("  NOT PR-ready: {n} gate(s) red — see above.\n")),
        }
        s
    }
}

/// Build every check, in the order `docs/adding-a-driver.md` lists them.
fn evaluate(root: &Path, driver: Option<&str>, heavy: bool) -> Scorecard {
    let (unsafe_check, boundary_check) = deps_checks(root);

    let checks = vec![
        unsafe_check,
        black_box_check(root, driver, heavy),
        reuse_check(root, heavy),
        lints_check(root, heavy),
        boundary_check,
        publish_check(root, driver, heavy),
    ];

    let hw = hw_summary(root, driver);
    Scorecard {
        driver: driver.map(str::to_owned),
        checks,
        hw,
    }
}

/// Criteria 1 and 5, both read from the one graph oracle CI runs ([`crate::deps::check`]) so this
/// cannot diverge from it. R4 is the `unsafe` quarantine; every other finding is a boundary break.
fn deps_checks(root: &Path) -> (Check, Check) {
    let mut findings: Vec<crate::lint::Finding> = Vec::new();
    if let Err(e) = crate::deps::check(root, &mut findings) {
        return (
            Check::fail("forbid-unsafe", format!("deps check could not run: {e}")),
            Check::fail("dep-boundary", "deps check could not run".to_owned()),
        );
    }

    let is_unsafe = |f: &&crate::lint::Finding| f.rule.contains("forbids unsafe");
    let unsafe_hits: Vec<_> = findings.iter().filter(is_unsafe).collect();
    let boundary_hits: Vec<_> = findings.iter().filter(|f| !is_unsafe(f)).collect();

    let unsafe_check = if unsafe_hits.is_empty() {
        Check::pass(
            "forbid-unsafe",
            "unsafe stays quarantined to the transport leaf (deps R4)".to_owned(),
        )
    } else {
        Check::fail(
            "forbid-unsafe",
            format!(
                "{} crate(s) miss #![forbid(unsafe_code)]: {}",
                unsafe_hits.len(),
                relative_files(root, &unsafe_hits),
            ),
        )
    };

    let boundary_check = if boundary_hits.is_empty() {
        Check::pass(
            "dep-boundary",
            "dependencies flow only toward the leaves (deps.rs)".to_owned(),
        )
    } else {
        Check::fail(
            "dep-boundary",
            format!(
                "{} boundary violation(s); run `cargo xtask lint` for the detail",
                boundary_hits.len(),
            ),
        )
    };

    (unsafe_check, boundary_check)
}

/// Criterion 3: the driver's black-box tests pass — the mock/replay/captured-frame suite that
/// `sources/` and `usb/mock_tests.rs` are. Scoped to the driver name when one is given.
fn black_box_check(root: &Path, driver: Option<&str>, heavy: bool) -> Check {
    if !heavy {
        return Check::skip("black-box");
    }
    match cargo_test(root, driver) {
        Ok(n) => Check::pass(
            "black-box",
            format!(
                "cargo test -p fprint-backend-native{}: {n} passed",
                filter_suffix(driver)
            ),
        ),
        Err(e) => Check::fail("black-box", e),
    }
}

fn cargo_test(root: &Path, driver: Option<&str>) -> Result<usize, String> {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(root)
        .args(["test", "-p", "fprint-backend-native"]);
    if let Some(d) = driver {
        cmd.arg(d);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("could not run cargo test: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "tests failed; run `cargo test -p fprint-backend-native{}`",
            filter_suffix(driver),
        ));
    }
    Ok(count_passed(&String::from_utf8_lossy(&out.stdout)))
}

/// Sum the `passed` counts across every `test result: ok.` line — one per test binary.
fn count_passed(stdout: &str) -> usize {
    stdout
        .lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix("test result: ok. ")?;
            rest.split(' ').next()?.parse::<usize>().ok()
        })
        .sum()
}

/// Criterion 4a: REUSE cleanliness, run with the exact tool `mise run reuse` uses.
fn reuse_check(root: &Path, heavy: bool) -> Check {
    if !heavy {
        return Check::skip("reuse");
    }
    let out = Command::new("uvx")
        .current_dir(root)
        .args(["--with", "charset-normalizer", "reuse", "lint"])
        .output();
    match out {
        Err(e) => Check::fail("reuse", format!("could not run `uvx reuse lint`: {e}")),
        Ok(o) if o.status.success() => Check::pass(
            "reuse",
            "every file declares its licence (uvx reuse lint)".to_owned(),
        ),
        Ok(_) => Check::fail(
            "reuse",
            "undeclared file(s); run `mise run reuse` for the list".to_owned(),
        ),
    }
}

/// Criterion 4b: the workspace lints, the exact commands the `check` CI job gates on — clippy with
/// `--all-features`, so the `usb` transport leaf a driver adds is linted, and `fmt --check`.
fn lints_check(root: &Path, heavy: bool) -> Check {
    if !heavy {
        return Check::skip("lints");
    }
    if let Err(e) = run_ok(root, "cargo", &["fmt", "--all", "--check"]) {
        return Check::fail("lints", format!("fmt --check: {e}"));
    }
    if let Err(e) = run_ok(
        root,
        "cargo",
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ],
    ) {
        return Check::fail("lints", format!("clippy: {e}"));
    }
    Check::pass(
        "lints",
        "clippy -D warnings and fmt --check are clean".to_owned(),
    )
}

/// Criterion 6: a driver that lives in its own crate must pass the registry's rules. An in-tree
/// driver (part of the `publish = false` backend) has no crate to publish, so this is n/a.
fn publish_check(root: &Path, driver: Option<&str>, heavy: bool) -> Check {
    let Some(name) = isolated_crate(root, driver) else {
        let reason = match driver {
            Some(d) => format!("`{d}` is an in-tree driver — no isolated crate to publish"),
            None => "no isolated-crate driver named".to_owned(),
        };
        return Check::na("publish-parity", reason);
    };
    if !heavy {
        return Check::skip("publish-parity");
    }
    match run_ok(
        root,
        "cargo",
        &["run", "--quiet", "-p", "xtask", "--", "publish-check"],
    ) {
        Ok(()) => Check::pass(
            "publish-parity",
            format!("`{name}` packages under the registry's rules"),
        ),
        Err(e) => Check::fail("publish-parity", format!("publish-check: {e}")),
    }
}

/// The driver name, if it names a workspace member crate — i.e. an `--isolated-crate` driver rather
/// than an in-tree one. The resolver is the authority on membership.
fn isolated_crate(root: &Path, driver: Option<&str>) -> Option<String> {
    let name = driver?;
    let metadata = MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .no_deps()
        .exec()
        .ok()?;
    metadata
        .workspace_packages()
        .iter()
        .any(|p| p.name == name)
        .then(|| name.to_owned())
}

/// The remaining bring-up work, from `hw-checklist` itself: shell out to the real subcommand and
/// read its JSON, so the count is whatever that oracle reports, not a second implementation.
fn hw_summary(root: &Path, driver: Option<&str>) -> HwSummary {
    let mut cmd = Command::new("cargo");
    cmd.current_dir(root)
        .args(["run", "--quiet", "-p", "xtask", "--", "hw-checklist"]);
    if let Some(d) = driver {
        cmd.arg(d);
    }
    cmd.arg("--json");
    match cmd.output() {
        Err(e) => HwSummary::Unavailable(format!("could not run hw-checklist: {e}")),
        Ok(o) if !o.status.success() => HwSummary::Unavailable(format!(
            "hw-checklist exited non-zero: {}",
            String::from_utf8_lossy(&o.stderr).trim(),
        )),
        Ok(o) => HwSummary::from_json(String::from_utf8_lossy(&o.stdout).trim()),
    }
}

/// What `hw-checklist --json` told us.
enum HwSummary {
    /// The number of pending `HW-verified: required` markers.
    Pending(usize),
    /// The subcommand could not be run or did not return JSON.
    Unavailable(String),
}

impl HwSummary {
    /// Read a pending count out of `hw-checklist --json`. Tolerant of the exact shape — an array of
    /// markers, or an object carrying the list or a count — because the count is all this needs.
    /// Empty output is a successful run with nothing pending.
    fn from_json(stdout: &str) -> Self {
        if stdout.is_empty() {
            return HwSummary::Pending(0);
        }
        match serde_json::from_str::<serde_json::Value>(stdout) {
            Ok(v) => HwSummary::Pending(count_pending(&v)),
            Err(e) => HwSummary::Unavailable(format!("output was not valid JSON: {e}")),
        }
    }

    fn line(&self) -> String {
        match self {
            HwSummary::Pending(0) => {
                "no pending HW-verified markers reported by `cargo xtask hw-checklist`".to_owned()
            }
            HwSummary::Pending(n) => format!(
                "{n} pending HW-verified marker(s) — run `cargo xtask hw-checklist` for the list"
            ),
            HwSummary::Unavailable(why) => format!("unavailable: {why}"),
        }
    }
}

/// The pending count from `hw-checklist --json`.
///
/// Its documented shape is `{ "drivers": [ { "pending": n, .. } ], "totals": { "pending": n, .. } }`;
/// the totals field is read first, then a sum over the drivers, so a driver-scoped run and a
/// whole-tree run both give the count of markers still awaiting a sensor. The remaining arms keep the
/// reading tolerant of a bare array or a flat count.
fn count_pending(v: &serde_json::Value) -> usize {
    use serde_json::Value;
    let as_count = |x: Option<&Value>| x.and_then(Value::as_u64).map(|n| n as usize);
    match v {
        Value::Array(a) => a.len(),
        Value::Object(o) => {
            if let Some(n) = as_count(o.get("totals").and_then(|t| t.get("pending"))) {
                return n;
            }
            if let Some(Value::Array(drivers)) = o.get("drivers") {
                return drivers
                    .iter()
                    .filter_map(|d| d.get("pending").and_then(Value::as_u64))
                    .map(|n| n as usize)
                    .sum();
            }
            if let Some(Value::Array(a)) = o.get("pending_markers") {
                return a.len();
            }
            as_count(o.get("pending")).unwrap_or(0)
        }
        _ => 0,
    }
}

/// Run a command, mapping a non-zero exit or a spawn failure into a short reason.
fn run_ok(root: &Path, program: &str, args: &[&str]) -> Result<(), String> {
    let out = Command::new(program)
        .current_dir(root)
        .args(args)
        .output()
        .map_err(|e| format!("could not run `{program}`: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("`{program} {}` failed", args.join(" ")))
    }
}

/// The ` <driver>` suffix a scoped cargo filter carries, or nothing.
fn filter_suffix(driver: Option<&str>) -> String {
    driver.map(|d| format!(" {d}")).unwrap_or_default()
}

/// The findings' files, workspace-relative and comma-joined, for a one-line reason.
fn relative_files(root: &Path, findings: &[&crate::lint::Finding]) -> String {
    findings
        .iter()
        .map(|f| {
            f.file
                .strip_prefix(root)
                .unwrap_or(&f.file)
                .display()
                .to_string()
                .replace('\\', "/")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask/ always has a parent")
            .to_path_buf()
    }

    fn find<'a>(card: &'a Scorecard, name: &str) -> &'a Check {
        card.checks
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("no `{name}` check on the scorecard"))
    }

    #[test]
    fn scorecard_runs_and_is_green_on_the_current_tree() {
        // Fast mode: the in-process graph gates and the hw-checklist shell-out run for real; the
        // minutes-long cargo gates (tests, clippy, REUSE, publish) are skipped. The real command
        // (`Mode::Full`) runs them — this only keeps the unit test quick.
        let root = repo_root();
        let card = evaluate(&root, None, false);

        assert_eq!(
            find(&card, "forbid-unsafe").status,
            Status::Pass,
            "{}",
            find(&card, "forbid-unsafe").detail,
        );
        assert_eq!(
            find(&card, "dep-boundary").status,
            Status::Pass,
            "{}",
            find(&card, "dep-boundary").detail,
        );

        // No gate is red on the green tree.
        let rendered = card.render();
        assert_eq!(card.reds(), 0, "a gate is red:\n{rendered}");
        assert!(card.gate().is_ok());

        // The hw-checklist section is present and carries a status line.
        assert!(
            rendered.contains("hw-checklist"),
            "no hw-checklist section:\n{rendered}"
        );
        assert!(
            rendered.contains("HW-verified marker")
                || rendered.contains("no pending HW-verified markers")
                || rendered.contains("unavailable"),
            "hw-checklist section is not populated:\n{rendered}",
        );
    }

    #[test]
    fn hw_json_yields_a_pending_count_across_shapes() {
        // The shape `hw-checklist --json` documents: totals carry the count.
        let documented = r#"{
          "drivers": [ { "driver": "vfs5011", "pending": 15, "resolved": 0, "total": 15 } ],
          "totals": { "drivers": 1, "pending": 15, "resolved": 0, "total": 15 }
        }"#;
        assert!(matches!(
            HwSummary::from_json(documented),
            HwSummary::Pending(15)
        ));

        // A driver list with no totals is summed.
        let no_totals = r#"{ "drivers": [ { "pending": 2 }, { "pending": 3 } ] }"#;
        assert!(matches!(
            HwSummary::from_json(no_totals),
            HwSummary::Pending(5)
        ));

        // Empty output is a clean run with nothing pending; a bare array counts its entries.
        assert!(matches!(HwSummary::from_json(""), HwSummary::Pending(0)));
        assert!(matches!(
            HwSummary::from_json("[{},{},{}]"),
            HwSummary::Pending(3)
        ));
        assert!(matches!(
            HwSummary::from_json("not json"),
            HwSummary::Unavailable(_)
        ));
    }

    #[test]
    fn passed_counts_sum_across_test_binaries() {
        let stdout = "\
running 3 tests
test result: ok. 3 passed; 0 failed; 0 ignored
running 5 tests
test result: ok. 5 passed; 0 failed; 0 ignored
";
        assert_eq!(count_passed(stdout), 8);
    }
}
