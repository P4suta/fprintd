// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [`VirtualDevice`]: the first concrete [`fp_core::Device`].
//!
//! It proves the async-trait seam is implementable with no runtime: every method is
//! straight-line and resolves on its first poll, except `enroll`, which awaits
//! [`yield_now`] once per capture stage so that dropping its future cancels the enrollment
//! (nothing is committed to storage until the final poll).
//!
//! Invariants are carried the way `ARCHITECTURE.md` prescribes: one operation at a time is
//! the borrow checker's job (`&mut self`), and there is no cancellation token — cancellation
//! is dropping the future. The only runtime guards are the ones the trait's error vocabulary
//! demands: [`Error::ProtoState`] before `open`, [`Error::NotSupported`] for absent features.

use fp_core::{
    Device, DeviceFeature, DeviceInfo, EnrollDate, EnrollProgress, Error, IdentifyOutcome, Print,
    Result, Template, VerifyOutcome,
};

use crate::scenario::{CaptureOutcome, EnrollScript, FingerId, Scenario};
use crate::store::PrintStore;
use crate::synth::{matches, template_for, TemplateKind};
use crate::yield_now::yield_now;

/// A pure-Rust, in-memory fingerprint reader driven by a [`Scenario`].
///
/// Construct one with a [`crate::VirtualDeviceBuilder`]. Beyond the [`fp_core::Device`]
/// trait it exposes a handful of test-only mutators ([`VirtualDevice::present_finger`] and
/// friends) so a test can change what the "sensor" sees between operations.
pub struct VirtualDevice {
    info: DeviceInfo,
    kind: TemplateKind,
    /// Whether verify/identify surface the freshly scanned print (host sensors do; MOC not).
    surfaces_scan: bool,
    open: bool,
    suspended: bool,
    /// The finger currently "on the sensor", if any.
    presented: Option<FingerId>,
    enroll: EnrollScript,
    store: PrintStore,
    /// Scenario override: make the next enroll report storage full regardless of `store`.
    force_data_full: bool,
}

impl VirtualDevice {
    /// Assemble from the builder's resolved parts (crate-internal; see the builder).
    pub(crate) fn from_parts(
        info: DeviceInfo,
        kind: TemplateKind,
        surfaces_scan: bool,
        capacity: Option<usize>,
        scenario: Scenario,
    ) -> Self {
        VirtualDevice {
            info,
            kind,
            surfaces_scan,
            open: false,
            suspended: false,
            presented: scenario.presented,
            enroll: scenario.enroll,
            store: PrintStore::new(capacity),
            force_data_full: scenario.force_data_full,
        }
    }

    // --- Test-only introspection & mutators -------------------------------------------

    /// Put a finger on the sensor (affects the next verify/identify).
    pub fn present_finger(&mut self, id: FingerId) {
        self.presented = Some(id);
    }

    /// Lift the finger off the sensor.
    pub fn clear_finger(&mut self) {
        self.presented = None;
    }

    /// Replace the enrollment script.
    pub fn set_enroll_script(&mut self, script: EnrollScript) {
        self.enroll = script;
    }

    /// The prints currently held in on-device storage (empty for host sensors).
    pub fn stored_prints(&self) -> &[Print] {
        self.store.as_slice()
    }

    /// Whether [`Device::open`] has been called (and not yet [`Device::close`]d).
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether the device is currently in the suspended state.
    pub fn is_suspended(&self) -> bool {
        self.suspended
    }

    // --- Internal guards & helpers -----------------------------------------------------

    /// `Ok` iff the device is open, else [`Error::ProtoState`].
    fn guard_open(&self) -> Result<()> {
        if self.open {
            Ok(())
        } else {
            Err(Error::ProtoState)
        }
    }

    /// `Ok` iff the device advertises `feature`, else [`Error::NotSupported`].
    fn need(&self, feature: DeviceFeature) -> Result<()> {
        if self.info.features.contains(feature) {
            Ok(())
        } else {
            Err(Error::NotSupported)
        }
    }

    /// Whether this is a match-on-chip device (has `STORAGE`).
    fn is_moc(&self) -> bool {
        self.info.features.is_match_on_chip()
    }

    /// Wrap a freshly synthesized scan template in a `Print` tagged for this device.
    fn scan_print(&self, template: Template) -> Print {
        Print {
            template,
            driver: Some(self.info.driver.clone()),
            device_id: Some(self.info.id.clone()),
            device_stored: self.is_moc(),
            ..Print::default()
        }
    }
}

impl Device for VirtualDevice {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    async fn open(&mut self) -> Result<()> {
        if self.open {
            return Err(Error::ProtoState);
        }
        self.open = true;
        Ok(())
    }

    async fn close(&mut self) -> Result<()> {
        self.open = false;
        Ok(())
    }

    async fn enroll<F: FnMut(EnrollProgress)>(
        &mut self,
        template: Print,
        mut on_progress: F,
    ) -> Result<Print> {
        self.guard_open()?;

        let is_moc = self.is_moc();
        if self.force_data_full || (is_moc && self.store.is_full()) {
            return Err(Error::DataFull);
        }

        let want = template_for(self.kind, self.enroll.produces.unwrap_or(FingerId(0)));
        if self.info.features.contains(DeviceFeature::DUPLICATES_CHECK)
            && self.store.contains_template(&want)
        {
            return Err(Error::DataDuplicate);
        }

        let total = self.info.enroll_stages;
        let mut completed: u32 = 0;
        let mut steps = self.enroll.steps.clone().into_iter();

        while completed < total {
            // One poll boundary per capture stage: this is where a dropped future cancels.
            yield_now().await;

            match steps.next().unwrap_or(CaptureOutcome::Advance) {
                CaptureOutcome::Retry(reason) => {
                    // A retry: the stage does not advance; the reason rides along so the
                    // daemon can pick the matching `enroll-*` status string.
                    on_progress(EnrollProgress {
                        completed_stages: completed,
                        total_stages: total,
                        retry: Some(reason),
                    });
                }
                CaptureOutcome::Advance => {
                    completed += 1;
                    on_progress(EnrollProgress {
                        completed_stages: completed,
                        total_stages: total,
                        retry: None,
                    });
                }
            }
        }

        let finished = Print {
            template: want,
            finger: template.finger,
            driver: Some(self.info.driver.clone()),
            device_id: Some(self.info.id.clone()),
            device_stored: is_moc,
            // Stamp a fixed, deterministic date so enrolled prints are reproducible and the
            // FP3 date round-trip (Gregorian <-> Julian day) is exercised end-to-end.
            enroll_date: Some(EnrollDate {
                year: 2026,
                month: 1,
                day: 1,
            }),
            ..Print::default()
        };

        // Commit to on-device storage only now, on the final poll — so a cancelled
        // (dropped) enroll leaves storage untouched.
        if is_moc {
            self.store.push(finished.clone())?;
        }

        Ok(finished)
    }

    async fn verify(&mut self, enrolled: &Print) -> Result<VerifyOutcome> {
        self.guard_open()?;
        self.need(DeviceFeature::VERIFY)?;

        let scanned = self.presented.map(|id| template_for(self.kind, id));
        let matched = scanned
            .as_ref()
            .is_some_and(|s| matches(&enrolled.template, s));
        let scanned = if self.surfaces_scan {
            scanned.map(|t| self.scan_print(t))
        } else {
            None
        };

        Ok(VerifyOutcome { matched, scanned })
    }

    async fn identify(&mut self, gallery: &[Print]) -> Result<IdentifyOutcome> {
        self.guard_open()?;
        self.need(DeviceFeature::IDENTIFY)?;

        let scanned = self.presented.map(|id| template_for(self.kind, id));
        let match_index = scanned
            .as_ref()
            .and_then(|s| gallery.iter().position(|p| matches(&p.template, s)));
        let scanned = if self.surfaces_scan {
            scanned.map(|t| self.scan_print(t))
        } else {
            None
        };

        Ok(IdentifyOutcome {
            match_index,
            scanned,
        })
    }

    async fn list_prints(&mut self) -> Result<Vec<Print>> {
        self.guard_open()?;
        self.need(DeviceFeature::STORAGE_LIST)?;
        Ok(self.store.as_slice().to_vec())
    }

    async fn delete_print(&mut self, print: &Print) -> Result<()> {
        self.guard_open()?;
        self.need(DeviceFeature::STORAGE_DELETE)?;
        self.store.remove_by_template(&print.template)
    }

    async fn clear_storage(&mut self) -> Result<()> {
        self.guard_open()?;
        self.need(DeviceFeature::STORAGE_CLEAR)?;
        self.store.clear();
        Ok(())
    }

    async fn suspend(&mut self) -> Result<()> {
        self.suspended = true;
        Ok(())
    }

    async fn resume(&mut self) -> Result<()> {
        self.suspended = false;
        Ok(())
    }
}
