// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [`VirtualBackend`]: the [`fp_core::Backend`] entry point over a set of virtual devices.
//!
//! It is the virtual analogue of libfprint's `FpContext`: a fixed list of device
//! *descriptions* ([`crate::VirtualDeviceBuilder`]) that it enumerates or opens by id. Each
//! call builds a fresh, closed device, so enumerating twice yields independent devices.

use fp_core::{Backend, DeviceId, Error, Result};

use crate::builder::VirtualDeviceBuilder;
use crate::device::VirtualDevice;

/// A backend serving a fixed roster of virtual devices.
pub struct VirtualBackend {
    builders: Vec<VirtualDeviceBuilder>,
}

impl VirtualBackend {
    /// A backend over several device descriptions.
    pub fn new(builders: Vec<VirtualDeviceBuilder>) -> Self {
        VirtualBackend { builders }
    }

    /// Convenience: a backend serving exactly one device.
    pub fn single(builder: VirtualDeviceBuilder) -> Self {
        VirtualBackend {
            builders: vec![builder],
        }
    }
}

impl Backend for VirtualBackend {
    type Device = VirtualDevice;

    async fn enumerate(&self) -> Result<Vec<VirtualDevice>> {
        Ok(self.builders.iter().map(|b| b.build()).collect())
    }

    async fn open(&self, id: &DeviceId) -> Result<VirtualDevice> {
        self.builders
            .iter()
            .find(|b| &b.effective_id() == id)
            .map(|b| b.build())
            .ok_or(Error::NotFound)
    }
}
