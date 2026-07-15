// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The per-device actor thread.
//!
//! The libfprint shim is thread-affine (GObject / `GMainContext`), so `fprint_core::Device` is
//! not required to be `Send` (ARCHITECTURE.md principle 7). Each device therefore gets one
//! dedicated OS thread running a single-threaded tokio runtime + `LocalSet` that owns the
//! device for its whole life. Commands arrive over a `Send` channel ([`DeviceCommand`]); the
//! actor runs the matching `fprint_core` async method on its own thread and answers via the
//! command's reply channel. The rest of the daemon holds only a [`DeviceHandle`], which is
//! `Send`.
//!
//! The daemon holds a `Send` factory (`Fn() -> B`); each actor thread calls it to build its own
//! backend locally, so `B` — and `B::Device` — may be `!Send` without any unsound `Send` impl.
//! The device is then opened lazily via `Backend::open` on that same thread on the first
//! [`DeviceCommand::Open`] (i.e. the first `Claim`), so the actor thread, not the daemon's
//! runtime, is the only one that ever touches the reader.

use fprint_core::{Backend, Device, DeviceId, DeviceInfo, EnrollProgress, Error};
use tokio::sync::mpsc;

use crate::command::DeviceCommand;
use crate::error::DaemonError;

/// A `Send` handle to a device actor: just the command channel. The static [`DeviceInfo`]
/// lives on the D-Bus [`Device`](crate::device::Device) object directly, so it is not
/// duplicated here.
#[derive(Clone)]
pub struct DeviceHandle {
    tx: mpsc::Sender<DeviceCommand>,
}

impl DeviceHandle {
    /// Send a command to the actor. Fails only if the actor thread has stopped.
    pub async fn send(&self, cmd: DeviceCommand) -> Result<(), DaemonError> {
        self.tx
            .send(cmd)
            .await
            .map_err(|_| DaemonError::Internal("device actor stopped".into()))
    }
}

/// Spawns and owns device actor threads.
pub struct DeviceActor;

impl DeviceActor {
    /// Spawn a dedicated thread for the device identified by `info`, returning a `Send`
    /// handle to it. The `factory` (which is `Send`) is moved onto the new thread and called
    /// *there* to build a thread-local backend, so the possibly `!Send` backend and device
    /// never leave this thread.
    pub fn spawn<F, B>(factory: F, info: DeviceInfo) -> DeviceHandle
    where
        F: FnOnce() -> B + Send + 'static,
        B: Backend,
        B::Device: 'static,
    {
        let (tx, rx) = mpsc::channel(16);
        let id = info.id.clone();
        std::thread::Builder::new()
            .name(format!("fprintd-dev-{}", id.0))
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("build device actor runtime");
                let local = tokio::task::LocalSet::new();
                local.block_on(&rt, async move {
                    let backend = factory();
                    run_actor(backend, id, rx).await;
                });
            })
            .expect("spawn device actor thread");
        DeviceHandle { tx }
    }
}

/// The actor loop: owns the thread-local `backend` and an `Option<B::Device>` (opened on
/// first `Open`), servicing commands until every handle is dropped.
async fn run_actor<B>(backend: B, id: DeviceId, mut rx: mpsc::Receiver<DeviceCommand>)
where
    B: Backend,
    B::Device: 'static,
{
    let mut dev: Option<B::Device> = None;

    while let Some(cmd) = rx.recv().await {
        match cmd {
            DeviceCommand::Open { reply } => {
                let res = open(&backend, &id, &mut dev).await;
                let _ = reply.send(res);
            }
            DeviceCommand::Close { reply } => {
                let res = match dev.take() {
                    Some(mut d) => d.close().await,
                    None => Ok(()),
                };
                let _ = reply.send(res);
            }
            DeviceCommand::Enroll {
                finger,
                template,
                progress,
                cancel,
                reply,
            } => {
                tracing::debug!(?finger, "enroll starting");
                let res = match dev.as_mut() {
                    Some(d) => {
                        let mut on_progress = |p: EnrollProgress| {
                            let _ = progress.try_send(p);
                        };
                        tokio::select! {
                            r = d.enroll(template, &mut on_progress) => r,
                            _ = cancel => Err(Error::Cancelled),
                        }
                    }
                    None => Err(Error::ProtoState),
                };
                let _ = reply.send(res);
            }
            DeviceCommand::Verify {
                enrolled,
                cancel,
                reply,
            } => {
                let res = match dev.as_mut() {
                    Some(d) => tokio::select! {
                        r = d.verify(&enrolled) => r,
                        _ = cancel => Err(Error::Cancelled),
                    },
                    None => Err(Error::ProtoState),
                };
                let _ = reply.send(res);
            }
            DeviceCommand::Identify {
                gallery,
                cancel,
                reply,
            } => {
                let res = match dev.as_mut() {
                    Some(d) => tokio::select! {
                        r = d.identify(&gallery) => r,
                        _ = cancel => Err(Error::Cancelled),
                    },
                    None => Err(Error::ProtoState),
                };
                let _ = reply.send(res);
            }
        }
    }
}

/// Acquire the reader if needed, open the sensor, and return its `DeviceInfo`.
///
/// The info is read after `open`, not before: see [`DeviceCommand::Open`].
async fn open<B>(
    backend: &B,
    id: &DeviceId,
    slot: &mut Option<B::Device>,
) -> Result<DeviceInfo, Error>
where
    B: Backend,
    B::Device: 'static,
{
    if slot.is_none() {
        *slot = Some(backend.open(id).await?);
    }
    let d = slot.as_mut().ok_or(Error::ProtoState)?;
    d.open().await?;
    Ok(d.info().clone())
}
