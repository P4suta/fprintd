// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The core seam: [`Backend`] and [`Device`] traits.
//!
//! ## Dispatch model
//!
//! These traits use **native `async fn` in trait** and **static dispatch**. `fprint-core`
//! introduces neither `dyn` nor an enum of backends: either would make the core depend on
//! its implementors and invert the dependency arrows. Runtime backend heterogeneity (e.g.
//! one device served by native Rust, another by the libfprint shim) is expressed *above*
//! the core by a `CompositeBackend` whose `Device` associated type is an enum over the
//! concrete backends — the only crate allowed to know all of them. See `ARCHITECTURE.md`.
//!
//! Cancellation is Rust-native: **dropping the returned future cancels the operation**.
//! Backends release the sensor in their own `Drop`; there is no `GCancellable`-style token.
//!
//! Every operation takes `&mut self`, so the borrow checker forbids concurrent
//! enroll/verify on one device — libfprint's single-in-flight-operation contract enforced
//! by the type system rather than runtime checks.
//!
//! The pair of doctests below is the proof, and **only the pair proves anything**: a lone
//! `compile_fail` passes for any reason at all, including a typo. These two differ in exactly one
//! thing — whether the two futures are alive at once — so the failure is attributable to the
//! overlap. Both are generic over `D: Device`, so the borrow checker rejects at the trait bound
//! and no implementor is needed.
//!
//! The `E0499` on the negative states the intended cause; stable rustdoc does not verify it, so
//! attribution rests on the control, not on the code.
//!
//! The positive control — the two operations in sequence compile:
//! ```
//! async fn sequential<D: fprint_core::Device>(dev: &mut D, p: &fprint_core::Print) {
//!     let _ = dev.enroll(p.clone(), |_| {}).await;
//!     let _ = dev.verify(p).await;
//! }
//! ```
//!
//! The negative — the same two operations overlapping do not:
//! ```compile_fail,E0499
//! fn concurrent<D: fprint_core::Device>(dev: &mut D, p: &fprint_core::Print) {
//!     let enrolling = dev.enroll(p.clone(), |_| {});
//!     let verifying = dev.verify(p);
//!     let _ = (enrolling, verifying);
//! }
//! ```

use crate::error::RetryReason;
use crate::{DeviceFeature, Error, FingerStatus, Print, Result, ScanType, Temperature};

/// Stable identifier for a physical reader (opaque; assigned by the backend).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct DeviceId(String);

impl DeviceId {
    /// Wrap a backend-assigned identifier.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Display for DeviceId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for DeviceId {
    fn from(id: &str) -> Self {
        Self::new(id)
    }
}

impl From<String> for DeviceId {
    fn from(id: String) -> Self {
        Self::new(id)
    }
}

/// Identifier of the driver a template is bound to (e.g. `"goodixmoc"`). Templates are
/// driver-specific, so this is its own type rather than a bare `String`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct DriverId(String);

impl DriverId {
    /// Wrap a driver identifier.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Display for DriverId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for DriverId {
    fn from(id: &str) -> Self {
        Self::new(id)
    }
}

impl From<String> for DriverId {
    fn from(id: String) -> Self {
        Self::new(id)
    }
}

/// Static description of a device, known once it is discovered.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct DeviceInfo {
    /// Backend-assigned identity of this reader.
    pub id: DeviceId,
    /// Driver id, which templates are bound to.
    pub driver: DriverId,
    /// Human-readable model name.
    pub name: String,
    /// How the finger is presented to this sensor.
    pub scan_type: ScanType,
    /// Everything this device advertises it can do; query it via [`Device::has_feature`].
    pub features: DeviceFeature,
    /// Number of finger presentations a full enrollment needs.
    pub enroll_stages: u32,
    /// Sensor thermal state, when the device reports it; `None` when it is not reported.
    pub temperature: Option<Temperature>,
}

impl DeviceInfo {
    /// Describe a device with no reported thermal state (`temperature` is `None`).
    #[must_use]
    pub fn new(
        id: DeviceId,
        driver: DriverId,
        name: impl Into<String>,
        scan_type: ScanType,
        features: DeviceFeature,
        enroll_stages: u32,
    ) -> Self {
        DeviceInfo {
            id,
            driver,
            name: name.into(),
            scan_type,
            features,
            enroll_stages,
            temperature: None,
        }
    }

    /// Set the reported thermal state.
    #[must_use]
    pub fn with_temperature(mut self, t: Temperature) -> Self {
        self.temperature = Some(t);
        self
    }
}

/// Progress report delivered during [`Device::enroll`], once per capture attempt.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct EnrollProgress {
    /// Stages completed so far (`0..=total_stages`).
    pub completed_stages: u32,
    /// Stages a full enrollment needs, mirroring [`DeviceInfo::enroll_stages`].
    pub total_stages: u32,
    /// `Some` when this capture failed and the user should present the finger again;
    /// the stage count did not advance.
    pub retry: Option<RetryReason>,
    /// Live finger-presence status accompanying this report ([`FingerStatus::NONE`] when the
    /// backend reports none).
    pub finger_status: FingerStatus,
}

impl EnrollProgress {
    /// A report of `completed_stages` of `total_stages`, with no retry and no finger status.
    #[must_use]
    pub fn new(completed_stages: u32, total_stages: u32) -> Self {
        EnrollProgress {
            completed_stages,
            total_stages,
            retry: None,
            finger_status: FingerStatus::NONE,
        }
    }

    /// Mark this report as a retry with reason `r`.
    #[must_use]
    pub fn with_retry(mut self, r: RetryReason) -> Self {
        self.retry = Some(r);
        self
    }

    /// Attach the live finger-presence status.
    #[must_use]
    pub fn with_finger_status(mut self, s: FingerStatus) -> Self {
        self.finger_status = s;
        self
    }
}

/// Result of a 1:1 [`Device::verify`].
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct VerifyOutcome {
    /// Whether the scan matched the enrolled print.
    pub matched: bool,
    /// The freshly scanned print, when the backend surfaces it (host-side sensors do; many
    /// match-on-chip sensors do not).
    pub scanned: Option<Print>,
}

impl VerifyOutcome {
    /// A verify result: `matched` and the optional freshly `scanned` print.
    #[must_use]
    pub fn new(matched: bool, scanned: Option<Print>) -> Self {
        VerifyOutcome { matched, scanned }
    }
}

/// Result of a 1:N [`Device::identify`].
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct IdentifyOutcome {
    /// Index into the gallery passed to `identify`, or `None` for no match.
    pub match_index: Option<usize>,
    /// The freshly scanned print, on the same terms as [`VerifyOutcome::scanned`].
    pub scanned: Option<Print>,
}

impl IdentifyOutcome {
    /// An identify result: the gallery `match_index` (or `None`) and the optional freshly
    /// `scanned` print.
    #[must_use]
    pub fn new(match_index: Option<usize>, scanned: Option<Print>) -> Self {
        IdentifyOutcome {
            match_index,
            scanned,
        }
    }
}

/// A physical fingerprint reader.
///
/// Implemented by each backend (the libfprint shim, native Rust drivers). Consumers hold
/// a concrete `Device` (static dispatch); the fprintd-compatible daemon is generic over
/// its [`Backend`].
///
/// `async fn` in a public trait is intentional (see the module docs), so the
/// `async_fn_in_trait` lint — which warns that callers cannot add `+ Send` bounds — is
/// expected here and only here.
#[expect(async_fn_in_trait)]
pub trait Device {
    /// Static metadata (available before and after `open`).
    fn info(&self) -> &DeviceInfo;

    /// Convenience: does this device advertise `feature`?
    fn has_feature(&self, feature: DeviceFeature) -> bool {
        self.info().features.contains(feature)
    }

    /// Open the device for use. Must be called before any operation below.
    async fn open(&mut self) -> Result<()>;

    /// Close the device, releasing the sensor.
    async fn close(&mut self) -> Result<()>;

    /// Enroll `finger` into `template` (a [`Print::new_for_enroll`]), reporting each
    /// capture via `on_progress`. Returns the completed print.
    ///
    /// `on_progress` is a generic `FnMut`, not a trait object: progress is pushed to the
    /// caller, and static dispatch keeps the core free of `dyn`.
    async fn enroll<F: FnMut(EnrollProgress)>(
        &mut self,
        template: Print,
        on_progress: F,
    ) -> Result<Print>;

    /// Verify a single scan against one `enrolled` print (1:1), reporting live finger-presence
    /// through `on_status` as the sensor sees a finger arrive and leave.
    ///
    /// `on_status` is the verify-side counterpart of [`enroll`](Device::enroll)'s progress callback:
    /// static dispatch, pushed to the caller, so the core stays `dyn`-free. A backend with no live
    /// finger reporting simply never calls it.
    async fn verify_with_status<F: FnMut(FingerStatus)>(
        &mut self,
        enrolled: &Print,
        on_status: F,
    ) -> Result<VerifyOutcome>;

    /// Verify a single scan against one `enrolled` print (1:1).
    ///
    /// The status-free convenience form of [`verify_with_status`](Device::verify_with_status), for
    /// callers that do not drive finger-presence UI prompts.
    async fn verify(&mut self, enrolled: &Print) -> Result<VerifyOutcome> {
        self.verify_with_status(enrolled, |_| {}).await
    }

    /// Identify a single scan against a `gallery` of prints (1:N), reporting live finger-presence
    /// through `on_status`.
    async fn identify_with_status<F: FnMut(FingerStatus)>(
        &mut self,
        gallery: &[Print],
        on_status: F,
    ) -> Result<IdentifyOutcome>;

    /// Identify a single scan against a `gallery` of prints (1:N).
    ///
    /// The status-free convenience form of [`identify_with_status`](Device::identify_with_status).
    async fn identify(&mut self, gallery: &[Print]) -> Result<IdentifyOutcome> {
        self.identify_with_status(gallery, |_| {}).await
    }

    /// List prints stored on the device (match-on-chip devices with `STORAGE`).
    ///
    /// A device that keeps no on-sensor storage (a host-image sensor) inherits the default, which
    /// reports [`Error::NotSupported`] — the absence of storage is spoken by *not* overriding this,
    /// its presence by advertising [`DeviceFeature::STORAGE`] and overriding it.
    async fn list_prints(&mut self) -> Result<Vec<Print>> {
        Err(Error::NotSupported)
    }

    /// Delete one print from on-device storage.
    ///
    /// Defaults to [`Error::NotSupported`] for a device without on-sensor storage; see
    /// [`list_prints`](Device::list_prints).
    async fn delete_print(&mut self, print: &Print) -> Result<()> {
        let _ = print;
        Err(Error::NotSupported)
    }

    /// Erase all templates from on-device storage.
    ///
    /// Defaults to [`Error::NotSupported`] for a device without on-sensor storage; see
    /// [`list_prints`](Device::list_prints).
    async fn clear_storage(&mut self) -> Result<()> {
        Err(Error::NotSupported)
    }

    /// Prepare for system suspend (the sensor may need to stop an active wait).
    async fn suspend(&mut self) -> Result<()>;

    /// Resume after system suspend.
    async fn resume(&mut self) -> Result<()>;
}

/// A source of [`Device`]s — the entry point, analogous to libfprint's `FpContext`.
///
/// The associated `Device` type keeps this on static dispatch.
#[expect(async_fn_in_trait)]
pub trait Backend {
    /// The concrete reader type this backend hands out.
    type Device: Device;

    /// Enumerate the readers currently attached.
    async fn enumerate(&self) -> Result<Vec<Self::Device>>;

    /// Open a specific reader by id (convenience over `enumerate` + find).
    async fn open(&self, id: &DeviceId) -> Result<Self::Device>;
}

#[cfg(test)]
mod tests {
    use super::{Device, DeviceId, DeviceInfo, DriverId, EnrollProgress, IdentifyOutcome};
    use crate::feature::FLAGS;
    use crate::{DeviceFeature, FingerStatus, Print, Result, ScanType, VerifyOutcome};

    /// Carries a [`DeviceInfo`] and nothing else. Only `info` is ever called: [`Device::has_feature`]
    /// is a default method over it, so the sensor operations need no bodies to test it.
    struct InfoOnly(DeviceInfo);

    impl Device for InfoOnly {
        fn info(&self) -> &DeviceInfo {
            &self.0
        }

        async fn open(&mut self) -> Result<()> {
            todo!("has_feature never opens the device")
        }
        async fn close(&mut self) -> Result<()> {
            todo!("has_feature never opens the device")
        }
        async fn enroll<F: FnMut(EnrollProgress)>(&mut self, _: Print, _: F) -> Result<Print> {
            todo!("has_feature never scans")
        }
        async fn verify_with_status<F: FnMut(FingerStatus)>(
            &mut self,
            _: &Print,
            _: F,
        ) -> Result<VerifyOutcome> {
            todo!("has_feature never scans")
        }
        async fn identify_with_status<F: FnMut(FingerStatus)>(
            &mut self,
            _: &[Print],
            _: F,
        ) -> Result<IdentifyOutcome> {
            todo!("has_feature never scans")
        }
        async fn suspend(&mut self) -> Result<()> {
            todo!("has_feature never suspends")
        }
        async fn resume(&mut self) -> Result<()> {
            todo!("has_feature never suspends")
        }
    }

    fn device_with(features: DeviceFeature) -> InfoOnly {
        InfoOnly(DeviceInfo::new(
            DeviceId::new("test-0"),
            DriverId::new("test"),
            "Test Reader",
            ScanType::Press,
            features,
            1,
        ))
    }

    /// **`has_feature` is `info().features.contains`, for every representable set and every defined
    /// flag** — the default body must not develop a rule of its own.
    ///
    /// The sets and the flags both come from [`crate::feature::FLAGS`], so an eleventh flag widens
    /// this test on its own rather than escaping it.
    #[test]
    fn has_feature_agrees_with_the_advertised_set() {
        let defined = DeviceFeature::from_bits_truncate(u32::MAX).bits();
        for bits in 0..=u32::from(defined) {
            let advertised = DeviceFeature::from_bits_truncate(bits);
            let device = device_with(advertised);
            for flag in
                core::iter::once(DeviceFeature::NONE).chain(FLAGS.into_iter().map(|(f, _)| f))
            {
                assert_eq!(
                    device.has_feature(flag),
                    advertised.contains(flag),
                    "{advertised:?}.has_feature({flag:?})"
                );
            }
        }
    }
}
