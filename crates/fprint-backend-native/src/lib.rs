// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! # fprint-backend-native
//!
//! A pure-Rust implementation of fprint-core's [`Backend`](fprint_core::Backend) /
//! [`Device`](fprint_core::Device) traits: an in-memory, deterministic **virtual** fingerprint
//! device with no USB, no async runtime and no biometrics, so the daemon and higher layers have
//! something to run against offline, on any platform.
//!
//! ```
//! use fprint_backend_native::{VirtualBackend, VirtualDeviceBuilder, Scenario, EnrollScript, FingerId};
//! use fprint_core::{Backend, Device, Print, Finger};
//!
//! # async fn demo() -> fprint_core::Result<()> {
//! let backend = VirtualBackend::single(
//!     VirtualDeviceBuilder::host_image_sensor()
//!         .scenario(Scenario::new().enroll(EnrollScript::default().produces(FingerId(7)))),
//! );
//! let mut dev = backend.enumerate().await?.pop().unwrap();
//! dev.open().await?;
//! let print = dev.enroll(Print::new_for_enroll(Finger::LeftIndex), &mut |_p| {}).await?;
//! assert_eq!(print.finger, Some(Finger::LeftIndex));
//! # Ok(())
//! # }
//! ```
//!
//! ## What is faithfully modelled
//!
//! Open/close state, per-feature capability gating, host vs. match-on-chip archetypes,
//! multi-stage enrollment with retries (each carrying a [`fprint_core::RetryReason`]),
//! duplicate/full storage errors, list/delete/clear, suspend/resume, and **drop-cancellation**:
//! `enroll` spans several polls and commits nothing to storage until its final poll, so
//! dropping the future cancels cleanly.

#![forbid(unsafe_code)]

mod backend;
mod builder;
mod device;
pub mod device_db;
mod frame;
mod frame_source;
mod image_backend;
mod image_device;
mod scenario;
mod sources;
mod store;
mod synth;
mod usb;
mod yield_now;

pub use backend::VirtualBackend;
pub use builder::{DeviceShape, VirtualDeviceBuilder};
pub use device::VirtualDevice;
pub use device_db::{lookup as lookup_device, DeviceRecord, Family};
pub use frame::Frame;
pub use frame_source::{Capture, FrameSource};
pub use image_backend::ImageBackend;
pub use image_device::ImageDevice;
pub use scenario::{CaptureOutcome, EnrollScript, FingerId, Scenario};
pub use sources::{frame_to_pgm, FileFrameSource, SyntheticFrameSource};
pub use usb::{
    assemble_frame, encode_frame_header, parse_frame_header, ScriptedTransport, Session,
    UsbFrameSource, UsbId, UsbTransfer, UsbTransport, FRAME_HEADER_LEN, FRAME_MAGIC,
};
/// The real `nusb`-backed transport and live bus enumeration are public only when the `usb`
/// feature is on.
#[cfg(feature = "usb")]
pub use usb::{list_usb_devices, NusbTransport, UsbDeviceInfo};

/// The types a caller reaches for first: build a device, script it, drive it.
///
/// A glob-friendly re-export of the crate's entry points, so `use
/// fprint_backend_native::prelude::*;` brings in what the top-level example uses — the
/// [`VirtualBackend`] itself, the [`VirtualDeviceBuilder`] that shapes a device, and the
/// [`Scenario`]/[`EnrollScript`]/[`FingerId`] trio that scripts its captures. The `fprint-core`
/// traits ([`Backend`](fprint_core::Backend)/[`Device`](fprint_core::Device)) whose methods you then
/// call are imported from `fprint_core` directly.
pub mod prelude {
    pub use crate::{EnrollScript, FingerId, Scenario, VirtualBackend, VirtualDeviceBuilder};
}
