// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `net.reactivated.Fprint.Device` at `/net/reactivated/Fprint/Device/<n>`.
//!
//! This object is the daemon's public D-Bus surface for one reader. It holds no device — it
//! talks to the device's actor thread through a [`DeviceHandle`] — and translates between the
//! `net.reactivated.Fprint.Device` contract and `fprint_core`:
//!
//! * **Claim/Release** open and close the sensor and record the claiming bus name; every
//!   subsequent operation must come from that same sender (mirroring fprintd's session model).
//! * **EnrollStart / VerifyStart** validate and authorize, then spawn a *pump* task that
//!   drives the actor and turns progress/results into `EnrollStatus` / `VerifyStatus` signals
//!   via the [`crate::status`] vocabulary. Verify auto-restarts on retry-class results, and a
//!   1:1 match emits `VerifyFingerMatched` before `VerifyStatus verify-match` — exactly as
//!   `src/device.c` does.
//! * **EnrollStop / VerifyStop** signal the pump to cancel; dropping the actor's operation
//!   future releases the sensor (ARCHITECTURE.md principle 4).
//!
//! "One operation in flight" is enforced by inspecting the stored pump task: a new start is
//! refused while the previous pump is still running.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use fprint_core::{
    DeviceFeature, DeviceId, DeviceInfo, DriverId, Error, Finger, FingerStatus, Print,
};
use tokio::sync::oneshot;
use zbus::object_server::SignalEmitter;

use crate::actor::DeviceHandle;
use crate::authorizer::{Authorizer, PolkitAction};
use crate::command::DeviceCommand;
use crate::error::DaemonError;
use crate::names;
use crate::status;
use crate::storage::Store;
use futures_util::StreamExt;

/// Who currently holds the device, recorded at `Claim`.
struct Session {
    /// The unique bus name that claimed the device.
    sender: String,
    /// The resolved username whose prints this session reads/writes.
    username: String,
    /// Watches for `sender` leaving the bus, and releases the claim when it does.
    ///
    /// A claim is held by a *connection*, not by a request, so nothing in the protocol obliges a
    /// client to release one — it can simply exit. Without this, that claim outlives its owner
    /// and every later caller, `pam_fprintd` included, gets `AlreadyInUse` until the daemon
    /// restarts. Aborted by [`Device::release`], and on drop, so the watcher never outlives the
    /// session it belongs to.
    watcher: tokio::task::JoinHandle<()>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.watcher.abort();
    }
}

/// Which streaming operation an [`ActiveOp`] is driving.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OpKind {
    Verify,
    Enroll,
}

/// A running verify/enroll pump.
struct ActiveOp {
    kind: OpKind,
    /// Signals the pump to cancel the in-flight operation and stop.
    stop: oneshot::Sender<()>,
    /// The pump task; `is_finished()` tells us whether the op has completed on its own.
    task: tokio::task::JoinHandle<()>,
}

/// The `net.reactivated.Fprint.Device` object.
pub struct Device {
    /// The shape `Backend::enumerate` reported, replaced by the settled one when
    /// [`Device::claim`] opens the sensor (see [`DeviceCommand::Open`]).
    ///
    /// `Arc` behind the lock so a reader clones the pointer and drops the guard at once: a
    /// `MutexGuard` held across an `.await` would make these interface futures `!Send`.
    info: Mutex<Arc<DeviceInfo>>,
    handle: DeviceHandle,
    store: Arc<Store>,
    authz: Arc<Authorizer>,
    /// `Arc` so the name watcher a claim starts can perform the same teardown `Release` does.
    /// The watcher outlives the method that spawned it, and zbus owns the `Device` itself.
    claim: Arc<Mutex<Option<Session>>>,
    active: Arc<Mutex<Option<ActiveOp>>>,
    /// Live finger-presence status ([`FingerStatus`] bits), driving the `finger-present` /
    /// `finger-needed` properties. Updated by the enroll / verify / identify pump as the backend
    /// reports it, and reset to [`FingerStatus::NONE`] when no operation is streaming it; `Arc` so
    /// the detached pump task shares it with the interface object zbus owns.
    finger_status: Arc<AtomicU8>,
}

impl Device {
    /// Assemble a device object. `handle` is the `Send` handle to the device's actor thread.
    /// `info` is the enumerated shape; `claim` settles it.
    pub fn new(
        info: DeviceInfo,
        handle: DeviceHandle,
        store: Arc<Store>,
        authz: Arc<Authorizer>,
    ) -> Self {
        Device {
            info: Mutex::new(Arc::new(info)),
            handle,
            store,
            authz,
            claim: Arc::new(Mutex::new(None)),
            active: Arc::new(Mutex::new(None)),
            finger_status: Arc::new(AtomicU8::new(FingerStatus::NONE.bits())),
        }
    }

    /// The shape as currently known. Safe to hold across an `.await`.
    fn info(&self) -> Arc<DeviceInfo> {
        self.info.lock().unwrap().clone()
    }

    /// The live finger-presence status the enroll pump last reported (see [`Device::finger_present`]).
    fn live_finger_status(&self) -> FingerStatus {
        FingerStatus::from_bits_truncate(self.finger_status.load(Ordering::Relaxed))
    }

    // --- claim / session helpers ------------------------------------------------------

    /// The username of the current session iff `sender` is the claimer; else the appropriate
    /// `ClaimDevice` / `AlreadyInUse` error (fprintd's `_fprint_device_check_claimed`).
    fn require_claimed(&self, sender: &str) -> Result<String, DaemonError> {
        let guard = self.claim.lock().unwrap();
        match guard.as_ref() {
            None => Err(DaemonError::ClaimDevice(
                "Device was not claimed before use".into(),
            )),
            Some(s) if s.sender == sender => Ok(s.username.clone()),
            Some(_) => Err(DaemonError::AlreadyInUse(
                "Device already in use by another user".into(),
            )),
        }
    }

    /// Watch for `sender` leaving the bus, and run [`Device::teardown`] when it does.
    ///
    /// The bus filters for us: `receive_name_owner_changed_with_args` adds a match rule on
    /// argument 0, so this stream carries only the one name. A `new_owner` of `None` is the
    /// vanish — the same signal `pam_fprintd`'s own bus uses to notice a peer die.
    async fn spawn_name_watcher(
        conn: &zbus::Connection,
        sender: &str,
        handle: DeviceHandle,
        claim: Arc<Mutex<Option<Session>>>,
        active: Arc<Mutex<Option<ActiveOp>>>,
    ) -> Result<tokio::task::JoinHandle<()>, DaemonError> {
        let dbus = zbus::fdo::DBusProxy::new(conn).await?;
        let mut vanished = dbus
            .receive_name_owner_changed_with_args(&[(0, sender)])
            .await?;

        Ok(tokio::spawn(async move {
            while let Some(signal) = vanished.next().await {
                let Ok(args) = signal.args() else { continue };
                if args.new_owner().is_none() {
                    Device::teardown(&handle, &claim, &active).await;
                    return;
                }
            }
        }))
    }

    /// Stop any operation, close the sensor, drop the session.
    ///
    /// Free-standing over the two pieces of shared state rather than a method, because the name
    /// watcher runs it too and zbus owns the `Device`. `Release` and a vanished client must take
    /// *the same* path: two teardowns would be two things to keep in step.
    async fn teardown(
        handle: &DeviceHandle,
        claim: &Mutex<Option<Session>>,
        active: &Mutex<Option<ActiveOp>>,
    ) {
        if let Some(op) = active.lock().unwrap().take() {
            if !op.task.is_finished() {
                let _ = op.stop.send(());
            }
        }

        let (reply_tx, reply_rx) = oneshot::channel();
        if handle
            .send(DeviceCommand::Close { reply: reply_tx })
            .await
            .is_ok()
        {
            let _ = reply_rx.await;
        }

        // Last: dropping the `Session` aborts the watcher, and a watcher that ran this must not
        // be aborted before it finishes.
        *claim.lock().unwrap() = None;
    }

    // --- active-operation bookkeeping -------------------------------------------------

    /// Refuse if a pump is still running (`AlreadyInUse`), else clear any finished one.
    fn ensure_idle(&self) -> Result<(), DaemonError> {
        let mut guard = self.active.lock().unwrap();
        if let Some(op) = guard.as_ref() {
            if !op.task.is_finished() {
                return Err(DaemonError::AlreadyInUse(
                    "Another operation is already in progress".into(),
                ));
            }
        }
        *guard = None;
        Ok(())
    }

    /// Record the pump task for the just-started operation.
    fn set_active(&self, op: ActiveOp) {
        *self.active.lock().unwrap() = Some(op);
    }

    /// Stop the active operation of kind `expected` (fprintd's `*_stop`). A completed pump is
    /// simply cleared; a mismatched still-running op yields `AlreadyInUse`; nothing running
    /// yields `NoActionInProgress`.
    fn stop_active(&self, expected: OpKind) -> Result<(), DaemonError> {
        let mut guard = self.active.lock().unwrap();
        // Read the `Copy` facts first so the borrow ends before we mutate `guard`.
        let (finished, kind) = match guard.as_ref() {
            None => return Err(DaemonError::NoActionInProgress(no_action_message(expected))),
            Some(op) => (op.task.is_finished(), op.kind),
        };
        if finished {
            *guard = None;
            return Ok(());
        }
        if kind != expected {
            return Err(DaemonError::AlreadyInUse(
                "Another operation is already in progress".into(),
            ));
        }
        if let Some(op) = guard.take() {
            let _ = op.stop.send(());
        }
        Ok(())
    }
}

#[zbus::interface(name = "net.reactivated.Fprint.Device")]
impl Device {
    // --- properties -------------------------------------------------------------------

    #[zbus(property, name = "name")]
    async fn name(&self) -> String {
        self.info().name.clone()
    }

    /// The scan type the sensor settled on at `Claim`; before that, the enumerated one.
    #[zbus(property, name = "scan-type")]
    async fn scan_type(&self) -> String {
        names::scan_type_dbus_str(self.info().scan_type).to_string()
    }

    /// `-1` until the device is claimed, then the number of enroll stages. The count is only
    /// knowable once the sensor is open, and `Claim` is what opens it.
    #[zbus(property, name = "num-enroll-stages")]
    async fn num_enroll_stages(&self) -> i32 {
        if self.claim.lock().unwrap().is_some() {
            i32::try_from(self.info().enroll_stages).unwrap_or(i32::MAX)
        } else {
            -1
        }
    }

    /// Whether a finger is on the sensor.
    ///
    /// Reflects the live [`FingerStatus`] the backend reports during an enroll, verify, or identify
    /// operation; that operation's pump updates it and emits a change signal, driving the desktop's
    /// "touch the sensor" prompts. Between operations it is [`FingerStatus::NONE`], so this reads
    /// `false`.
    #[zbus(property, name = "finger-present")]
    async fn finger_present(&self) -> bool {
        self.live_finger_status().contains(FingerStatus::PRESENT)
    }

    /// Whether the sensor is waiting for a finger (see [`Device::finger_present`]).
    #[zbus(property, name = "finger-needed")]
    async fn finger_needed(&self) -> bool {
        self.live_finger_status().contains(FingerStatus::NEEDED)
    }

    // --- anytime / auto-claim methods -------------------------------------------------

    /// List the fingers enrolled for `username` (STATE_ANYTIME: no claim required).
    async fn list_enrolled_fingers(
        &self,
        username: &str,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> Result<Vec<String>, DaemonError> {
        let sender = sender_of(&hdr)?;
        self.authz.check(&sender, PolkitAction::Verify).await?;
        let user = resolve_user(conn, &sender, username, &self.authz).await?;

        let info = self.info();
        let names: Vec<String> = self
            .store
            .list_fingers(&user, &info.driver, &info.id)
            .into_iter()
            .map(|f| names::finger_dbus_name(f).to_string())
            .collect();

        if names.is_empty() {
            return Err(DaemonError::NoEnrolledPrints(
                "Failed to discover prints".into(),
            ));
        }
        Ok(names)
    }

    /// Delete every enrolled finger for `username` (legacy, auto-claim form).
    async fn delete_enrolled_fingers(
        &self,
        username: &str,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> Result<(), DaemonError> {
        let sender = sender_of(&hdr)?;
        self.authz.check(&sender, PolkitAction::Enroll).await?;
        let user = resolve_user(conn, &sender, username, &self.authz).await?;
        self.delete_all(&user)
    }

    /// Delete every enrolled finger for the claiming user.
    async fn delete_enrolled_fingers2(
        &self,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
    ) -> Result<(), DaemonError> {
        let sender = sender_of(&hdr)?;
        let user = self.require_claimed(&sender)?;
        self.authz.check(&sender, PolkitAction::Enroll).await?;
        self.delete_all(&user)
    }

    /// Delete one enrolled finger for the claiming user.
    async fn delete_enrolled_finger(
        &self,
        finger_name: &str,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
    ) -> Result<(), DaemonError> {
        let sender = sender_of(&hdr)?;
        let user = self.require_claimed(&sender)?;
        self.authz.check(&sender, PolkitAction::Enroll).await?;
        let finger = real_finger(finger_name)?;

        let info = self.info();
        let has = self
            .store
            .list_fingers(&user, &info.driver, &info.id)
            .contains(&finger);
        if !has {
            return Err(DaemonError::NoEnrolledPrints(format!(
                "Fingerprint for finger {finger_name} is not enrolled"
            )));
        }
        self.store.delete(&user, &info.driver, &info.id, finger)
    }

    // --- claim / release --------------------------------------------------------------

    /// Claim the device for `username` and open the sensor.
    async fn claim(
        &self,
        username: &str,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> Result<(), DaemonError> {
        let sender = sender_of(&hdr)?;

        // Claim needs authorization for verify OR enroll.
        if self
            .authz
            .check(&sender, PolkitAction::Verify)
            .await
            .is_err()
        {
            self.authz.check(&sender, PolkitAction::Enroll).await?;
        }
        let user = resolve_user(conn, &sender, username, &self.authz).await?;

        // Built before the claim is taken, so every path that does not take it drops the session
        // and aborts the watcher with it. A `JoinHandle` dropped on its own detaches instead.
        let session = Session {
            sender: sender.clone(),
            username: user,
            watcher: Self::spawn_name_watcher(
                conn,
                &sender,
                self.handle.clone(),
                Arc::clone(&self.claim),
                Arc::clone(&self.active),
            )
            .await?,
        };

        {
            let mut guard = self.claim.lock().unwrap();
            if guard.is_some() {
                return Err(DaemonError::AlreadyInUse(
                    "Device was already claimed".into(),
                ));
            }
            *guard = Some(session);
        }

        // Open the sensor on the actor thread.
        let (reply_tx, reply_rx) = oneshot::channel();
        if let Err(e) = self
            .handle
            .send(DeviceCommand::Open { reply: reply_tx })
            .await
        {
            *self.claim.lock().unwrap() = None;
            return Err(e);
        }
        match reply_rx.await {
            Ok(Ok(settled)) => {
                // The sensor is open, so its shape is now known. Everything published from here
                // reads this, not the enumerated info.
                *self.info.lock().unwrap() = Arc::new(settled);
                Ok(())
            }
            Ok(Err(e)) => {
                *self.claim.lock().unwrap() = None;
                Err(DaemonError::Internal(format!(
                    "Open failed with error: {e}"
                )))
            }
            Err(_) => {
                *self.claim.lock().unwrap() = None;
                Err(DaemonError::Internal("device actor stopped".into()))
            }
        }
    }

    /// Release a claimed device: stop any operation, close the sensor, drop the session.
    async fn release(
        &self,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
    ) -> Result<(), DaemonError> {
        let sender = sender_of(&hdr)?;
        let _ = self.require_claimed(&sender)?;

        Device::teardown(&self.handle, &self.claim, &self.active).await;
        Ok(())
    }

    // --- verify -----------------------------------------------------------------------

    /// Start verifying `finger_name` (or `"any"`) against the claiming user's prints.
    async fn verify_start(
        &self,
        finger_name: &str,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), DaemonError> {
        let sender = sender_of(&hdr)?;
        let user = self.require_claimed(&sender)?;
        self.authz.check(&sender, PolkitAction::Verify).await?;
        self.ensure_idle()?;

        let info = self.info();
        let driver = &info.driver;
        let device_id = &info.id;
        let requested = names::finger_from_dbus_name(finger_name)
            .ok_or_else(|| DaemonError::InvalidFingername("Invalid finger name".into()))?;

        // Resolve which print(s) to match, mirroring fprint_device_verify_start.
        let op = if requested != Finger::Unknown {
            let print = self
                .store
                .load(&user, driver, device_id, requested)
                .ok_or_else(|| {
                    DaemonError::NoEnrolledPrints(format!("No such print {}", requested.as_u8()))
                })?;
            VerifyOp::Single {
                print,
                finger: requested,
            }
        } else {
            let mut gallery: Vec<(Finger, Print)> = self
                .store
                .list_fingers(&user, driver, device_id)
                .into_iter()
                .filter_map(|f| self.store.load(&user, driver, device_id, f).map(|p| (f, p)))
                .collect();

            if gallery.is_empty() {
                return Err(DaemonError::NoEnrolledPrints(
                    "No fingerprints enrolled".into(),
                ));
            } else if let [only] = gallery.as_slice() {
                let (finger, print) = only.clone();
                VerifyOp::Single { print, finger }
            } else if info.features.contains(DeviceFeature::IDENTIFY) {
                VerifyOp::Identify { gallery }
            } else {
                let (finger, print) = gallery.remove(0);
                VerifyOp::Single { print, finger }
            }
        };

        let selected: &'static str = match &op {
            VerifyOp::Single { finger, .. } => names::finger_dbus_name(*finger),
            VerifyOp::Identify { .. } => "any",
        };

        // Spawn the pump, then tell the client which finger we selected.
        let (stop_tx, stop_rx) = oneshot::channel();
        let handle = self.handle.clone();
        let owned = emitter.to_owned();
        let pump_emitter = owned.clone();
        let finger_status = self.finger_status.clone();
        let task = tokio::spawn(async move {
            run_verify(handle, pump_emitter, op, finger_status, stop_rx).await;
        });
        self.set_active(ActiveOp {
            kind: OpKind::Verify,
            stop: stop_tx,
            task,
        });

        let _ = Device::verify_finger_selected(&owned, selected).await;
        Ok(())
    }

    /// Stop an ongoing verification.
    async fn verify_stop(
        &self,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
    ) -> Result<(), DaemonError> {
        let sender = sender_of(&hdr)?;
        let _ = self.require_claimed(&sender)?;
        self.stop_active(OpKind::Verify)
    }

    // --- enroll -----------------------------------------------------------------------

    /// Start enrolling `finger_name` for the claiming user.
    async fn enroll_start(
        &self,
        finger_name: &str,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), DaemonError> {
        let sender = sender_of(&hdr)?;
        let user = self.require_claimed(&sender)?;
        self.authz.check(&sender, PolkitAction::Enroll).await?;
        let finger = real_finger(finger_name)?;
        self.ensure_idle()?;

        // Build the enrollment template with the metadata storage needs.
        let mut template = Print::new_for_enroll(finger);
        template.username = Some(user.clone());
        let info = self.info();
        template.driver = Some(info.driver.clone());
        template.device_id = Some(info.id.clone());

        let (stop_tx, stop_rx) = oneshot::channel();
        let handle = self.handle.clone();
        let emitter = emitter.to_owned();
        let store = self.store.clone();
        let driver = info.driver.clone();
        let device_id = info.id.clone();
        let finger_status = self.finger_status.clone();
        let task = tokio::spawn(async move {
            run_enroll(EnrollPump {
                handle,
                emitter,
                template,
                finger,
                store,
                user,
                driver,
                device_id,
                finger_status,
                stop_rx,
            })
            .await;
        });
        self.set_active(ActiveOp {
            kind: OpKind::Enroll,
            stop: stop_tx,
            task,
        });
        Ok(())
    }

    /// Stop an ongoing enrollment.
    async fn enroll_stop(
        &self,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
    ) -> Result<(), DaemonError> {
        let sender = sender_of(&hdr)?;
        let _ = self.require_claimed(&sender)?;
        self.stop_active(OpKind::Enroll)
    }

    // --- signals (bodyless: emitted via the generated associated functions) -----------

    #[zbus(signal)]
    async fn verify_status(
        emitter: &SignalEmitter<'_>,
        result: &str,
        done: bool,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn verify_finger_selected(
        emitter: &SignalEmitter<'_>,
        finger_name: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn verify_finger_matched(
        emitter: &SignalEmitter<'_>,
        finger_name: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn enroll_status(
        emitter: &SignalEmitter<'_>,
        result: &str,
        done: bool,
    ) -> zbus::Result<()>;
}

impl Device {
    /// Delete all of a user's prints, erroring `NoEnrolledPrints` if there were none — the
    /// behaviour of fprintd's `delete_enrolled_fingers(FP_FINGER_UNKNOWN)`.
    fn delete_all(&self, user: &str) -> Result<(), DaemonError> {
        let info = self.info();
        let fingers = self.store.list_fingers(user, &info.driver, &info.id);
        if fingers.is_empty() {
            return Err(DaemonError::NoEnrolledPrints(
                "No fingerprint enrolled".into(),
            ));
        }
        for finger in fingers {
            self.store.delete(user, &info.driver, &info.id, finger)?;
        }
        Ok(())
    }
}

/// A `VerifyStart` resolved to concrete work: a 1:1 verify or a 1:N identify.
enum VerifyOp {
    Single { print: Print, finger: Finger },
    Identify { gallery: Vec<(Finger, Print)> },
}

/// The verify pump: drive the actor, auto-restart on retry, and stream `VerifyStatus`.
async fn run_verify(
    handle: DeviceHandle,
    emitter: SignalEmitter<'static>,
    op: VerifyOp,
    finger_status: Arc<AtomicU8>,
    stop_rx: oneshot::Receiver<()>,
) {
    match op {
        VerifyOp::Single { print, finger } => {
            verify_loop(handle, emitter, print, finger, finger_status, stop_rx).await
        }
        VerifyOp::Identify { gallery } => {
            identify_loop(handle, emitter, gallery, finger_status, stop_rx).await
        }
    }
}

async fn verify_loop(
    handle: DeviceHandle,
    emitter: SignalEmitter<'static>,
    print: Print,
    finger: Finger,
    finger_status: Arc<AtomicU8>,
    mut stop_rx: oneshot::Receiver<()>,
) {
    loop {
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let (reply_tx, mut reply_rx) = oneshot::channel();
        let (status_tx, mut status_rx) = tokio::sync::mpsc::channel(8);
        let cmd = DeviceCommand::Verify {
            enrolled: print.clone(),
            status: status_tx,
            cancel: cancel_rx,
            reply: reply_tx,
        };
        if handle.send(cmd).await.is_err() {
            break;
        }

        // Wait for the outcome, publishing live finger status as it streams in.
        let result = loop {
            tokio::select! {
                r = &mut reply_rx => break r,
                _ = &mut stop_rx => { drop(cancel_tx); publish_finger_status(&finger_status, &emitter, FingerStatus::NONE).await; return; }
                Some(s) = status_rx.recv() => {
                    publish_finger_status(&finger_status, &emitter, s).await;
                }
            }
        };
        drop(cancel_tx);

        match result {
            Ok(Ok(outcome)) => {
                if outcome.matched {
                    let _ =
                        Device::verify_finger_matched(&emitter, names::finger_dbus_name(finger))
                            .await;
                }
                let (s, done) = status::verify_match(outcome.matched);
                let _ = Device::verify_status(&emitter, s, done).await;
                break;
            }
            Ok(Err(Error::RetryScan(reason))) => {
                let (s, done) = status::verify_retry(reason);
                let _ = Device::verify_status(&emitter, s, done).await;
                // The finger lifted between attempts; the next attempt re-reports presence.
                publish_finger_status(&finger_status, &emitter, FingerStatus::NONE).await;
                // loop: re-issue the verify
            }
            Ok(Err(e)) => {
                let (s, done) = status::verify_error(&e);
                let _ = Device::verify_status(&emitter, s, done).await;
                break;
            }
            Err(_) => break, // actor gone
        }
    }

    // The operation is over: the sensor is no longer reporting finger presence.
    publish_finger_status(&finger_status, &emitter, FingerStatus::NONE).await;
}

async fn identify_loop(
    handle: DeviceHandle,
    emitter: SignalEmitter<'static>,
    gallery: Vec<(Finger, Print)>,
    finger_status: Arc<AtomicU8>,
    mut stop_rx: oneshot::Receiver<()>,
) {
    let prints: Vec<Print> = gallery.iter().map(|(_, p)| p.clone()).collect();
    loop {
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let (reply_tx, mut reply_rx) = oneshot::channel();
        let (status_tx, mut status_rx) = tokio::sync::mpsc::channel(8);
        let cmd = DeviceCommand::Identify {
            gallery: prints.clone(),
            status: status_tx,
            cancel: cancel_rx,
            reply: reply_tx,
        };
        if handle.send(cmd).await.is_err() {
            break;
        }

        let result = loop {
            tokio::select! {
                r = &mut reply_rx => break r,
                _ = &mut stop_rx => { drop(cancel_tx); publish_finger_status(&finger_status, &emitter, FingerStatus::NONE).await; return; }
                Some(s) = status_rx.recv() => {
                    publish_finger_status(&finger_status, &emitter, s).await;
                }
            }
        };
        drop(cancel_tx);

        match result {
            Ok(Ok(outcome)) => {
                let matched_finger = outcome
                    .match_index
                    .and_then(|i| gallery.get(i))
                    .map(|(f, _)| *f);
                if let Some(finger) = matched_finger {
                    let _ =
                        Device::verify_finger_matched(&emitter, names::finger_dbus_name(finger))
                            .await;
                }
                let (s, done) = status::verify_match(matched_finger.is_some());
                let _ = Device::verify_status(&emitter, s, done).await;
                break;
            }
            Ok(Err(Error::RetryScan(reason))) => {
                let (s, done) = status::verify_retry(reason);
                let _ = Device::verify_status(&emitter, s, done).await;
                publish_finger_status(&finger_status, &emitter, FingerStatus::NONE).await;
            }
            Ok(Err(e)) => {
                let (s, done) = status::verify_error(&e);
                let _ = Device::verify_status(&emitter, s, done).await;
                break;
            }
            Err(_) => break,
        }
    }

    publish_finger_status(&finger_status, &emitter, FingerStatus::NONE).await;
}

/// Everything the enroll pump needs: the actor `handle`, the signal `emitter`, the `template`
/// being enrolled, its `finger`, and the storage coordinates (`store` / `user` / `driver` /
/// `device_id`) for persisting the finished print, plus the `stop_rx` cancellation signal.
struct EnrollPump {
    handle: DeviceHandle,
    emitter: SignalEmitter<'static>,
    template: Print,
    finger: Finger,
    store: Arc<Store>,
    user: String,
    driver: DriverId,
    device_id: DeviceId,
    /// Shared with the [`Device`] object: the live [`FingerStatus`] backing `finger-present` /
    /// `finger-needed`. The pump writes each progress report's status here and signals the change.
    finger_status: Arc<AtomicU8>,
    stop_rx: oneshot::Receiver<()>,
}

/// The enroll pump: forward progress as `EnrollStatus`, then save the finished print and
/// report `enroll-completed` / `enroll-failed`, or map a terminal error.
///
/// Along the way it publishes each report's live [`FingerStatus`] to the `finger-present` /
/// `finger-needed` properties, and clears it to [`FingerStatus::NONE`] when the pump ends — the
/// sensor is no longer streaming presence once the operation is over.
async fn run_enroll(pump: EnrollPump) {
    let EnrollPump {
        handle,
        emitter,
        template,
        finger,
        store,
        user,
        driver,
        device_id,
        finger_status,
        mut stop_rx,
    } = pump;

    let (prog_tx, mut prog_rx) = tokio::sync::mpsc::channel(8);
    // `_cancel_tx` is held for the whole pump: dropping it (on any exit) tells the actor
    // to cancel the in-flight enroll (ARCHITECTURE.md principle 4).
    let (_cancel_tx, cancel_rx) = oneshot::channel();
    let (reply_tx, mut reply_rx) = oneshot::channel();
    let cmd = DeviceCommand::Enroll {
        finger,
        template,
        progress: prog_tx,
        cancel: cancel_rx,
        reply: reply_tx,
    };
    if handle.send(cmd).await.is_err() {
        return;
    }

    let mut prog_open = true;
    loop {
        tokio::select! {
            _ = &mut stop_rx => break,
            maybe = prog_rx.recv(), if prog_open => {
                match maybe {
                    Some(p) => {
                        publish_finger_status(&finger_status, &emitter, p.finger_status).await;
                        if let Some(s) = status::enroll_progress(&p) {
                            let _ = Device::enroll_status(&emitter, s, false).await;
                        }
                    }
                    None => prog_open = false,
                }
            }
            r = &mut reply_rx => {
                let status = match r {
                    Ok(Ok(mut print)) => {
                        // Ensure the fields storage keys on are present, then persist.
                        print.username.get_or_insert_with(|| user.clone());
                        print.finger.get_or_insert(finger);
                        print.driver.get_or_insert_with(|| driver.clone());
                        print.device_id.get_or_insert_with(|| device_id.clone());
                        match store.save(&print) {
                            Ok(()) => "enroll-completed",
                            Err(_) => "enroll-failed",
                        }
                    }
                    Ok(Err(e)) => status::enroll_error(&e),
                    Err(_) => break, // actor gone
                };
                let _ = Device::enroll_status(&emitter, status, true).await;
                break;
            }
        }
    }

    // The operation is over: the sensor is no longer reporting finger presence.
    publish_finger_status(&finger_status, &emitter, FingerStatus::NONE).await;
}

/// Publish a finger-status change to the `finger-present` / `finger-needed` properties: store the
/// new bits and, if they changed, emit `PropertiesChanged` from the live `Device` object.
///
/// Best-effort — if the interface is already gone (the object was torn down) the emission is simply
/// skipped; the stored value is what the property getter reads regardless.
async fn publish_finger_status(
    status: &AtomicU8,
    emitter: &SignalEmitter<'static>,
    new: FingerStatus,
) {
    let previous = status.swap(new.bits(), Ordering::Relaxed);
    if previous == new.bits() {
        return;
    }
    let server = emitter.connection().object_server();
    if let Ok(iface) = server.interface::<_, Device>(emitter.path()).await {
        // The generated `*_changed` emitters re-read the getters (which read the value just stored)
        // and emit `PropertiesChanged`; the pump's own `emitter` carries the right path.
        let device = iface.get_mut().await;
        let _ = device.finger_present_changed(emitter).await;
        let _ = device.finger_needed_changed(emitter).await;
    }
}

// --- free helpers ---------------------------------------------------------------------

/// The caller's unique bus name, or an internal error if the message has none.
fn sender_of(hdr: &zbus::message::Header<'_>) -> Result<String, DaemonError> {
    hdr.sender()
        .map(|s| s.to_string())
        .ok_or_else(|| DaemonError::Internal("message has no sender".into()))
}

/// Parse a finger name that must be a real finger (rejects `""` / `"any"` / unknown).
fn real_finger(finger_name: &str) -> Result<Finger, DaemonError> {
    match names::finger_from_dbus_name(finger_name) {
        Some(f) if f != Finger::Unknown => Ok(f),
        _ => Err(DaemonError::InvalidFingername("Invalid finger name".into())),
    }
}

/// The `NoActionInProgress` message for a stop of the given kind.
fn no_action_message(kind: OpKind) -> String {
    match kind {
        OpKind::Verify => "No verification in progress".into(),
        OpKind::Enroll => "No enrollment in progress".into(),
    }
}

/// Resolve the effective username for a request, following fprintd's rules: an empty request
/// means "the caller's own user"; a different user requires the `setusername` authorization.
async fn resolve_user(
    conn: &zbus::Connection,
    sender: &str,
    requested: &str,
    authz: &Arc<Authorizer>,
) -> Result<String, DaemonError> {
    let own = caller_username(conn, sender).await;
    if requested.is_empty() {
        return own
            .ok_or_else(|| DaemonError::Internal("could not determine caller's username".into()));
    }
    if own.as_deref() == Some(requested) {
        return Ok(requested.to_string());
    }
    authz.check(sender, PolkitAction::SetUsername).await?;
    Ok(requested.to_string())
}

/// The caller's own username, resolved via `GetConnectionUnixUser` on the request's bus and a
/// `/etc/passwd` lookup. Best-effort: returns `None` if either step fails.
async fn caller_username(conn: &zbus::Connection, sender: &str) -> Option<String> {
    let proxy = zbus::fdo::DBusProxy::new(conn).await.ok()?;
    let bus_name = zbus::names::BusName::try_from(sender).ok()?;
    let uid = proxy.get_connection_unix_user(bus_name).await.ok()?;
    uid_to_name(uid)
}

/// Map a uid to a username by scanning `/etc/passwd`.
fn uid_to_name(uid: u32) -> Option<String> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    for line in passwd.lines() {
        let mut fields = line.split(':');
        let name = fields.next()?;
        let _password = fields.next()?;
        let field_uid = fields.next()?;
        if field_uid.parse::<u32>().ok() == Some(uid) {
            return Some(name.to_string());
        }
    }
    None
}
