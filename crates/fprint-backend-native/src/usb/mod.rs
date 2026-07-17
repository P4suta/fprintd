// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The real-USB scaffold: a second [`crate::FrameSource`] implementor behind the same seam the
//! synthetic and file sources use.
//!
//! **Status: experimental — not a project goal.** Reimplementing hardware drivers in Rust is an
//! open invitation, never a yardstick for this project (see `ARCHITECTURE.md` §Non-goals and
//! `docs/adding-a-driver.md`). This module does not capture from a real sensor: the Validity
//! VFS5011 protocol values (VID/PID, endpoints, frame geometry, init/deinit byte sequences) are
//! placeholders marked "HW-verified: required", nothing discovers a sensor and opens it into a
//! running device, and the `nusb`-backed transport has done no real I/O. It is a worked example,
//! not a working driver.
//!
//! The layering keeps the platform-independent protocol out of the platform-dependent transport:
//!
//! - `proto` is pure `Vec<u8>` framing / encode / parse — no transport, no `nusb`, unit-tested on
//!   any platform (Windows included). It is compiled **unconditionally**.
//! - [`transport::UsbTransport`] is the tiny async bulk/control seam the driver drives; the real
//!   [`nusb`]-backed implementor (`NusbTransport`) is the *only* thing behind `#[cfg(feature =
//!   "usb")]`, so a default build pulls no USB stack at all.
//! - [`source::UsbFrameSource`] is a [`crate::FrameSource`] generic over any `UsbTransport`, so
//!   `ImageDevice<UsbFrameSource<NusbTransport>>` runs the genuine host-image pipeline over real
//!   hardware, and `ImageDevice<UsbFrameSource<ScriptedTransport>>` exercises the same code path
//!   with scripted bytes and no hardware.
//! - `wire` is the transport-agnostic record of USB traffic ([`wire::UsbTransfer`] / [`wire::Session`]);
//!   `scripted` ([`scripted::ScriptedTransport`]) replays a recording through the trait so a capture
//!   round-trips offline, and `vfs5011`'s init/deinit sequences are expressed in that same vocabulary.
//! - `vfs5011` holds the Validity VFS5011 device constants and init/deinit sequences, written as
//!   original code from interoperability facts (see that module's provenance note).
//!
//! **Hardware verification.** Everything except the real I/O in `NusbTransport` is exercised
//! offline; the actual bytes a Validity sensor expects, and the `nusb` calls that deliver them, can
//! only be confirmed against physical hardware. Such spots are marked "HW-verified: required".

pub mod proto;
pub mod scripted;
mod source;
mod transport;
mod vfs5011;
pub mod wire;

#[cfg(test)]
mod mock_tests;

pub use proto::{
    assemble_frame, encode_frame_header, parse_frame_header, FRAME_HEADER_LEN, FRAME_MAGIC,
};
pub use scripted::ScriptedTransport;
pub use source::UsbFrameSource;
pub use transport::UsbTransport;
pub use wire::{Session, UsbId, UsbTransfer};

// The real transport and live bus enumeration are the feature-gated surface; the rest of the module
// is always built.
#[cfg(feature = "usb")]
pub use transport::{list_usb_devices, NusbTransport, UsbDeviceInfo};
