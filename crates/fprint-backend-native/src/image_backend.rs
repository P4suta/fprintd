// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [`ImageBackend`]: the [`fprint_core::Backend`] entry point over a single host-image device.
//!
//! The host-image analogue of [`crate::VirtualBackend`]: it holds one device *description* — the
//! [`DeviceInfo`], a cloneable [`FrameSource`], and the match threshold — and stamps out a fresh,
//! closed [`ImageDevice`] on each `enumerate`/`open`, so enumerating twice yields independent
//! devices (the source is `Clone`d per device).

use fprint_core::{Backend, DeviceId, DeviceInfo, Error, Result};

use crate::frame_source::FrameSource;
use crate::image_device::ImageDevice;

/// A backend serving one host-image device built from a cloneable [`FrameSource`].
pub struct ImageBackend<S> {
    info: DeviceInfo,
    source: S,
    threshold: u32,
}

impl<S: FrameSource + Clone> ImageBackend<S> {
    /// Describe a single host-image device: its metadata, capture source, and match threshold.
    pub fn new(info: DeviceInfo, source: S, threshold: u32) -> Self {
        ImageBackend {
            info,
            source,
            threshold,
        }
    }

    /// Mint a fresh, closed device from this description (the source is cloned per device).
    fn build(&self) -> ImageDevice<S> {
        ImageDevice::new(self.info.clone(), self.source.clone(), self.threshold)
    }
}

impl<S: FrameSource + Clone> Backend for ImageBackend<S> {
    type Device = ImageDevice<S>;

    async fn enumerate(&self) -> Result<Vec<ImageDevice<S>>> {
        Ok(vec![self.build()])
    }

    async fn open(&self, id: &DeviceId) -> Result<ImageDevice<S>> {
        if &self.info.id == id {
            Ok(self.build())
        } else {
            Err(Error::NotFound)
        }
    }
}
