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

use fprint_core::{Backend, Device, DeviceId, DeviceInfo, EnrollProgress, Error, FingerStatus};
use tokio::sync::{mpsc, oneshot};

use crate::command::{DeviceCommand, PrioCommand};
use crate::error::DaemonError;

/// A `Send` handle to a device actor: the command channel plus a priority channel. The static
/// [`DeviceInfo`] lives on the D-Bus [`Device`](crate::device::Device) object directly, so it is
/// not duplicated here.
#[derive(Clone)]
pub struct DeviceHandle {
    tx: mpsc::Sender<DeviceCommand>,
    /// Out-of-band channel for suspend/resume, so a suspend preempts an in-flight streaming op
    /// instead of queuing behind a verify/enroll parked on a finger (which would only end when the
    /// user acts, blowing past logind's inhibitor deadline). See [`PrioCommand`].
    prio: mpsc::Sender<PrioCommand>,
}

impl DeviceHandle {
    /// Send a command to the actor. Fails only if the actor thread has stopped.
    pub async fn send(&self, cmd: DeviceCommand) -> Result<(), DaemonError> {
        self.tx
            .send(cmd)
            .await
            .map_err(|_| DaemonError::Internal("device actor stopped".into()))
    }

    /// Prepare the reader for system suspend.
    ///
    /// Rides the priority channel so it preempts an in-flight verify/enroll parked on a finger
    /// rather than queuing behind it (which would block past logind's inhibitor deadline).
    pub async fn suspend(&self) -> Result<(), DaemonError> {
        let (reply, rx) = oneshot::channel();
        self.prio
            .send(PrioCommand::Suspend { reply })
            .await
            .map_err(|_| DaemonError::Internal("device actor stopped".into()))?;
        rx.await
            .map_err(|_| DaemonError::Internal("device actor stopped".into()))?
            .map_err(DaemonError::from)
    }

    /// Resume the reader after system suspend.
    pub async fn resume(&self) -> Result<(), DaemonError> {
        let (reply, rx) = oneshot::channel();
        self.prio
            .send(PrioCommand::Resume { reply })
            .await
            .map_err(|_| DaemonError::Internal("device actor stopped".into()))?;
        rx.await
            .map_err(|_| DaemonError::Internal("device actor stopped".into()))?
            .map_err(DaemonError::from)
    }

    /// Enumerate the prints held in the reader's on-device storage.
    pub async fn list_prints(&self) -> Result<Vec<fprint_core::Print>, DaemonError> {
        let (reply, rx) = oneshot::channel();
        self.send(DeviceCommand::ListPrints { reply }).await?;
        rx.await
            .map_err(|_| DaemonError::Internal("device actor stopped".into()))?
            .map_err(DaemonError::from)
    }

    /// Delete one `print` from the reader's on-device storage.
    pub async fn delete_device_print(&self, print: fprint_core::Print) -> Result<(), DaemonError> {
        let (reply, rx) = oneshot::channel();
        self.send(DeviceCommand::DeletePrint { print, reply })
            .await?;
        rx.await
            .map_err(|_| DaemonError::Internal("device actor stopped".into()))?
            .map_err(DaemonError::from)
    }

    /// Wipe the reader's on-device storage.
    pub async fn clear_storage(&self) -> Result<(), DaemonError> {
        let (reply, rx) = oneshot::channel();
        self.send(DeviceCommand::ClearStorage { reply }).await?;
        rx.await
            .map_err(|_| DaemonError::Internal("device actor stopped".into()))?
            .map_err(DaemonError::from)
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
        // Suspend/resume ride this separate priority channel so a suspend preempts an in-flight op.
        let (prio, prio_rx) = mpsc::channel(4);
        let id = info.id.clone();
        std::thread::Builder::new()
            .name(format!("fprintd-dev-{}", id.as_str()))
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("build device actor runtime");
                let local = tokio::task::LocalSet::new();
                local.block_on(&rt, async move {
                    let backend = factory();
                    run_actor(backend, id, rx, prio_rx).await;
                });
            })
            .expect("spawn device actor thread");
        DeviceHandle { tx, prio }
    }
}

/// The actor loop: owns the thread-local `backend` and an `Option<B::Device>` (opened on
/// first `Open`), servicing commands until every handle is dropped.
///
/// `prio` carries suspend/resume out-of-band. The loop `select!`s it ahead of ordinary commands
/// (`biased`), and each streaming op `select!`s it too: a `PrioCommand` arriving mid-op cancels the
/// op (dropping its future) and is then run against the now-free device. Because the priority
/// message is consumed exactly once — by whichever `select!` reaches it — a preempt can neither be
/// lost (hanging the suspend) nor linger to cancel a later, unrelated op.
async fn run_actor<B>(
    backend: B,
    id: DeviceId,
    mut rx: mpsc::Receiver<DeviceCommand>,
    mut prio: mpsc::Receiver<PrioCommand>,
) where
    B: Backend,
    B::Device: 'static,
{
    let mut dev: Option<B::Device> = None;

    loop {
        let cmd = tokio::select! {
            biased;
            Some(p) = prio.recv() => {
                handle_prio(&mut dev, p).await;
                continue;
            }
            cmd = rx.recv() => match cmd {
                Some(cmd) => cmd,
                None => break,
            },
        };
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
                let mut preempt = None;
                let res = match dev.as_mut() {
                    Some(d) => {
                        let mut on_progress = |p: EnrollProgress| {
                            let _ = progress.try_send(p);
                        };
                        tokio::select! {
                            r = d.enroll(template, &mut on_progress) => r,
                            _ = cancel => Err(Error::Cancelled),
                            Some(p) = prio.recv() => { preempt = Some(p); Err(Error::Cancelled) }
                        }
                    }
                    None => Err(Error::ProtoState),
                };
                let _ = reply.send(res);
                if let Some(p) = preempt {
                    handle_prio(&mut dev, p).await;
                }
            }
            DeviceCommand::Verify {
                enrolled,
                status,
                cancel,
                reply,
            } => {
                let mut preempt = None;
                let res = match dev.as_mut() {
                    Some(d) => {
                        let mut on_status = |s: FingerStatus| {
                            let _ = status.try_send(s);
                        };
                        tokio::select! {
                            r = d.verify_with_status(&enrolled, &mut on_status) => r,
                            _ = cancel => Err(Error::Cancelled),
                            Some(p) = prio.recv() => { preempt = Some(p); Err(Error::Cancelled) }
                        }
                    }
                    None => Err(Error::ProtoState),
                };
                let _ = reply.send(res);
                if let Some(p) = preempt {
                    handle_prio(&mut dev, p).await;
                }
            }
            DeviceCommand::Identify {
                gallery,
                status,
                cancel,
                reply,
            } => {
                let mut preempt = None;
                let res = match dev.as_mut() {
                    Some(d) => {
                        let mut on_status = |s: FingerStatus| {
                            let _ = status.try_send(s);
                        };
                        tokio::select! {
                            r = d.identify_with_status(&gallery, &mut on_status) => r,
                            _ = cancel => Err(Error::Cancelled),
                            Some(p) = prio.recv() => { preempt = Some(p); Err(Error::Cancelled) }
                        }
                    }
                    None => Err(Error::ProtoState),
                };
                let _ = reply.send(res);
                if let Some(p) = preempt {
                    handle_prio(&mut dev, p).await;
                }
            }
            DeviceCommand::ListPrints { reply } => {
                let res = match dev.as_mut() {
                    Some(d) => d.list_prints().await,
                    None => Err(Error::ProtoState),
                };
                let _ = reply.send(res);
            }
            DeviceCommand::DeletePrint { print, reply } => {
                let res = match dev.as_mut() {
                    Some(d) => d.delete_print(&print).await,
                    None => Err(Error::ProtoState),
                };
                let _ = reply.send(res);
            }
            DeviceCommand::ClearStorage { reply } => {
                let res = match dev.as_mut() {
                    Some(d) => d.clear_storage().await,
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

/// Run a priority command (suspend/resume) against the device and answer its reply. A not-yet-open
/// device suspends/resumes as a no-op (`Ok`), matching the ordinary command arms.
async fn handle_prio<D: Device>(dev: &mut Option<D>, cmd: PrioCommand) {
    match cmd {
        PrioCommand::Suspend { reply } => {
            let res = match dev.as_mut() {
                Some(d) => d.suspend().await,
                None => Ok(()),
            };
            let _ = reply.send(res);
        }
        PrioCommand::Resume { reply } => {
            let res = match dev.as_mut() {
                Some(d) => d.resume().await,
                None => Ok(()),
            };
            let _ = reply.send(res);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DeviceActor, DeviceHandle};
    use crate::command::DeviceCommand;
    use fprint_backend_native::{FingerId, Scenario, VirtualBackend, VirtualDeviceBuilder};
    use fprint_core::{Device as _, Error, Finger, FingerStatus, Print};
    use tokio::sync::{mpsc, oneshot};

    /// Spawn an actor over a fresh match-on-chip virtual reader (on-sensor storage, one enroll
    /// stage), returning its `Send` handle.
    fn moc_handle() -> DeviceHandle {
        let info = VirtualDeviceBuilder::chip_storage_sensor()
            .build()
            .info()
            .clone();
        DeviceActor::spawn(
            || VirtualBackend::single(VirtualDeviceBuilder::chip_storage_sensor()),
            info,
        )
    }

    /// Claim-equivalent: open the sensor on the actor thread.
    async fn open(handle: &DeviceHandle) {
        let (reply, rx) = oneshot::channel();
        handle.send(DeviceCommand::Open { reply }).await.unwrap();
        rx.await.unwrap().expect("open");
    }

    /// Enroll `finger` and return the finished print, driving the actor's `Enroll` arm.
    async fn enroll(handle: &DeviceHandle, finger: Finger) -> Print {
        let (progress, _progress_rx) = mpsc::channel(8);
        // Held for the whole enroll: dropping it would make the actor's `cancel` branch fire.
        let (_cancel_tx, cancel) = oneshot::channel();
        let (reply, rx) = oneshot::channel();
        handle
            .send(DeviceCommand::Enroll {
                finger,
                template: Print::new_for_enroll(finger),
                progress,
                cancel,
                reply,
            })
            .await
            .unwrap();
        rx.await.unwrap().expect("enroll")
    }

    /// **The on-device storage commands reach the reader.** An enrolled print appears in the
    /// device's own listing; a single delete frees just its slot; a fresh enroll fills it again and
    /// clear empties it — every step observed through `list_prints`, the actor's window onto
    /// on-sensor storage. One print is in flight at a time, so the reader's `DUPLICATES_CHECK` never
    /// trips.
    #[tokio::test]
    async fn on_device_storage_commands_reach_the_reader() {
        let handle = moc_handle();
        open(&handle).await;
        assert!(handle.list_prints().await.unwrap().is_empty());

        let print = enroll(&handle, Finger::RightIndex).await;
        assert_eq!(handle.list_prints().await.unwrap().len(), 1);

        // Delete frees the slot.
        handle.delete_device_print(print).await.unwrap();
        assert!(handle.list_prints().await.unwrap().is_empty());

        // Re-enroll (the store is empty again, so no duplicate), then clear.
        let _ = enroll(&handle, Finger::RightIndex).await;
        assert_eq!(handle.list_prints().await.unwrap().len(), 1);
        handle.clear_storage().await.unwrap();
        assert!(handle.list_prints().await.unwrap().is_empty());
    }

    /// **Suspend and resume route to an open reader without error.** The virtual device toggles its
    /// internal suspend flag; the observable contract at this layer is that the round trip succeeds
    /// (a not-open device is handled by the actor's `None` arm, exercised implicitly elsewhere).
    #[tokio::test]
    async fn suspend_and_resume_route_to_the_reader() {
        let handle = moc_handle();
        open(&handle).await;
        handle.suspend().await.unwrap();
        handle.resume().await.unwrap();
    }

    /// **A system-suspend preempts an in-flight verify parked on a finger.** The sensor hangs
    /// after reporting the finger present, so the verify never resolves on its own; the suspend
    /// (delivered on the priority channel) must cancel the parked op rather than queue behind it
    /// past logind's inhibitor deadline. Observable: `suspend()` returns `Ok` and the preempted
    /// verify's reply is `Err(Error::Cancelled)`.
    #[tokio::test]
    async fn suspend_preempts_a_parked_verify() {
        let scenario = Scenario::new().present(FingerId(1)).hang();
        let info = VirtualDeviceBuilder::chip_storage_sensor()
            .scenario(scenario.clone())
            .build()
            .info()
            .clone();
        let handle = DeviceActor::spawn(
            move || {
                VirtualBackend::single(
                    VirtualDeviceBuilder::chip_storage_sensor().scenario(scenario),
                )
            },
            info,
        );
        open(&handle).await;

        let (status, mut status_rx) = mpsc::channel(8);
        // Held for the whole verify: dropping it would fire the actor's `cancel` branch instead.
        let (_cancel_tx, cancel) = oneshot::channel();
        let (reply, reply_rx) = oneshot::channel();
        handle
            .send(DeviceCommand::Verify {
                enrolled: Print::new_for_enroll(Finger::RightIndex),
                status,
                cancel,
                reply,
            })
            .await
            .unwrap();

        // The first status is the finger going PRESENT: the verify is now in-flight and hanging.
        assert_eq!(status_rx.recv().await.unwrap(), FingerStatus::PRESENT);

        // Suspend must preempt the parked op, not wait behind it.
        handle.suspend().await.unwrap();

        // The priority-channel preempt cancelled the parked verify.
        assert_eq!(reply_rx.await.unwrap(), Err(Error::Cancelled));
    }

    /// **A suspend racing a just-issued verify never hangs.** A verify command is queued and a
    /// suspend fired immediately — before the actor could observe the finger — over many rounds. The
    /// suspend must always return within a bound: the priority command is consumed exactly once (by
    /// the idle loop or the op's own `select!`), so a preempt can neither be lost (hanging `suspend`)
    /// nor linger. (Regression: a drain + depth-1 interrupt channel could eat a preempt landing in
    /// the pre-`select!` window, hanging `suspend()` and, with it, logind's sleep.)
    #[tokio::test]
    async fn suspend_never_hangs_racing_a_verify() {
        let scenario = Scenario::new().present(FingerId(1)).hang();
        let info = VirtualDeviceBuilder::chip_storage_sensor()
            .scenario(scenario.clone())
            .build()
            .info()
            .clone();
        let handle = DeviceActor::spawn(
            move || {
                VirtualBackend::single(
                    VirtualDeviceBuilder::chip_storage_sensor().scenario(scenario),
                )
            },
            info,
        );
        open(&handle).await;

        for round in 0..200 {
            // Queue a hanging verify, then fire suspend with no gap — closing the window the old
            // drain could exploit. We do not wait for the finger status.
            let (status, _status_rx) = mpsc::channel(8);
            let (_cancel_tx, cancel) = oneshot::channel();
            let (reply, _reply_rx) = oneshot::channel();
            handle
                .send(DeviceCommand::Verify {
                    enrolled: Print::new_for_enroll(Finger::RightIndex),
                    status,
                    cancel,
                    reply,
                })
                .await
                .unwrap();

            tokio::time::timeout(std::time::Duration::from_secs(2), handle.suspend())
                .await
                .unwrap_or_else(|_| panic!("round {round}: suspend() hung behind a parked verify"))
                .unwrap();
            // Resume for the next round; if the verify was dequeued after the suspend and is now
            // parked, resume preempts it too — so the actor never carries a stuck op across rounds.
            tokio::time::timeout(std::time::Duration::from_secs(2), handle.resume())
                .await
                .unwrap_or_else(|_| panic!("round {round}: resume() hung"))
                .unwrap();
        }
    }
}
