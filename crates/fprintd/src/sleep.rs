// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! systemd-logind sleep integration: suspend every reader before the system sleeps, resume it
//! after, and hold a *delay* inhibitor across the transition so the readers are quiesced first.
//!
//! The mechanism is logind's (`org.freedesktop.login1.Manager`): take a `delay` inhibitor file
//! descriptor, then watch the `PrepareForSleep(b)` signal. On `true` (about to sleep) suspend each
//! device and then release the inhibitor fd, which is the daemon telling logind "go ahead". On
//! `false` (resumed) resume each device and re-arm a fresh inhibitor for the next cycle.
//!
//! logind may be absent — a private test bus has no `org.freedesktop.login1`. That is not an error
//! here: [`install`] then simply does nothing, and the daemon runs without sleep integration.

use zbus::zvariant::OwnedFd;

use crate::actor::DeviceHandle;
use futures_util::future::join_all;
use futures_util::StreamExt;

/// The slice of `org.freedesktop.login1.Manager` this daemon uses.
#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait Logind {
    /// Take an inhibitor lock. A `delay` lock on `sleep` yields a file descriptor whose lifetime is
    /// the lock: holding it delays the transition, closing it releases the delay.
    fn inhibit(&self, what: &str, who: &str, why: &str, mode: &str) -> zbus::Result<OwnedFd>;

    /// Emitted with `true` just before the system sleeps and `false` once it resumes.
    #[zbus(signal)]
    fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;
}

/// Arm sleep integration on `conn` for `handles`, spawning the watcher task.
///
/// Returns without arming anything when logind is unreachable (e.g. a private test bus), so it is
/// safe to call on every bus the daemon attaches to.
pub async fn install(conn: zbus::Connection, handles: Vec<DeviceHandle>) {
    let proxy = match LogindProxy::new(&conn).await {
        Ok(proxy) => proxy,
        Err(e) => {
            tracing::debug!("logind proxy unavailable, sleep integration disabled: {e}");
            return;
        }
    };
    let inhibitor = match take_inhibitor(&proxy).await {
        Ok(fd) => fd,
        Err(e) => {
            tracing::debug!("no logind sleep inhibitor (not on a system bus?): {e}");
            return;
        }
    };
    tokio::spawn(run(proxy, handles, inhibitor));
}

/// Take a fresh `delay` inhibitor for sleep, named as fprintd's.
async fn take_inhibitor(proxy: &LogindProxy<'_>) -> zbus::Result<OwnedFd> {
    proxy
        .inhibit(
            "sleep",
            "net.reactivated.Fprint",
            "Suspend fingerprint readers",
            "delay",
        )
        .await
}

/// Watch `PrepareForSleep` and drive every device across each transition.
///
/// The inhibitor is held between cycles and dropped only after every device is suspended, which is
/// what tells logind the readers are ready; a fresh one is taken after resume for the next sleep.
async fn run(proxy: LogindProxy<'static>, handles: Vec<DeviceHandle>, inhibitor: OwnedFd) {
    let mut signals = match proxy.receive_prepare_for_sleep().await {
        Ok(signals) => signals,
        Err(e) => {
            tracing::warn!("cannot watch logind PrepareForSleep: {e}");
            return;
        }
    };

    // `Some` while a cycle's inhibitor is held; dropped (→ `None`) to let a sleep proceed.
    let mut held = Some(inhibitor);
    while let Some(signal) = signals.next().await {
        let Ok(args) = signal.args() else { continue };
        if args.start {
            tracing::debug!("preparing {} device(s) for sleep", handles.len());
            // Suspend every reader concurrently: one slow device must not push the whole batch past
            // logind's `InhibitDelayMaxSec` (each `suspend` also preempts any in-flight op).
            let results = join_all(handles.iter().map(|h| h.suspend())).await;
            for result in results {
                if let Err(e) = result {
                    tracing::warn!("failed to suspend a reader for sleep: {e:?}");
                }
            }
            // Every reader is quiesced: release the delay so the system may sleep.
            held = None;
        } else {
            tracing::debug!("resuming {} device(s) after sleep", handles.len());
            let results = join_all(handles.iter().map(|h| h.resume())).await;
            for result in results {
                if let Err(e) = result {
                    tracing::warn!("failed to resume a reader after sleep: {e:?}");
                }
            }
            // Re-arm for the next cycle. On failure we log and carry `None`: the next suspend then
            // proceeds without a delay lock (degraded, but self-healing — the following resume
            // re-arms again). logind still sleeps; readers just are not guaranteed quiesced first.
            held = match take_inhibitor(&proxy).await {
                Ok(fd) => Some(fd),
                Err(e) => {
                    tracing::warn!("failed to re-arm the sleep inhibitor: {e}");
                    None
                }
            };
        }
    }
    drop(held);
}
