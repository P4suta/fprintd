// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [`VirtualDeviceBuilder`]: describe a virtual reader, then stamp out [`VirtualDevice`]s.
//!
//! Two presets cover the two archetypes fprint-core distinguishes:
//! * [`VirtualDeviceBuilder::host_image_sensor`] — a host-side image sensor: multi-stage
//!   enrollment, NBIS templates, surfaces the scanned print, no on-device storage.
//! * [`VirtualDeviceBuilder::chip_storage_sensor`] — a match-on-chip sensor: single-stage
//!   enrollment, opaque Raw templates kept on the device, hidden scans, duplicate checking.
//!
//! A builder is a *description*, not a device: [`build`](VirtualDeviceBuilder::build) takes
//! `&self`, so one builder can mint many fresh devices (a [`crate::VirtualBackend`] holds a
//! `Vec` of them and builds on demand).

use fprint_core::{DeviceFeature, DeviceId, DeviceInfo, DriverId, Finger, Print, ScanType};

use crate::device::VirtualDevice;
use crate::scenario::Scenario;
use crate::store::PrintStore;
use crate::synth::TemplateKind;

/// A test-side view of a device's shared on-chip storage: the slots the "sensor" holds after a
/// sequence of daemon operations. Obtained from [`VirtualDeviceBuilder::storage_handle`] on a
/// builder configured with [`shared_storage`](VirtualDeviceBuilder::shared_storage), so a test can
/// assert what a `Claim`/enroll/delete sequence left on the sensor — the one thing the D-Bus
/// surface never exposes directly.
#[derive(Clone)]
pub struct SharedStorage {
    store: PrintStore,
}

impl SharedStorage {
    /// The prints currently on the sensor, in insertion order.
    #[must_use]
    pub fn prints(&self) -> Vec<Print> {
        self.store.snapshot()
    }

    /// How many slots are occupied.
    #[must_use]
    pub fn len(&self) -> usize {
        self.store.snapshot().len()
    }

    /// Whether the sensor holds no prints.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.store.snapshot().is_empty()
    }

    /// The fingers currently enrolled on the sensor (slots that name a real finger).
    #[must_use]
    pub fn fingers(&self) -> Vec<Finger> {
        self.store
            .snapshot()
            .into_iter()
            .filter_map(|p| p.finger)
            .collect()
    }
}

/// The fields of a `DeviceInfo` a driver may still change after enumeration.
///
/// A C libfprint driver can set its scan type, features and enroll-stage count from its
/// probe/open path (`fpi_device_set_scan_type`, `fpi_device_update_features`,
/// `fpi_device_set_nr_enroll_stages`), so what [`fprint_core::Backend::enumerate`] reports is a
/// class default. The libfprint shim re-reads `DeviceInfo` in `Device::open` for this reason.
///
/// Identity (`id`, `driver`) and the model name are absent because a driver cannot change them.
///
/// Used by [`VirtualDeviceBuilder::probe_reports`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceShape {
    pub scan_type: ScanType,
    pub features: DeviceFeature,
    pub enroll_stages: u32,
}

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
    /// [`fprint_bozorth3`] matcher (score `>= threshold` is a match). `None` keeps the deterministic stub.
    match_threshold: Option<u32>,
    /// `Some` ⇒ enumeration advertises this instead, and `open` settles to the real shape.
    /// `None` ⇒ the device is what it says it is from the start (most tests want this).
    probed: Option<DeviceShape>,
    /// `Some` ⇒ every device this builder mints shares this one on-chip store, so slots persist
    /// across the open/close cycles the daemon drives on each `Claim` (a real sensor keeps them).
    /// `None` ⇒ each device gets its own fresh store. Set via [`VirtualDeviceBuilder::shared_storage`].
    shared: Option<PrintStore>,
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
            probed: None,
            shared: None,
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
            probed: None,
            shared: None,
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
    /// `VERIFY` to exercise the [`fprint_core::Error::NotSupported`] path.
    pub fn features(mut self, features: DeviceFeature) -> Self {
        self.features = features;
        self
    }

    /// Override the number of enrollment stages.
    pub fn enroll_stages(mut self, stages: u32) -> Self {
        self.enroll_stages = stages;
        self
    }

    /// Override the scan type. Both presets are [`ScanType::Press`].
    pub fn scan_type(mut self, scan_type: ScanType) -> Self {
        self.scan_type = scan_type;
        self
    }

    /// Report `probed` from enumeration, and settle to this builder's real shape on
    /// [`fprint_core::Device::open`]. See [`DeviceShape`].
    pub fn probe_reports(mut self, probed: DeviceShape) -> Self {
        self.probed = Some(probed);
        self
    }

    /// Give the device bounded on-device storage of the given capacity.
    pub fn storage_capacity(mut self, capacity: usize) -> Self {
        self.capacity = Some(capacity);
        self
    }

    /// Persist on-chip storage across the devices this builder mints, at the builder's current
    /// capacity. Every [`build`](Self::build) then shares one store, so slots enrolled through one
    /// device handle are still there on the next — modelling a real match-on-chip sensor whose
    /// memory survives the open/close the daemon drives on each `Claim`. Call it *after* setting the
    /// capacity (the presets already have). Test-only; the default (fresh store per build) is right
    /// for host sensors and for tests that want an independent device each time.
    pub fn shared_storage(mut self) -> Self {
        self.shared = Some(PrintStore::new(self.capacity));
        self
    }

    /// A test-side [`SharedStorage`] view of this builder's shared on-chip store, or `None` if
    /// [`shared_storage`](Self::shared_storage) was not set. Clone the builder into the daemon's
    /// backend factory and keep this handle to assert what the sensor holds after a run.
    #[must_use]
    pub fn storage_handle(&self) -> Option<SharedStorage> {
        self.shared.clone().map(|store| SharedStorage { store })
    }

    /// Attach the [`Scenario`] the built device will act out.
    pub fn scenario(mut self, scenario: Scenario) -> Self {
        self.scenario = scenario;
        self
    }

    /// Match NBIS templates with the **real** [`fprint_bozorth3`] matcher (score `>= threshold` matches),
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
            .unwrap_or_else(|| DeviceId::new(self.driver.clone()))
    }

    /// Mint a fresh, closed [`VirtualDevice`] from this description.
    #[must_use]
    pub fn build(&self) -> VirtualDevice {
        let real = DeviceShape {
            scan_type: self.scan_type,
            features: self.features,
            enroll_stages: self.enroll_stages,
        };
        // Without a probe/open split, the device is its real shape from the start and `open`
        // has nothing to settle.
        let (advertised, settles_to) = match self.probed {
            Some(probed) => (probed, Some(real)),
            None => (real, None),
        };
        let info = DeviceInfo::new(
            self.effective_id(),
            DriverId::new(self.driver.clone()),
            self.name.clone(),
            advertised.scan_type,
            advertised.features,
            advertised.enroll_stages,
        );
        // A shared store persists across builds (a real on-chip sensor); otherwise each device gets
        // its own fresh store at this builder's capacity.
        let store = self
            .shared
            .clone()
            .unwrap_or_else(|| PrintStore::new(self.capacity));
        VirtualDevice::from_parts(
            info,
            settles_to,
            self.kind,
            self.surfaces_scan,
            store,
            self.scenario.clone(),
            self.match_threshold,
        )
    }
}
