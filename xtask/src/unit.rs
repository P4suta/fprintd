// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Checking the systemd unit by giving it to systemd.

use std::path::Path;

use crate::docker::Session;

/// Where the daemon's binary is assumed to live when verifying the unit. Only the substitution
/// has to be real; nothing is executed.
const LIBEXECDIR: &str = "/usr/libexec";
/// A stock distro image: the project's own dev image has no systemd, and this task needs nothing
/// else from it.
const UNIT_VERIFY_IMAGE: &str = "debian:bookworm";
/// Where our unit is installed, and where `Alias=fprintd.service` must land to shadow upstream's.
const UNIT_PATH: &str = "/etc/systemd/system/fprintd-rs.service";
const ALIAS_PATH: &str = "/etc/systemd/system/fprintd.service";

/// The daemon flag that swaps PolicyKit for an authorizer granting everything. It exists for
/// bring-up against a virtual device with no PolicyKit daemon running, and the unit is the only
/// path by which it could reach a real system.
const TEST_MODE_FLAG: &str = "--test-mode";

/// The first `@PLACEHOLDER@` left in `s`, if any.
///
/// Matches the autotools convention the template follows — `@`, then upper-case, digits or
/// underscores, then `@`. Not "contains an `@`": systemd's own syntax is full of them
/// (`SystemCallFilter=@system-service`).
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

/// The `ExecStart=` lines a unit declares.
fn exec_starts(unit: &str) -> Vec<&str> {
    unit.lines()
        .map(str::trim)
        .filter_map(|l| l.strip_prefix("ExecStart="))
        .collect()
}

/// Check the systemd unit by giving it to systemd. Neither claim can be checked any other way:
///
/// 1. `systemd-analyze verify` — the unit is well-formed and `systemctl enable` will accept it.
/// 2. `systemctl enable fprintd-rs` creates `/etc/systemd/system/fprintd.service` pointing at it.
///    That symlink *is* the coexistence design (ARCHITECTURE.md §Coexistence): it outranks
///    upstream's unit in `/usr/lib`, so D-Bus activation reaches us, and `disable` hands the seat
///    back.
///
/// The third claim is checked here rather than by systemd, because systemd has no opinion about
/// it: the unit must not start the daemon with [`TEST_MODE_FLAG`].
pub fn verify(root: &Path) -> Result<(), String> {
    let template = root.join("crates/fprintd/dbus/fprintd-rs.service.in");

    // The `@LIBEXECDIR@` substitution packaging will do. This is the only place that knows the
    // template needs substituting at all.
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

    // PolicyKit is what stands between a local caller and someone else's enrolled fingers. The
    // flag that disables it is a bring-up convenience, and packaging is the only way it could
    // reach a real machine.
    for exec in exec_starts(&unit) {
        if exec.split_whitespace().any(|arg| arg == TEST_MODE_FLAG) {
            return Err(format!(
                "{} starts the daemon with {TEST_MODE_FLAG}, which disables every PolicyKit \
                 check:\n  ExecStart={exec}",
                template.display()
            ));
        }
    }

    let staging = std::env::temp_dir().join("fprintd-xtask-unit-verify");
    std::fs::create_dir_all(&staging).map_err(|e| format!("create {}: {e}", staging.display()))?;
    let staged = staging.join("fprintd-rs.service");
    std::fs::write(&staged, &unit).map_err(|e| format!("write {}: {e}", staged.display()))?;

    let session = Session::start(UNIT_VERIFY_IMAGE)?;
    println!("xtask: verifying the unit against real systemd in {UNIT_VERIFY_IMAGE} ...");

    // One command per step, so the systemd-analyze check below can be exact: nothing else is
    // writing to that stream.
    session.exec(&["apt-get", "update", "-qq"])?;
    session.exec(&["apt-get", "install", "-y", "-qq", "systemd"])?;
    session.exec(&["mkdir", "-p", "/usr/libexec"])?;
    session.exec(&["install", "-m755", "/dev/null", "/usr/libexec/fprintd-rs"])?;

    session.copy_in(&staged, UNIT_PATH)?;
    // `docker cp` carries the host's mode, and a Windows host calls everything executable, which
    // systemd rejects for a unit. 644 is what packaging installs.
    session.exec(&["chmod", "644", UNIT_PATH])?;

    let verify = session.exec(&["systemd-analyze", "verify", "fprintd-rs.service"])?;
    // `systemd-analyze verify` reports some problems on stderr and still exits 0, so its status
    // is not on its own an answer.
    if !verify.stderr.trim().is_empty() {
        return Err(format!(
            "systemd reported problems with the unit:\n{}",
            verify.stderr.trim()
        ));
    }
    println!("xtask: unit verifies clean");

    session.exec(&["systemctl", "enable", "fprintd-rs"])?;

    // A non-link is the answer here, not an error, so ask rather than assert.
    let (linked, link) = session.try_exec(&["readlink", ALIAS_PATH])?;
    if !linked || link.line() != UNIT_PATH {
        return Err(format!(
            "`systemctl enable fprintd-rs` did not take the seat: {ALIAS_PATH} is {}, expected a \
             link to {UNIT_PATH}. Check that the unit still has `[Install] Alias=fprintd.service`.",
            if linked { link.line() } else { "not a symlink" }
        ));
    }

    println!(
        "xtask: alias takes the seat: {ALIAS_PATH} -> {}",
        link.line()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_placeholder_is_at_at_upper_case_at() {
        assert_eq!(
            unsubstituted_placeholder("ExecStart=@LIBEXECDIR@/fprintd-rs"),
            Some("@LIBEXECDIR@".to_string())
        );
        assert_eq!(
            unsubstituted_placeholder("A@B_2@C"),
            Some("@B_2@".to_string())
        );
    }

    /// The false positives are the point: this check runs on every green build, and one that fires
    /// on a correct unit is a check on its way to being switched off.
    #[test]
    fn systemd_syntax_is_not_a_placeholder() {
        // The reason the check is not "contains an @".
        assert_eq!(
            unsubstituted_placeholder("SystemCallFilter=@system-service"),
            None
        );
        assert_eq!(unsubstituted_placeholder("@lower@"), None, "lower case");
        assert_eq!(unsubstituted_placeholder("@@"), None, "empty name");
        assert_eq!(unsubstituted_placeholder("@Mixed@"), None, "mixed case");
        assert_eq!(unsubstituted_placeholder("no ats here"), None);
    }

    /// `@A@B@` — the rescan starts one past the opening `@`, not past the whole pair, so a real
    /// placeholder is still found when a stray `@` precedes it.
    #[test]
    fn a_placeholder_is_found_after_a_stray_at() {
        assert_eq!(
            unsubstituted_placeholder("Exec=@system-service @FOO@"),
            Some("@FOO@".to_string())
        );
    }

    #[test]
    fn exec_starts_reads_every_exec_start_line_and_nothing_else() {
        let unit = "[Service]\nType=dbus\nExecStart=/usr/libexec/fprintd-rs\nExecStop=/bin/true\n";
        assert_eq!(exec_starts(unit), ["/usr/libexec/fprintd-rs"]);
        assert!(exec_starts("[Unit]\nDescription=x\n").is_empty());
    }

    /// The flag is an argument, not a substring: a path that happens to contain the text is not a
    /// finding, and one that passes the flag is.
    #[test]
    fn the_test_mode_flag_is_matched_as_a_whole_argument() {
        let names_it = |unit: &str| {
            exec_starts(unit)
                .iter()
                .any(|e| e.split_whitespace().any(|a| a == TEST_MODE_FLAG))
        };
        assert!(names_it("ExecStart=/usr/libexec/fprintd-rs --test-mode\n"));
        assert!(!names_it("ExecStart=/usr/libexec/fprintd-rs\n"));
        assert!(
            !names_it("ExecStart=/opt/--test-mode-build/fprintd-rs\n"),
            "a path containing the text is not the flag"
        );
    }
}
