// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [`VirtualDevice`]: an in-memory [`fprint_core::Device`].
//!
//! No async runtime is needed: every method is straight-line and resolves on its first poll,
//! except `enroll`, which awaits `yield_now` once per capture stage so that dropping its future
//! cancels the enrollment (nothing is committed to storage until the final poll).
//!
//! Invariants follow `ARCHITECTURE.md`: one operation at a time is the borrow checker's job
//! (`&mut self`), and there is no cancellation token — cancellation is dropping the future. The
//! only runtime guards are the ones the trait's error vocabulary demands:
//! [`Error::ProtoState`] before `open`, [`Error::NotSupported`] for absent features.

use fprint_core::{
    Device, DeviceFeature, DeviceInfo, EnrollDate, EnrollProgress, Error, FingerStatus,
    IdentifyOutcome, Print, Result, Template, VerifyOutcome,
};

use crate::builder::DeviceShape;
use crate::scenario::{CaptureOutcome, EnrollScript, FingerId, Scenario};
use crate::store::PrintStore;
use crate::synth::{matches, template_for, TemplateKind};
use crate::yield_now::yield_now;

/// A pure-Rust, in-memory fingerprint reader driven by a [`Scenario`].
///
/// Construct one with a [`crate::VirtualDeviceBuilder`]. Beyond the [`fprint_core::Device`]
/// trait it exposes a handful of test-only mutators ([`VirtualDevice::present_finger`] and
/// friends) so a test can change what the "sensor" sees between operations.
pub struct VirtualDevice {
    /// The probed shape until `open`, the settled one after.
    info: DeviceInfo,
    /// The shape `open` settles to, if the builder modelled a probe/open split (see
    /// [`crate::DeviceShape`]); `None` otherwise. Only the refinable fields are stored, not a
    /// second `DeviceInfo`: identity cannot change, and a `DeviceInfo` here would grow
    /// `CompositeDevice`'s largest variant by ~90 bytes.
    settles_to: Option<DeviceShape>,
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
    /// `Some(threshold)` ⇒ NBIS templates are matched by the real [`fprint_bozorth3`] matcher
    /// (score `>= threshold`); `None` ⇒ the synthetic byte-equality stub (`crate::synth`).
    match_threshold: Option<u32>,
    /// Real minutiae the host-image path enrolls, overriding the synthetic template.
    enroll_template: Option<Template>,
    /// Real minutiae presented as the live scan (a distinct capture from the enrolled one).
    presented_template: Option<Template>,
    /// Test hook: when set, `verify` reports the finger present and then hangs (awaits a future that
    /// never resolves), so a caller can exercise cancellation or suspend-preemption of an in-flight
    /// operation. Set via [`Scenario::hang`].
    hang: bool,
}

impl VirtualDevice {
    /// Assemble from the builder's resolved parts (crate-internal; see the builder).
    pub(crate) fn from_parts(
        info: DeviceInfo,
        settles_to: Option<DeviceShape>,
        kind: TemplateKind,
        surfaces_scan: bool,
        store: PrintStore,
        scenario: Scenario,
        match_threshold: Option<u32>,
    ) -> Self {
        VirtualDevice {
            info,
            settles_to,
            kind,
            surfaces_scan,
            open: false,
            suspended: false,
            presented: scenario.presented,
            enroll: scenario.enroll,
            store,
            force_data_full: scenario.force_data_full,
            match_threshold,
            enroll_template: scenario.enroll_template,
            presented_template: scenario.presented_template,
            hang: scenario.hang,
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
    pub fn stored_prints(&self) -> Vec<Print> {
        self.store.snapshot()
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
        Print::builder()
            .template(template)
            .driver(self.info.driver.clone())
            .device_id(self.info.id.clone())
            .device_stored(self.is_moc())
            .build()
    }

    /// The template presented as the live scan: the scenario's real capture if one was set, else
    /// the synthetic template for the presented finger id.
    fn scan_template(&self) -> Option<Template> {
        self.presented_template
            .clone()
            .or_else(|| self.presented.map(|id| template_for(self.kind, id)))
    }

    /// Whether `scanned` matches `enrolled`. With a `match_threshold` set and both sides NBIS, this
    /// is the **real** BOZORTH3 score `>= threshold` (`fprint_pipeline::nbis_match_score`); otherwise
    /// it is the synthetic byte-equality stub (`crate::synth::matches`) — the default, and the only
    /// path for `Raw`/MOC handles.
    fn match_templates(&self, enrolled: &Template, scanned: &Template) -> bool {
        match self.match_threshold {
            Some(threshold)
                if matches!(enrolled, Template::Nbis(_))
                    && matches!(scanned, Template::Nbis(_)) =>
            {
                fprint_pipeline::nbis_match_score(enrolled, scanned).accepts(threshold)
            }
            _ => matches(enrolled, scanned),
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
        // The shape settles at open, as it does in the libfprint shim. A no-op unless a
        // probe/open split was modelled. `take`, because `close` does not un-settle it.
        if let Some(shape) = self.settles_to.take() {
            self.info.scan_type = shape.scan_type;
            self.info.features = shape.features;
            self.info.enroll_stages = shape.enroll_stages;
        }
        Ok(())
    }

    async fn close(&mut self) -> Result<()> {
        self.open = false;
        Ok(())
    }

    async fn enroll<F: FnMut(EnrollProgress)>(
        &mut self,
        print: Print,
        mut on_progress: F,
    ) -> Result<Print> {
        self.guard_open()?;

        let is_moc = self.is_moc();
        if self.force_data_full || (is_moc && self.store.is_full()) {
            return Err(Error::DataFull);
        }

        let want = self.enroll_template.clone().unwrap_or_else(|| {
            template_for(self.kind, self.enroll.produces.unwrap_or(FingerId(0)))
        });
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
                    on_progress(EnrollProgress::new(completed, total).with_retry(reason));
                }
                CaptureOutcome::Advance => {
                    completed += 1;
                    on_progress(EnrollProgress::new(completed, total));
                }
            }
        }

        // Stamp a fixed, deterministic date so enrolled prints are reproducible and the
        // FP3 date round-trip (Gregorian <-> Julian day) is exercised end-to-end.
        let finished = Print::builder()
            .template(want)
            .finger(print.finger)
            // Preserve the owner on the stored slot, mirroring a real match-on-chip sensor (and the
            // libfprint shim, which recovers it from the FP3 blob): the daemon attributes on-sensor
            // slots to users by this, so a shared reader never frees another user's slot.
            .username(print.username)
            .driver(self.info.driver.clone())
            .device_id(self.info.id.clone())
            .device_stored(is_moc)
            .enroll_date(EnrollDate::new(2026, 1, 1))
            .build();

        // Commit to on-device storage only now, on the final poll — so a cancelled
        // (dropped) enroll leaves storage untouched.
        if is_moc {
            self.store.push(finished.clone())?;
        }

        Ok(finished)
    }

    async fn verify_with_status<F: FnMut(FingerStatus)>(
        &mut self,
        enrolled: &Print,
        mut on_status: F,
    ) -> Result<VerifyOutcome> {
        self.guard_open()?;
        self.need(DeviceFeature::VERIFY)?;

        let scanned = self.scan_template();
        // The scripted device models a finger being presented: report it when a scan is available.
        if scanned.is_some() {
            on_status(FingerStatus::PRESENT);
        }
        if self.hang {
            // Finger reported, then never resolve: the caller can now cancel (drop the future) or
            // preempt (suspend) this in-flight operation. Used only by the suspend-preemption test.
            std::future::pending::<()>().await;
        }
        let matched = scanned
            .as_ref()
            .is_some_and(|s| self.match_templates(&enrolled.template, s));
        let scanned = if self.surfaces_scan {
            scanned.map(|t| self.scan_print(t))
        } else {
            None
        };

        Ok(VerifyOutcome::new(matched, scanned))
    }

    async fn identify_with_status<F: FnMut(FingerStatus)>(
        &mut self,
        gallery: &[Print],
        mut on_status: F,
    ) -> Result<IdentifyOutcome> {
        self.guard_open()?;
        self.need(DeviceFeature::IDENTIFY)?;

        let scanned = self.scan_template();
        if scanned.is_some() {
            on_status(FingerStatus::PRESENT);
        }
        let match_index = scanned.as_ref().and_then(|s| match self.match_threshold {
            // With a threshold and an NBIS scan, identify through the real BOZORTH3 matcher (1:N,
            // strongest above threshold) — mirroring `verify`'s `match_templates` NBIS branch.
            Some(t) if matches!(s, Template::Nbis(_)) => {
                let templates: Vec<Template> = gallery.iter().map(|p| p.template.clone()).collect();
                fprint_pipeline::nbis_identify(s, &templates, t)
            }
            _ => gallery
                .iter()
                .position(|p| self.match_templates(&p.template, s)),
        });
        let scanned = if self.surfaces_scan {
            scanned.map(|t| self.scan_print(t))
        } else {
            None
        };

        Ok(IdentifyOutcome::new(match_index, scanned))
    }

    async fn list_prints(&mut self) -> Result<Vec<Print>> {
        self.guard_open()?;
        self.need(DeviceFeature::STORAGE_LIST)?;
        Ok(self.store.snapshot())
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
