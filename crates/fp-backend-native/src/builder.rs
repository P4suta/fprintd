// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [`VirtualDeviceBuilder`]: describe a virtual reader, then stamp out [`VirtualDevice`]s.
//!
//! Two presets cover the two archetypes fp-core distinguishes:
//! * [`VirtualDeviceBuilder::host_image_sensor`] — a host-side image sensor: multi-stage
//!   enrollment, NBIS templates, surfaces the scanned print, no on-device storage.
//! * [`VirtualDeviceBuilder::chip_storage_sensor`] — a match-on-chip sensor: single-stage
//!   enrollment, opaque Raw templates kept on the device, hidden scans, duplicate checking.
//!
//! A builder is a *description*, not a device: [`build`](VirtualDeviceBuilder::build) takes
//! `&self`, so one builder can mint many fresh devices (a [`crate::VirtualBackend`] holds a
//! `Vec` of them and builds on demand).

use fp_core::{DeviceFeature, DeviceId, DeviceInfo, DriverId, ScanType};

use crate::device::VirtualDevice;
use crate::scenario::Scenario;
use crate::synth::TemplateKind;

/// A reusable description of a virtual device.
///
/// `#[must_use]`: the consuming builder methods return a fresh `Self`, so an ignored result
/// (`b.name("x");`) is a bug — the whole type is `must_use` to catch it.
#[derive(Clone, Debug)]
#[must_use]
pub struct VirtualDeviceBuilder {
    id: Option<DeviceId>,
    driver: String,
    name: String,
    scan_type: ScanType,
    features: DeviceFeature,
    enroll_stages: u32,
    kind: TemplateKind,
    surfaces_scan: bool,
    capacity: Option<usize>,
    scenario: Scenario,
    /// `Some(threshold)` switches NBIS matching from the synthetic byte-equality stub to the real
    /// [`fp_bozorth3`] matcher (score `>= threshold` is a match). `None` keeps the deterministic stub.
    match_threshold: Option<u32>,
}

impl VirtualDeviceBuilder {
    /// Preset: a host-side image sensor (`virtual_image`).
    ///
    /// `CAPTURE | VERIFY | IDENTIFY`, five enroll stages, NBIS templates, surfaces the scan,
    /// no on-device storage.
    pub fn host_image_sensor() -> Self {
        VirtualDeviceBuilder {
            id: None,
            driver: "virtual_image".to_string(),
            name: "Virtual Image Sensor".to_string(),
            scan_type: ScanType::Press,
            features: DeviceFeature::CAPTURE | DeviceFeature::VERIFY | DeviceFeature::IDENTIFY,
            enroll_stages: 5,
            kind: TemplateKind::Nbis,
            surfaces_scan: true,
            capacity: None,
            scenario: Scenario::new(),
            match_threshold: None,
        }
    }

    /// Preset: a match-on-chip sensor (`virtual_moc`).
    ///
    /// `VERIFY | IDENTIFY | STORAGE | STORAGE_LIST | STORAGE_DELETE | STORAGE_CLEAR |
    /// DUPLICATES_CHECK`, one enroll stage, Raw templates stored on the device (capacity 10),
    /// scans hidden.
    pub fn chip_storage_sensor() -> Self {
        VirtualDeviceBuilder {
            id: None,
            driver: "virtual_moc".to_string(),
            name: "Virtual MOC Sensor".to_string(),
            scan_type: ScanType::Press,
            features: DeviceFeature::VERIFY
                | DeviceFeature::IDENTIFY
                | DeviceFeature::STORAGE
                | DeviceFeature::STORAGE_LIST
                | DeviceFeature::STORAGE_DELETE
                | DeviceFeature::STORAGE_CLEAR
                | DeviceFeature::DUPLICATES_CHECK,
            enroll_stages: 1,
            kind: TemplateKind::Raw,
            surfaces_scan: false,
            capacity: Some(10),
            scenario: Scenario::new(),
            match_threshold: None,
        }
    }

    /// Override the device id (otherwise defaults to the driver name).
    pub fn id(mut self, id: DeviceId) -> Self {
        self.id = Some(id);
        self
    }

    /// Override the human-readable model name.
    pub fn name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    /// Override the advertised capability flags.
    ///
    /// Beyond the presets this is mainly a testing hook — e.g. building a device that lacks
    /// `VERIFY` to exercise the [`fp_core::Error::NotSupported`] path.
    pub fn features(mut self, features: DeviceFeature) -> Self {
        self.features = features;
        self
    }

    /// Override the number of enrollment stages.
    pub fn enroll_stages(mut self, stages: u32) -> Self {
        self.enroll_stages = stages;
        self
    }

    /// Give the device bounded on-device storage of the given capacity.
    pub fn storage_capacity(mut self, capacity: usize) -> Self {
        self.capacity = Some(capacity);
        self
    }

    /// Attach the [`Scenario`] the built device will act out.
    pub fn scenario(mut self, scenario: Scenario) -> Self {
        self.scenario = scenario;
        self
    }

    /// Match NBIS templates with the **real** [`fp_bozorth3`] matcher (score `>= threshold` matches),
    /// instead of the default synthetic byte-equality. Pair with [`Scenario::enroll_real`] /
    /// [`Scenario::present_real`] to feed genuine minutiae; `Raw`/MOC templates stay byte-compared.
    pub fn bozorth3_matching(mut self, threshold: u32) -> Self {
        self.match_threshold = Some(threshold);
        self
    }

    /// The id this builder will assign — the override, or the driver name by default.
    pub(crate) fn effective_id(&self) -> DeviceId {
        self.id
            .clone()
            .unwrap_or_else(|| DeviceId(self.driver.clone()))
    }

    /// Mint a fresh, closed [`VirtualDevice`] from this description.
    #[must_use]
    pub fn build(&self) -> VirtualDevice {
        let info = DeviceInfo {
            id: self.effective_id(),
            driver: DriverId(self.driver.clone()),
            name: self.name.clone(),
            scan_type: self.scan_type,
            features: self.features,
            enroll_stages: self.enroll_stages,
        };
        VirtualDevice::from_parts(
            info,
            self.kind,
            self.surfaces_scan,
            self.capacity,
            self.scenario.clone(),
            self.match_threshold,
        )
    }
}
