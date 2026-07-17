// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The message protocol between the async D-Bus objects and a device's actor thread.
//!
//! Each [`DeviceCommand`] carries its own reply channel (a `oneshot`), and the streaming
//! operations additionally carry a progress channel and a cancellation receiver. Every
//! payload is `Send`, so the command can cross from the tokio runtime hosting the D-Bus
//! objects to the dedicated OS thread that owns the (possibly `!Send`) device — see
//! [`crate::actor`].
//!
//! Cancellation is ARCHITECTURE.md principle 4: the actor `select!`s the operation future
//! against `cancel`, and dropping the future cancels it. The sender drops `cancel`'s
//! counterpart (or sends `()`), and the actor stops.

use fprint_core::{
    DeviceInfo, EnrollProgress, Error, FingerStatus, IdentifyOutcome, Print, VerifyOutcome,
};
use tokio::sync::{mpsc, oneshot};

/// A unit of work handed to a device actor. The `reply` oneshot delivers the outcome.
pub enum DeviceCommand {
    /// Acquire the reader (on first use) and open the sensor.
    ///
    /// Replies with the [`DeviceInfo`] as it stands once open. A backend may only learn a
    /// reader's scan type, features and enroll-stage count from its open path, so what
    /// `Backend::enumerate` reported is a class default; the libfprint shim re-reads its
    /// `DeviceInfo` in `Device::open` for this reason.
    Open {
        reply: oneshot::Sender<Result<DeviceInfo, Error>>,
    },
    /// Close the sensor and release the reader.
    Close {
        reply: oneshot::Sender<Result<(), Error>>,
    },
    /// Enroll `finger` from `template`, streaming each capture over `progress`.
    Enroll {
        finger: fprint_core::Finger,
        template: Print,
        progress: mpsc::Sender<EnrollProgress>,
        cancel: oneshot::Receiver<()>,
        reply: oneshot::Sender<Result<Print, Error>>,
    },
    /// Verify a single scan against one `enrolled` print (1:1), streaming live finger-presence
    /// over `status`.
    Verify {
        enrolled: Print,
        status: mpsc::Sender<FingerStatus>,
        cancel: oneshot::Receiver<()>,
        reply: oneshot::Sender<Result<VerifyOutcome, Error>>,
    },
    /// Identify a single scan against a `gallery` of prints (1:N), streaming live finger-presence
    /// over `status`.
    Identify {
        gallery: Vec<Print>,
        status: mpsc::Sender<FingerStatus>,
        cancel: oneshot::Receiver<()>,
        reply: oneshot::Sender<Result<IdentifyOutcome, Error>>,
    },
    /// Enumerate the prints held in the reader's on-device storage.
    ListPrints {
        reply: oneshot::Sender<Result<Vec<Print>, Error>>,
    },
    /// Delete one `print` from the reader's on-device storage.
    DeletePrint {
        print: Print,
        reply: oneshot::Sender<Result<(), Error>>,
    },
    /// Wipe the reader's on-device storage.
    ClearStorage {
        reply: oneshot::Sender<Result<(), Error>>,
    },
}

/// A **priority** command, delivered on a channel separate from [`DeviceCommand`].
///
/// System suspend must preempt an in-flight streaming op, not queue behind it: the actor services
/// one [`DeviceCommand`] at a time, so a verify/enroll parked on a finger would hold the actor
/// until the user acts, and a `Suspend` sitting behind it in the FIFO would miss logind's inhibitor
/// deadline. Instead the actor `select!`s this channel both in its idle loop and inside every
/// streaming op, so a `Suspend` arriving mid-op cancels that op (dropping its future) and then runs.
/// Delivering it out-of-band — consumed exactly once by whichever `select!` reaches it — is why no
/// preempt can be lost or misapplied to a later op.
pub enum PrioCommand {
    /// Prepare the reader for system suspend.
    Suspend {
        reply: oneshot::Sender<Result<(), Error>>,
    },
    /// Resume the reader after system suspend.
    Resume {
        reply: oneshot::Sender<Result<(), Error>>,
    },
}
