// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [`LibfprintBackend`]: an `fprint-core` [`Backend`] over a libfprint `FpContext`.
//!
//! The context is the discovery root (libfprint's `FpContext`). Like the `FpContext` it wraps,
//! this backend is `!Send`.

use fprint_core::{Backend, DeviceId, Error, Result};

use crate::convert;
use crate::device::LibfprintDevice;
use crate::ffi::FpContext;

/// Entry point to the C-libfprint-backed devices on this system.
pub struct LibfprintBackend {
    ctx: FpContext,
}

impl LibfprintBackend {
    /// Create a backend with a fresh libfprint `FpContext`.
    pub fn new() -> Self {
        LibfprintBackend {
            ctx: FpContext::new(),
        }
    }
}

impl Default for LibfprintBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for LibfprintBackend {
    type Device = LibfprintDevice;

    async fn enumerate(&self) -> Result<Vec<LibfprintDevice>> {
        // Read each device's static getters on this thread, hand the resulting `DeviceInfo` to a
        // worker (which re-finds the device in its own context), and drop the caller-thread
        // `FpDevice`s as this Vec goes out of scope — the sensor is owned only by its worker.
        Ok(self
            .ctx
            .devices()
            .into_iter()
            .map(|dev| LibfprintDevice::spawn(convert::device_info(&dev)))
            .collect())
    }

    /// Locate a reader by id (convenience over `enumerate` + find); the returned device is not
    /// yet opened — call [`fprint_core::Device::open`] on it.
    ///
    /// Matches on libfprint's `device_id`, falling back to the driver id when `device_id` is
    /// empty, as it is for the virtual debug devices (mirroring how `DeviceInfo` is built).
    async fn open(&self, id: &DeviceId) -> Result<LibfprintDevice> {
        for dev in self.ctx.devices() {
            let device_id = dev.device_id();
            let hit = if device_id.is_empty() {
                dev.driver() == id.as_str()
            } else {
                device_id == id.as_str()
            };
            if hit {
                return Ok(LibfprintDevice::spawn(convert::device_info(&dev)));
            }
        }
        Err(Error::NotFound)
    }
}
