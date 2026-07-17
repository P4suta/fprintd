// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [`LibfprintDevice`]: an `fprint-core` [`Device`] backed by a C-libfprint `FpDevice`.
//!
//! Every operation is a blocking `*_sync` call into libfprint (the binding exposes no async
//! entry points), translated in and out of the domain model by [`crate::convert`] and
//! [`crate::print`]. The type is `!Send` — it holds a glib `FpDevice` — and one operation at
//! a time is guaranteed by `&mut self`, matching libfprint's single-in-flight contract without
//! any runtime guard.

use core::ffi::c_void;

use fprint_core::{
    Device, DeviceFeature, DeviceInfo, EnrollProgress, Error, FingerStatus, IdentifyOutcome, Print,
    Result, VerifyOutcome,
};
use gio::Cancellable;
use libfprint_rs::{FpDevice, FpEnrollProgress, FpMatchCb, FpPrint};

use crate::progress::{on_enroll_progress, on_match_status, MatchTrampoline, Trampoline};
use crate::{convert, print, storage};

/// A fingerprint reader driven through the C libfprint.
///
/// Obtained from [`crate::LibfprintBackend`]; call [`Device::open`] before any operation.
pub struct LibfprintDevice {
    dev: FpDevice,
    info: DeviceInfo,
    /// The cancellable handed to the currently in-flight operation, if any. See the crate-level
    /// note on the cancellation limitation: it is installed but cannot be fired from a `Drop`
    /// that never runs while the thread is parked inside a blocking `*_sync` call.
    cancel: Option<Cancellable>,
}

impl LibfprintDevice {
    /// Wrap a device discovered by the backend (its [`DeviceInfo`] is read from the getters,
    /// then refreshed on [`Device::open`] since some fields firm up only once the device is open).
    pub(crate) fn from_device(dev: FpDevice) -> Self {
        let info = convert::device_info(&dev);
        LibfprintDevice {
            dev,
            info,
            cancel: None,
        }
    }
}

impl Drop for LibfprintDevice {
    fn drop(&mut self) {
        if self.dev.is_open() {
            // Best-effort release of the sensor; nothing to do if it fails as we are tearing down.
            let _ = self.dev.close_sync(None);
        }
    }
}

impl Device for LibfprintDevice {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    async fn open(&mut self) -> Result<()> {
        self.dev.open_sync(None).map_err(convert::from_gerror)?;
        // Features, scan type and enroll-stage count can become known only after open.
        self.info = convert::device_info(&self.dev);
        Ok(())
    }

    async fn close(&mut self) -> Result<()> {
        self.dev.close_sync(None).map_err(convert::from_gerror)
    }

    async fn enroll<F: FnMut(EnrollProgress)>(
        &mut self,
        template: Print,
        mut on_progress: F,
    ) -> Result<Print> {
        let fp = print::core_to_fp(&template, &self.dev);
        self.cancel = Some(Cancellable::new());

        let mut trampoline = Trampoline {
            cb: &mut on_progress,
            total: self.info.enroll_stages,
        };
        let user_data: *mut c_void = (&mut trampoline as *mut Trampoline<'_, F>).cast();

        let result = self.dev.enroll_sync(
            fp,
            self.cancel.as_ref(),
            Some(on_enroll_progress::<F> as FpEnrollProgress<*mut c_void>),
            Some(user_data),
        );

        self.cancel = None;
        let enrolled = result.map_err(convert::from_gerror)?;
        print::fp_to_core(&enrolled)
    }

    async fn verify_with_status<F: FnMut(FingerStatus)>(
        &mut self,
        enrolled: &Print,
        mut on_status: F,
    ) -> Result<VerifyOutcome> {
        let fp = print::core_to_fp_for_match(enrolled)?;
        let mut scanned = FpPrint::new(&self.dev);
        self.cancel = Some(Cancellable::new());

        let mut trampoline = MatchTrampoline { cb: &mut on_status };
        let user_data: *mut c_void = (&mut trampoline as *mut MatchTrampoline<'_, F>).cast();

        let result = self.dev.verify_sync::<*mut c_void>(
            &fp,
            self.cancel.clone(),
            Some(on_match_status::<F> as FpMatchCb<*mut c_void>),
            Some(user_data),
            Some(&mut scanned),
        );

        self.cancel = None;
        let matched = result.map_err(convert::from_gerror)?;

        // Match-on-chip sensors do not surface the live scan; treat an unconvertible
        // (undefined) print as "no scan surfaced" rather than a failure.
        Ok(VerifyOutcome::new(
            matched,
            print::fp_to_core(&scanned).ok(),
        ))
    }

    async fn identify_with_status<F: FnMut(FingerStatus)>(
        &mut self,
        gallery: &[Print],
        mut on_status: F,
    ) -> Result<IdentifyOutcome> {
        let fps = gallery
            .iter()
            .map(print::core_to_fp_for_match)
            .collect::<Result<Vec<FpPrint>>>()?;
        let mut scanned = FpPrint::new(&self.dev);
        self.cancel = Some(Cancellable::new());

        let mut trampoline = MatchTrampoline { cb: &mut on_status };
        let user_data: *mut c_void = (&mut trampoline as *mut MatchTrampoline<'_, F>).cast();

        let result = self.dev.identify_sync::<*mut c_void>(
            &fps,
            self.cancel.as_ref(),
            Some(on_match_status::<F> as FpMatchCb<*mut c_void>),
            Some(user_data),
            Some(&mut scanned),
        );

        self.cancel = None;
        let matched = result.map_err(convert::from_gerror)?;

        // Recover the gallery index by comparing the matched print's serialization to each
        // candidate's — the binding hands back the matching `FpPrint`, not its position.
        let match_index = match matched {
            Some(m) => {
                let needle = m.serialize().map_err(convert::from_gerror)?;
                fps.iter()
                    .position(|p| p.serialize().ok().as_deref() == Some(needle.as_slice()))
            }
            None => None,
        };

        Ok(IdentifyOutcome::new(
            match_index,
            print::fp_to_core(&scanned).ok(),
        ))
    }

    async fn list_prints(&mut self) -> Result<Vec<Print>> {
        if !self.has_feature(DeviceFeature::STORAGE_LIST) {
            return Err(Error::NotSupported);
        }
        storage::list(&self.dev, self.cancel.as_ref())?
            .iter()
            .map(print::fp_to_core)
            .collect()
    }

    async fn delete_print(&mut self, stored: &Print) -> Result<()> {
        if !self.has_feature(DeviceFeature::STORAGE_DELETE) {
            return Err(Error::NotSupported);
        }
        let fp = print::core_to_fp_for_match(stored)?;
        storage::delete(&self.dev, &fp, self.cancel.as_ref())
    }

    async fn clear_storage(&mut self) -> Result<()> {
        if !self.has_feature(DeviceFeature::STORAGE_CLEAR) {
            return Err(Error::NotSupported);
        }
        storage::clear(&self.dev, self.cancel.as_ref())
    }

    async fn suspend(&mut self) -> Result<()> {
        self.dev.suspend_sync(None).map_err(convert::from_gerror)
    }

    async fn resume(&mut self) -> Result<()> {
        self.dev.resume_sync(None).map_err(convert::from_gerror)
    }
}
