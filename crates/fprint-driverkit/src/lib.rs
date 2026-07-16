// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! # fprint-driverkit
//!
//! The library behind `fpdev`, the workbench for writing a native driver for a sensor the stack
//! does not yet support. Each bring-up phase is a module here; the binary is a thin clap shell over
//! them. This is the *identify* phase's home: probe a device and report what it is before a line of
//! driver code is written.

#![forbid(unsafe_code)]

pub mod capture;
pub mod cassette;
pub mod diag;
pub mod doctor;
pub mod frame;
pub mod matchcmd;
pub mod newdriver;
pub mod probe;
pub mod record;
pub mod replay;
pub mod shell;
pub mod ship;
