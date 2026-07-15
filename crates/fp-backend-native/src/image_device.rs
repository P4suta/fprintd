// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [`ImageDevice`]: the first [`fp_core::Device`] to wire the real detector and matcher together.
//!
//! Where [`crate::VirtualDevice`] scripts *what the sensor pretends happened*, `ImageDevice` runs the
//! genuine host-image pipeline over whatever frames a [`FrameSource`] hands it: `enroll` detects
//! minutiae from each captured frame (`crate::detector::template_from_images`) into an
//! [`fp_core::Template::Nbis`], and `verify` / `identify` score a fresh scan against it with the real
//! BOZORTH3 matcher (`crate::matcher`). It is a host-side sensor — no on-device storage, and it
//! surfaces the scanned print.
//!
//! Invariants follow the same rules as [`crate::VirtualDevice`]: one operation at a time is the
//! borrow checker's job (`&mut self`); cancellation is dropping the future — `enroll` commits its
//! `Print` only on the final poll, and the sole await inside the capture loop
//! ([`FrameSource::capture`]) is the poll boundary a drop cancels at; the only runtime guards are
//! [`Error::ProtoState`] before `open` and [`Error::NotSupported`] for absent features / absent
//! storage.

use fp_core::{
    Device, DeviceFeature, DeviceInfo, EnrollDate, EnrollProgress, Error, IdentifyOutcome, Print,
    Result, Template, VerifyOutcome,
};

use crate::frame::Frame;
use crate::frame_source::{Capture, FrameSource};

/// A host-image [`fp_core::Device`] driven by a [`FrameSource`], matching through real MINDTCT +
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
        Print {
            template,
            driver: Some(self.info.driver.clone()),
            device_id: Some(self.info.id.clone()),
            device_stored: false,
            ..Print::default()
        }
    }

    /// Capture one frame and detect it into a single-sample [`Template::Nbis`]; a weak capture
    /// surfaces as [`Error::RetryScan`].
    async fn one_scan(&mut self) -> Result<Template> {
        match self.source.capture().await? {
            Capture::Frame(f) => Ok(crate::detector::template_from_images(&[f.as_gray()])),
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
                    on_progress(EnrollProgress {
                        completed_stages: completed,
                        total_stages: total,
                        retry: Some(reason),
                    });
                }
                Capture::Frame(frame) => {
                    frames.push(frame);
                    completed += 1;
                    on_progress(EnrollProgress {
                        completed_stages: completed,
                        total_stages: total,
                        retry: None,
                    });
                }
            }
        }

        // Detect once per captured frame — one enrolled minutiae sample per capture.
        let grays: Vec<fp_mindtct::GrayImage<'_>> = frames.iter().map(Frame::as_gray).collect();
        let detected = crate::detector::template_from_images(&grays);

        Ok(Print {
            template: detected,
            finger: template.finger,
            driver: Some(self.info.driver.clone()),
            device_id: Some(self.info.id.clone()),
            device_stored: false,
            // Fixed, deterministic date so enrolled prints are reproducible (as VirtualDevice does).
            enroll_date: Some(EnrollDate {
                year: 2026,
                month: 1,
                day: 1,
            }),
            ..Print::default()
        })
    }

    async fn verify(&mut self, enrolled: &Print) -> Result<VerifyOutcome> {
        self.guard_open()?;
        self.need(DeviceFeature::VERIFY)?;

        let scanned = self.one_scan().await?;
        let matched =
            crate::matcher::nbis_match_score(&enrolled.template, &scanned) >= self.threshold;
        Ok(VerifyOutcome {
            matched,
            scanned: Some(self.scan_print(scanned)),
        })
    }

    async fn identify(&mut self, gallery: &[Print]) -> Result<IdentifyOutcome> {
        self.guard_open()?;
        self.need(DeviceFeature::IDENTIFY)?;

        let scanned = self.one_scan().await?;
        let galv: Vec<Template> = gallery.iter().map(|p| p.template.clone()).collect();
        let match_index = crate::matcher::nbis_identify(&scanned, &galv, self.threshold);
        Ok(IdentifyOutcome {
            match_index,
            scanned: Some(self.scan_print(scanned)),
        })
    }

    async fn list_prints(&mut self) -> Result<Vec<Print>> {
        // Host-image sensors keep no templates on the device.
        Err(Error::NotSupported)
    }

    async fn delete_print(&mut self, _print: &Print) -> Result<()> {
        Err(Error::NotSupported)
    }

    async fn clear_storage(&mut self) -> Result<()> {
        Err(Error::NotSupported)
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
