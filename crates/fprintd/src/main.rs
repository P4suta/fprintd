// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `fprintd` binary entry point.
//!
//! The daemon is Linux-only (system D-Bus, PolicyKit, logind, the libfprint shim), so all
//! real logic lives in the `#![cfg(target_os = "linux")]` library crate ([`fprintd`]) and
//! this thin `main` merely dispatches to it — keeping the binary buildable on any platform.

#[cfg(target_os = "linux")]
fn main() {
    fprintd::run();
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("fprintd is Linux-only.");
    std::process::exit(1);
}
