// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Shared scaffolding for the D-Bus integration tests.

#![cfg(target_os = "linux")]

/// A private session bus, spawned for the duration of a test so it is self-contained (no
/// ambient `DBUS_SESSION_BUS_ADDRESS` or `dbus-run-session` wrapper required). The daemon and
/// client both connect to it; it is torn down when the guard drops.
///
/// One per test binary: `start` writes a process-global env var, so two concurrent buses in one
/// process would race for it. Cargo gives each `tests/*.rs` its own process, which is why a
/// second D-Bus test lives in its own file.
pub struct PrivateBus {
    child: std::process::Child,
}

impl PrivateBus {
    pub fn start() -> Self {
        use std::io::BufRead;
        let mut child = std::process::Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn dbus-daemon (install the `dbus` package)");
        let stdout = child.stdout.take().expect("dbus-daemon stdout");
        let mut address = String::new();
        std::io::BufReader::new(stdout)
            .read_line(&mut address)
            .expect("read bus address");
        // Safe under the one-per-binary rule above: no other test in this process touches it.
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", address.trim());
        PrivateBus { child }
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
