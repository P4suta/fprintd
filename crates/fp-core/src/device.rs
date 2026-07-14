// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The core seam: [`Backend`] and [`Device`] traits.
//!
//! ## Dispatch model (an architectural decision — see `ARCHITECTURE.md`)
//!
//! These traits use **native `async fn` in trait** and **static dispatch**. `fp-core`
//! deliberately introduces neither `dyn` nor an enum of backends: doing so would make the
//! core depend on its implementors and invert the dependency arrows. Runtime backend
//! heterogeneity (e.g. one device served by native Rust, another by the libfprint shim,
//! during migration) is expressed *above* the core by a `CompositeBackend` whose
//! `Device` associated type is an enum over the concrete backends — the only crate that
//! is allowed to know all of them. The core stays a pure crystal.
//!
//! Cancellation is Rust-native: **dropping the returned future cancels the operation**.
//! Backends release the sensor in their own `Drop`; there is no `GCancellable`-style token.
//!
//! One operation at a time is enforced by the type system: every operation takes
//! `&mut self`, so the borrow checker forbids concurrent enroll/verify on one device —
//! mirroring libfprint's single-in-flight-operation contract without runtime checks.

use crate::error::RetryReason;
use crate::{DeviceFeature, Print, Result, ScanType};

/// Stable identifier for a physical reader (opaque; assigned by the backend).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct DeviceId(pub String);

/// Identifier of the driver a template is bound to (e.g. `"goodixmoc"`). Templates are
/// driver-specific, so this is its own type rather than a bare `String`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct DriverId(pub String);

/// Static description of a device, known once it is discovered.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DeviceInfo {
    pub id: DeviceId,
    /// Driver id, which templates are bound to.
    pub driver: DriverId,
    /// Human-readable model name.
    pub name: String,
    pub scan_type: ScanType,
    pub features: DeviceFeature,
    /// Number of finger presentations a full enrollment needs.
    pub enroll_stages: u32,
}

/// Progress report delivered during [`Device::enroll`], once per capture attempt.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EnrollProgress {
    /// Stages completed so far (`0..=total_stages`).
    pub completed_stages: u32,
    pub total_stages: u32,
    /// `Some` when this capture failed and the user should present the finger again;
    /// the stage count did not advance.
    pub retry: Option<RetryReason>,
}

/// Result of a 1:1 [`Device::verify`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct VerifyOutcome {
    pub matched: bool,
    /// The freshly scanned print, when the backend surfaces it (host-side sensors do; many
    /// match-on-chip sensors do not).
    pub scanned: Option<Print>,
}

/// Result of a 1:N [`Device::identify`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct IdentifyOutcome {
    /// Index into the gallery passed to `identify`, or `None` for no match.
    pub match_index: Option<usize>,
    pub scanned: Option<Print>,
}

/// A physical fingerprint reader.
///
/// Implemented by each backend (the libfprint shim, native Rust drivers). Consumers hold
/// a concrete `Device` (static dispatch); the fprintd-compatible daemon is generic over
/// its [`Backend`].
///
/// `async fn` in a public trait is a deliberate architectural choice (native AFIT, static
/// dispatch — see the module docs and `ARCHITECTURE.md`), so the `async_fn_in_trait` lint
/// (which warns that callers cannot add `+ Send` bounds) is allowed here and only here.
#[allow(async_fn_in_trait)]
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
    /// caller (the right model for a blocking, thread-affine backend like the libfprint shim),
    /// and static dispatch keeps the core free of `dyn`. The method is therefore not
    /// object-safe — as with every `async fn` here — which is fine: dispatch is static.
    async fn enroll<F: FnMut(EnrollProgress)>(
        &mut self,
        template: Print,
        on_progress: F,
    ) -> Result<Print>;

    /// Verify a single scan against one `enrolled` print (1:1).
    async fn verify(&mut self, enrolled: &Print) -> Result<VerifyOutcome>;

    /// Identify a single scan against a `gallery` of prints (1:N).
    async fn identify(&mut self, gallery: &[Print]) -> Result<IdentifyOutcome>;

    /// List prints stored on the device (match-on-chip devices with `STORAGE`).
    async fn list_prints(&mut self) -> Result<Vec<Print>>;

    /// Delete one print from on-device storage.
    async fn delete_print(&mut self, print: &Print) -> Result<()>;

    /// Erase all templates from on-device storage.
    async fn clear_storage(&mut self) -> Result<()>;

    /// Prepare for system suspend (the sensor may need to stop an active wait).
    async fn suspend(&mut self) -> Result<()>;

    /// Resume after system suspend.
    async fn resume(&mut self) -> Result<()>;
}

/// A source of [`Device`]s — the entry point, analogous to libfprint's `FpContext`.
///
/// The associated `Device` type keeps this on static dispatch. Hotplug (device
/// added/removed streams) will be layered on once the transport crates exist.
#[allow(async_fn_in_trait)]
pub trait Backend {
    type Device: Device;

    /// Enumerate the readers currently attached.
    async fn enumerate(&self) -> Result<Vec<Self::Device>>;

    /// Open a specific reader by id (convenience over `enumerate` + find).
    async fn open(&self, id: &DeviceId) -> Result<Self::Device>;
}
