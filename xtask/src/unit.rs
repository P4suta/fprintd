// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Checking the systemd unit by giving it to systemd.

use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

/// Where the daemon's binary is assumed to live when verifying the unit. Only the substitution
/// has to be real; nothing is executed.
const LIBEXECDIR: &str = "/usr/libexec";
/// A stock distro image: the project's own dev image has no systemd, and this task needs nothing
/// else from it.
const UNIT_VERIFY_IMAGE: &str = "debian:bookworm";

/// The first `@PLACEHOLDER@` left in `s`, if any.
///
/// Matches the autotools convention the template follows — `@`, then upper-case, digits or
/// underscores, then `@`. Deliberately not "contains an `@`": systemd's own syntax is full of
/// them (`SystemCallFilter=@system-service`), and a check that cries wolf gets deleted.
fn unsubstituted_placeholder(s: &str) -> Option<String> {
    let mut from = 0;
    while let Some(open) = s[from..].find('@').map(|i| i + from) {
        let rest = &s[open + 1..];
        if let Some(close) = rest.find('@') {
            let name = &rest[..close];
            let is_placeholder = !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
            if is_placeholder {
                return Some(format!("@{name}@"));
            }
        }
        from = open + 1;
    }
    None
}

/// Check the systemd unit, in the only way that actually proves anything: by giving it to systemd.
///
/// Two claims are otherwise unfalsifiable, and one of them has already been wrong once — the unit
/// shipped without an `[Install]` section for a while, which meant it could not be `systemctl
/// enable`d at all.
///
/// 1. `systemd-analyze verify` — the unit is well-formed.
/// 2. `systemctl enable fprintd-rs` creates `/etc/systemd/system/fprintd.service` pointing at it.
///    That symlink *is* the coexistence design (ARCHITECTURE.md §Coexistence): it outranks
///    upstream's unit in `/usr/lib`, so D-Bus activation reaches us, and `disable` hands the seat
///    back.
pub fn verify(root: &Path) -> Result<(), String> {
    let template = root.join("crates/fprintd/dbus/fprintd-rs.service.in");

    // The `@LIBEXECDIR@` substitution a build system would do, done here instead of by sed. This
    // is also the only place that knows the template needs substituting at all, which is worth
    // it being findable.
    let unit = std::fs::read_to_string(&template)
        .map_err(|e| format!("read {}: {e}", template.display()))?
        .replace("@LIBEXECDIR@", LIBEXECDIR);
    if let Some(left) = unsubstituted_placeholder(&unit) {
        return Err(format!(
            "{} still contains {left} — this task substitutes only @LIBEXECDIR@, so a new \
             placeholder needs teaching here (and to whatever packaging grows later)",
            template.display()
        ));
    }

    let staging = std::env::temp_dir().join("fprintd-xtask-unit-verify");
    std::fs::create_dir_all(&staging).map_err(|e| format!("create {}: {e}", staging.display()))?;
    let staged = staging.join("fprintd-rs.service");
    std::fs::write(&staged, &unit).map_err(|e| format!("write {}: {e}", staged.display()))?;

    // The container half stays small on purpose: it installs systemd and reports, and every
    // judgement about what it reported is made below, in Rust. apt is silenced on both streams so
    // that anything left on stderr came from systemd and nothing else.
    //
    // `install -m644`, not `cp`: the staging directory is a bind mount, which on a Windows host
    // presents every file as executable, and systemd rightly complains about an executable unit.
    // 644 is also what packaging will use, so the check runs against the real thing.
    let script = "set -e
apt-get update -qq > /dev/null 2>&1
apt-get install -y -qq systemd > /dev/null 2>&1
mkdir -p /usr/libexec
install -m755 /dev/null /usr/libexec/fprintd-rs
install -m644 /staging/fprintd-rs.service /etc/systemd/system/fprintd-rs.service
systemd-analyze verify fprintd-rs.service
systemctl enable fprintd-rs > /dev/null 2>&1
readlink /etc/systemd/system/fprintd.service || echo '<no alias link>'";

    println!("xtask: verifying the unit against real systemd in {UNIT_VERIFY_IMAGE} ...");
    let output = Command::new("docker")
        .arg("run")
        .arg("--rm")
        // No MSYS_NO_PATHCONV, and no `pwd -W`: we are not in a shell, so nothing rewrites this.
        .arg("-v")
        .arg(format!("{}:/staging:ro", staging.display()))
        .arg(UNIT_VERIFY_IMAGE)
        .arg("bash")
        .arg("-c")
        .arg(script)
        .output()
        .map_err(|e| format!("spawn docker: {e} (is Docker running?)"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        let mut msg = String::new();
        let _ = write!(
            msg,
            "the unit did not verify (docker exited {})",
            output.status
        );
        if !stderr.trim().is_empty() {
            let _ = write!(msg, "\n--- stderr ---\n{}", stderr.trim());
        }
        if !stdout.trim().is_empty() {
            let _ = write!(msg, "\n--- stdout ---\n{}", stdout.trim());
        }
        return Err(msg);
    }

    // `systemd-analyze verify` is a poor citizen: it reports problems on stderr and still exits
    // 0 for some of them, so a green exit code is not on its own an answer.
    if !stderr.trim().is_empty() {
        return Err(format!(
            "systemd reported problems with the unit:\n{}",
            stderr.trim()
        ));
    }

    let alias = stdout.lines().last().unwrap_or_default().trim();
    if alias != "/etc/systemd/system/fprintd-rs.service" {
        return Err(format!(
            "`systemctl enable fprintd-rs` did not take the seat: \
             /etc/systemd/system/fprintd.service is {alias}, expected a link to our unit. \
             Check that the unit still has `[Install] Alias=fprintd.service`."
        ));
    }

    println!("xtask: unit verifies clean");
    println!("xtask: alias takes the seat: /etc/systemd/system/fprintd.service -> {alias}");
    Ok(())
}
