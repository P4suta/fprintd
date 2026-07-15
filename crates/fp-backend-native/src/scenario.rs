// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Scripted behaviour for the virtual device.
//!
//! A real sensor's output depends on the finger physically present and on capture quality;
//! a *virtual* one has to be *told* what to pretend happened. A [`Scenario`] is that script:
//! which finger is currently "on the sensor" ([`Scenario::present`]), how the next
//! enrollment plays out ([`EnrollScript`]), and whether on-device storage should report
//! itself full ([`Scenario::storage_full`]).
//!
//! Everything here is plain data with consuming builders, so a test reads like a sentence:
//!
//! ```
//! use fp_backend_native::{Scenario, EnrollScript, FingerId};
//! use fp_core::RetryReason;
//! let scenario = Scenario::new()
//!     .present(FingerId(7))
//!     .enroll(EnrollScript::default().produces(FingerId(7)).retry(RetryReason::NotCentered).advance());
//! ```

use fp_core::{RetryReason, Template};

/// Opaque identity of a finger, for tests.
///
/// Two presentations with the same `FingerId` produce byte-identical templates and thus
/// match; different ids never match. It is **not** a biometric — see `crate::synth`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FingerId(pub u64);

/// What a single capture attempt during enrollment does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CaptureOutcome {
    /// A clean capture: the enrollment progresses by one stage.
    Advance,
    /// A poor capture with a reason: the user must present the finger again; the stage count
    /// does not move. The [`RetryReason`] is forwarded verbatim in
    /// [`EnrollProgress::retry`](fp_core::EnrollProgress::retry) during
    /// [`VirtualDevice`](crate::VirtualDevice) enroll, so the daemon can render the matching status string.
    Retry(RetryReason),
}

/// The scripted arc of one enrollment: a sequence of capture outcomes and the identity the
/// finished template encodes.
///
/// The `Default` (empty) script is a device that simply completes every stage cleanly.
/// When the script runs out of steps mid-enrollment, remaining stages default to
/// [`CaptureOutcome::Advance`], so `EnrollScript::default()` enrolls any `nr_enroll_stages`.
#[derive(Clone, Debug, Default)]
#[must_use]
pub struct EnrollScript {
    pub(crate) steps: Vec<CaptureOutcome>,
    pub(crate) produces: Option<FingerId>,
}

impl EnrollScript {
    /// Set the finger identity the resulting template encodes (defaults to `FingerId(0)`).
    pub fn produces(mut self, id: FingerId) -> Self {
        self.produces = Some(id);
        self
    }

    /// Append one clean capture (advances a stage).
    pub fn advance(mut self) -> Self {
        self.steps.push(CaptureOutcome::Advance);
        self
    }

    /// Append one failed capture (a retry with `reason`; the stage does not advance).
    pub fn retry(mut self, reason: RetryReason) -> Self {
        self.steps.push(CaptureOutcome::Retry(reason));
        self
    }
}

/// The full script handed to a [`crate::VirtualDeviceBuilder`].
#[derive(Clone, Debug, Default)]
#[must_use]
pub struct Scenario {
    pub(crate) enroll: EnrollScript,
    pub(crate) presented: Option<FingerId>,
    pub(crate) force_data_full: bool,
    /// Real minutiae the *host-image* path enrolls (overrides the synthetic `crate::synth`
    /// template). Set with [`Scenario::enroll_real`] to drive genuine BOZORTH3 matching.
    pub(crate) enroll_template: Option<Template>,
    /// Real minutiae presented as the live scan for verify/identify (a distinct capture from the
    /// enrolled one). Set with [`Scenario::present_real`].
    pub(crate) presented_template: Option<Template>,
}

impl Scenario {
    /// An empty scenario: no finger present, enrollment completes cleanly, storage not full.
    pub fn new() -> Self {
        Self::default()
    }

    /// Script the next enrollment.
    pub fn enroll(mut self, script: EnrollScript) -> Self {
        self.enroll = script;
        self
    }

    /// Put a finger "on the sensor" for the next verify/identify.
    pub fn present(mut self, id: FingerId) -> Self {
        self.presented = Some(id);
        self
    }

    /// Force the next enrollment to report on-device storage as full ([`fp_core::Error::DataFull`]).
    pub fn storage_full(mut self) -> Self {
        self.force_data_full = true;
        self
    }

    /// Enroll this **real** template (host-image minutiae) instead of the synthetic stub, so a
    /// device built with [`crate::VirtualDeviceBuilder::bozorth3_matching`] exercises genuine
    /// matching. Pair with [`Scenario::present_real`] for a distinct verify capture.
    pub fn enroll_real(mut self, template: Template) -> Self {
        self.enroll_template = Some(template);
        self
    }

    /// Present this **real** template as the live scan for the next verify/identify.
    pub fn present_real(mut self, template: Template) -> Self {
        self.presented_template = Some(template);
        self
    }
}
