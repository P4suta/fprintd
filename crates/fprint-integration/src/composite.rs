// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [`CompositeBackend`] / [`CompositeDevice`]: the heterogeneity seam.
//!
//! One [`fprint_core::Backend`] whose devices come from either the pure-Rust native backend or
//! the Linux libfprint shim. The [`Device`] impl on [`CompositeDevice`] delegates every
//! method with an explicit `match`; see the crate docs for why, rather than `enum_dispatch`.

use fprint_core::{
    Backend, Device, DeviceId, DeviceInfo, EnrollProgress, FingerStatus, IdentifyOutcome, Print,
    Result, Temperature, VerifyOutcome,
};

use fprint_backend_native::{VirtualBackend, VirtualDevice};

#[cfg(target_os = "linux")]
use fprint_backend_libfprint::{LibfprintBackend, LibfprintDevice};

/// A device served by one of the composed backends.
///
/// The `Shim` variant exists only on Linux, where the libfprint FFI backend is available; on
/// every other host a `CompositeDevice` is always `Native`. `has_feature` is *not* delegated:
/// it inherits [`Device`]'s default, which is expressed in terms of [`Device::info`], so it
/// already works through the enum.
pub enum CompositeDevice {
    /// A device from the pure-Rust native backend.
    Native(VirtualDevice),
    /// A device from the libfprint shim (Linux only).
    #[cfg(target_os = "linux")]
    Shim(LibfprintDevice),
}

impl Device for CompositeDevice {
    fn info(&self) -> &DeviceInfo {
        match self {
            CompositeDevice::Native(d) => d.info(),
            #[cfg(target_os = "linux")]
            CompositeDevice::Shim(d) => d.info(),
        }
    }

    fn temperature(&self) -> Option<Temperature> {
        match self {
            CompositeDevice::Native(d) => d.temperature(),
            #[cfg(target_os = "linux")]
            CompositeDevice::Shim(d) => d.temperature(),
        }
    }

    async fn open(&mut self) -> Result<()> {
        match self {
            CompositeDevice::Native(d) => d.open().await,
            #[cfg(target_os = "linux")]
            CompositeDevice::Shim(d) => d.open().await,
        }
    }

    async fn close(&mut self) -> Result<()> {
        match self {
            CompositeDevice::Native(d) => d.close().await,
            #[cfg(target_os = "linux")]
            CompositeDevice::Shim(d) => d.close().await,
        }
    }

    async fn enroll<F: FnMut(EnrollProgress)>(
        &mut self,
        print: Print,
        on_progress: F,
    ) -> Result<Print> {
        match self {
            CompositeDevice::Native(d) => d.enroll(print, on_progress).await,
            #[cfg(target_os = "linux")]
            CompositeDevice::Shim(d) => d.enroll(print, on_progress).await,
        }
    }

    async fn verify_with_status<F: FnMut(FingerStatus)>(
        &mut self,
        enrolled: &Print,
        on_status: F,
    ) -> Result<VerifyOutcome> {
        match self {
            CompositeDevice::Native(d) => d.verify_with_status(enrolled, on_status).await,
            #[cfg(target_os = "linux")]
            CompositeDevice::Shim(d) => d.verify_with_status(enrolled, on_status).await,
        }
    }

    async fn identify_with_status<F: FnMut(FingerStatus)>(
        &mut self,
        gallery: &[Print],
        on_status: F,
    ) -> Result<IdentifyOutcome> {
        match self {
            CompositeDevice::Native(d) => d.identify_with_status(gallery, on_status).await,
            #[cfg(target_os = "linux")]
            CompositeDevice::Shim(d) => d.identify_with_status(gallery, on_status).await,
        }
    }

    async fn list_prints(&mut self) -> Result<Vec<Print>> {
        match self {
            CompositeDevice::Native(d) => d.list_prints().await,
            #[cfg(target_os = "linux")]
            CompositeDevice::Shim(d) => d.list_prints().await,
        }
    }

    async fn delete_print(&mut self, print: &Print) -> Result<()> {
        match self {
            CompositeDevice::Native(d) => d.delete_print(print).await,
            #[cfg(target_os = "linux")]
            CompositeDevice::Shim(d) => d.delete_print(print).await,
        }
    }

    async fn clear_storage(&mut self) -> Result<()> {
        match self {
            CompositeDevice::Native(d) => d.clear_storage().await,
            #[cfg(target_os = "linux")]
            CompositeDevice::Shim(d) => d.clear_storage().await,
        }
    }

    async fn suspend(&mut self) -> Result<()> {
        match self {
            CompositeDevice::Native(d) => d.suspend().await,
            #[cfg(target_os = "linux")]
            CompositeDevice::Shim(d) => d.suspend().await,
        }
    }

    async fn resume(&mut self) -> Result<()> {
        match self {
            CompositeDevice::Native(d) => d.resume().await,
            #[cfg(target_os = "linux")]
            CompositeDevice::Shim(d) => d.resume().await,
        }
    }
}

/// A [`Backend`] composing the native backend with (on Linux) the libfprint shim.
///
/// The shim is optional even where it exists: [`with_native`](CompositeBackend::with_native)
/// builds a native-only composite and is available on every host; `new` (Linux only) composes
/// both.
pub struct CompositeBackend {
    native: VirtualBackend,
    /// The shim, if one was supplied. `None` for a native-only composite; `Some` after
    /// [`new`](CompositeBackend::new).
    #[cfg(target_os = "linux")]
    shim: Option<LibfprintBackend>,
}

impl CompositeBackend {
    /// Compose the native backend with the libfprint shim (Linux only).
    #[cfg(target_os = "linux")]
    pub fn new(native: VirtualBackend, shim: LibfprintBackend) -> Self {
        CompositeBackend {
            native,
            shim: Some(shim),
        }
    }

    /// A native-only composite. Available on every host; on Linux it carries no shim.
    pub fn with_native(native: VirtualBackend) -> Self {
        CompositeBackend {
            native,
            #[cfg(target_os = "linux")]
            shim: None,
        }
    }
}

impl Backend for CompositeBackend {
    type Device = CompositeDevice;

    async fn enumerate(&self) -> Result<Vec<CompositeDevice>> {
        let native = self
            .native
            .enumerate()
            .await?
            .into_iter()
            .map(CompositeDevice::Native);

        #[cfg(not(target_os = "linux"))]
        {
            Ok(native.collect())
        }
        #[cfg(target_os = "linux")]
        {
            match &self.shim {
                Some(shim) => {
                    let shim = shim
                        .enumerate()
                        .await?
                        .into_iter()
                        .map(CompositeDevice::Shim);
                    Ok(native.chain(shim).collect())
                }
                None => Ok(native.collect()),
            }
        }
    }

    async fn open(&self, id: &DeviceId) -> Result<CompositeDevice> {
        match self.native.open(id).await {
            Ok(d) => Ok(CompositeDevice::Native(d)),
            // The native backend didn't recognise the id: on Linux, offer it to the shim.
            #[cfg(target_os = "linux")]
            Err(fprint_core::Error::NotFound) => match &self.shim {
                Some(shim) => shim.open(id).await.map(CompositeDevice::Shim),
                None => Err(fprint_core::Error::NotFound),
            },
            Err(e) => Err(e),
        }
    }
}
