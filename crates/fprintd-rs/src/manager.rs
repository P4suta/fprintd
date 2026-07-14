// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `net.reactivated.Fprint.Manager` at `/net/reactivated/Fprint/Manager`.
//!
//! The manager is the client's entry point: it enumerates the device object paths and names
//! a default. Since the daemon discovers devices once at start-up (M1 has no hotplug), the
//! manager just holds the fixed path list. `GetDefaultDevice` returns the last device, as
//! fprintd's `fprint_manager_get_default_device` does.

use zbus::zvariant::OwnedObjectPath;

use crate::error::DaemonError;

/// The manager D-Bus object.
pub struct Manager {
    device_paths: Vec<OwnedObjectPath>,
}

impl Manager {
    /// Create a manager over the given device object paths (in discovery order).
    pub fn new(device_paths: Vec<OwnedObjectPath>) -> Self {
        Manager { device_paths }
    }
}

#[zbus::interface(name = "net.reactivated.Fprint.Manager")]
impl Manager {
    /// All fingerprint reader object paths; an empty array if none are attached.
    async fn get_devices(&self) -> Vec<OwnedObjectPath> {
        self.device_paths.clone()
    }

    /// The default reader — the last discovered device — or `NoSuchDevice` if none.
    async fn get_default_device(&self) -> Result<OwnedObjectPath, DaemonError> {
        self.device_paths
            .last()
            .cloned()
            .ok_or_else(|| DaemonError::NoSuchDevice("No devices available".into()))
    }
}
