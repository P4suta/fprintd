// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Concrete [`crate::FrameSource`] implementations.
//!
//! The deterministic [`SyntheticFrameSource`] and the committed-corpus [`FileFrameSource`] are both
//! hardware-free capture sources that let [`crate::ImageDevice`] run the full detect → match
//! pipeline offline. The real USB transport source lives in [`crate::usb`] alongside them.

mod file;
mod synthetic;

pub use file::FileFrameSource;
pub use synthetic::SyntheticFrameSource;
