// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! # fp-backend-native
//!
//! The first implementor of fp-core's [`Backend`](fp_core::Backend) /
//! [`Device`](fp_core::Device) traits: a pure-Rust, in-memory, deterministic **virtual**
//! fingerprint device. It has no USB, no async runtime, no biometrics — it exists to prove
//! that fp-core's native `async fn`-in-trait seam is implementable and pleasant, and to give
//! the daemon and higher layers something to run against offline, on any platform.
//!
//! ```
//! use fp_backend_native::{VirtualBackend, VirtualDeviceBuilder, Scenario, EnrollScript, FingerId};
//! use fp_core::{Backend, Device, Print, Finger};
//!
//! # async fn demo() -> fp_core::Result<()> {
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
//! multi-stage enrollment with retries (each carrying a [`fp_core::RetryReason`]),
//! duplicate/full storage errors, list/delete/clear, suspend/resume, and — crucially —
//! **drop-cancellation**: `enroll` spans several polls and commits nothing to storage until
//! its final poll, so dropping the future cancels cleanly.

#![forbid(unsafe_code)]

mod backend;
mod builder;
mod device;
mod scenario;
mod store;
mod synth;
mod yield_now;

pub use backend::VirtualBackend;
pub use builder::VirtualDeviceBuilder;
pub use device::VirtualDevice;
pub use scenario::{CaptureOutcome, EnrollScript, FingerId, Scenario};
