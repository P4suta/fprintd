// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [`ImageDevice`]: the [`fprint_core::Device`] that wires the detector and matcher together.
//!
//! Where [`crate::VirtualDevice`] scripts what the sensor pretends happened, `ImageDevice` runs the
//! host-image pipeline over whatever frames a [`FrameSource`] hands it: `enroll` detects
//! minutiae from each captured frame (`fprint_pipeline::template_from_images`) into an
//! [`fprint_core::Template::Nbis`], and `verify` / `identify` score a fresh scan against it with the
//! BOZORTH3 matcher (`fprint_pipeline::nbis_match_score`). It is a host-side sensor: no on-device
//! storage, and it surfaces the scanned print.
//!
//! Invariants follow the same rules as [`crate::VirtualDevice`]: one operation at a time is the
//! borrow checker's job (`&mut self`); cancellation is dropping the future — `enroll` commits its
//! `Print` only on the final poll, and the sole await inside the capture loop
//! ([`FrameSource::capture`]) is the poll boundary a drop cancels at; the only runtime guards are
//! [`Error::ProtoState`] before `open` and [`Error::NotSupported`] for absent features / absent
//! storage.

use fprint_core::{
    Device, DeviceFeature, DeviceInfo, EnrollDate, EnrollProgress, Error, FingerStatus,
    IdentifyOutcome, Print, Result, Template, VerifyOutcome,
};

use crate::frame::Frame;
use crate::frame_source::{Capture, FrameSource};

/// A host-image [`fprint_core::Device`] driven by a [`FrameSource`], matching through real MINDTCT +
/// BOZORTH3.
pub struct ImageDevice<S: FrameSource> {
    info: DeviceInfo,
    source: S,
    /// BOZORTH3 score at/above which a scan is a match.
    threshold: u32,
    open: bool,
    suspended: bool,
}

impl<S: FrameSource> ImageDevice<S> {
    /// Build a fresh, **closed** host-image device over `source`, matching at `threshold`.
    pub fn new(info: DeviceInfo, source: S, threshold: u32) -> Self {
        ImageDevice {
            info,
            source,
            threshold,
            open: false,
            suspended: false,
        }
    }

    /// Whether [`Device::open`] has been called (and not yet [`Device::close`]d).
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether the device is currently in the suspended state.
    pub fn is_suspended(&self) -> bool {
        self.suspended
    }

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

    /// Wrap a freshly detected scan template in a `Print` tagged for this device. A host sensor keeps
    /// nothing on-chip, so `device_stored` is always false.
    fn scan_print(&self, template: Template) -> Print {
        Print::builder()
            .template(template)
            .driver(self.info.driver.clone())
            .device_id(self.info.id.clone())
            .device_stored(false)
            .build()
    }

    /// Capture one frame and detect it into a single-sample [`Template::Nbis`]; a weak capture
    /// surfaces as [`Error::RetryScan`].
    async fn one_scan(&mut self) -> Result<Template> {
        match self.source.capture().await? {
            Capture::Frame(f) => Ok(fprint_pipeline::template_from_images(&[f.as_gray()?])),
            Capture::Retry(r) => Err(Error::RetryScan(r)),
        }
    }
}

impl<S: FrameSource> Device for ImageDevice<S> {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    async fn open(&mut self) -> Result<()> {
        if self.open {
            return Err(Error::ProtoState);
        }
        self.source.arm().await?;
        self.open = true;
        Ok(())
    }

    async fn close(&mut self) -> Result<()> {
        self.source.disarm().await?;
        self.open = false;
        Ok(())
    }

    async fn enroll<F: FnMut(EnrollProgress)>(
        &mut self,
        template: Print,
        mut on_progress: F,
    ) -> Result<Print> {
        self.guard_open()?;

        let total = self.info.enroll_stages;
        let mut completed: u32 = 0;
        let mut frames: Vec<Frame> = Vec::new();

        while completed < total {
            // `capture` is the sole await: this is the poll boundary a dropped future cancels at,
            // and nothing is returned until the loop finishes on the final poll.
            match self.source.capture().await? {
                Capture::Retry(reason) => {
                    // A weak capture: the stage does not advance; the reason rides along.
                    on_progress(EnrollProgress::new(completed, total).with_retry(reason));
                }
                Capture::Frame(frame) => {
                    frames.push(frame);
                    completed += 1;
                    on_progress(EnrollProgress::new(completed, total));
                }
            }
        }

        // Detect once per captured frame — one enrolled minutiae sample per capture.
        let grays: Vec<fprint_mindtct::GrayImage<'_>> =
            frames.iter().map(Frame::as_gray).collect::<Result<_>>()?;
        let detected = fprint_pipeline::template_from_images(&grays);

        // Fixed, deterministic date so enrolled prints are reproducible (as VirtualDevice does).
        Ok(Print::builder()
            .template(detected)
            .finger(template.finger)
            .driver(self.info.driver.clone())
            .device_id(self.info.id.clone())
            .device_stored(false)
            .enroll_date(EnrollDate::new(2026, 1, 1))
            .build())
    }

    async fn verify_with_status<F: FnMut(FingerStatus)>(
        &mut self,
        enrolled: &Print,
        mut on_status: F,
    ) -> Result<VerifyOutcome> {
        self.guard_open()?;
        self.need(DeviceFeature::VERIFY)?;

        let scanned = self.one_scan().await?;
        // A frame was captured, so a finger was on the sensor.
        on_status(FingerStatus::PRESENT);
        let matched = fprint_pipeline::nbis_verify(&enrolled.template, &scanned, self.threshold);
        Ok(VerifyOutcome::new(matched, Some(self.scan_print(scanned))))
    }

    async fn identify_with_status<F: FnMut(FingerStatus)>(
        &mut self,
        gallery: &[Print],
        mut on_status: F,
    ) -> Result<IdentifyOutcome> {
        self.guard_open()?;
        self.need(DeviceFeature::IDENTIFY)?;

        let scanned = self.one_scan().await?;
        on_status(FingerStatus::PRESENT);
        let galv: Vec<Template> = gallery.iter().map(|p| p.template.clone()).collect();
        let match_index = fprint_pipeline::nbis_identify(&scanned, &galv, self.threshold);
        Ok(IdentifyOutcome::new(
            match_index,
            Some(self.scan_print(scanned)),
        ))
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
