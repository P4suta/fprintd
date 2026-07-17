// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! # fprintd
//!
//! A pure-Rust daemon speaking the `net.reactivated.Fprint` D-Bus contract, so the existing
//! Linux fingerprint stack (pam_fprintd, GNOME/KDE settings) runs on it unchanged. It is
//! generic over any [`fprint_core::Backend`]: the `fprintd` binary drives the libfprint shim
//! ([`fprint_backend_libfprint`]); the integration test drives `fprint-backend-native`'s virtual
//! backend, so the whole D-Bus surface can be exercised with no hardware.
//!
//! ## Shape
//!
//! * [`Daemon`] owns the backend and, at start-up, discovers devices and wires up the D-Bus
//!   objects: one `manager::Manager` and one `device::Device` per reader.
//! * Each device is confined to its own actor thread (`actor`) that owns the possibly
//!   `!Send` `fprint_core::Device`; the D-Bus objects talk to it over `Send` channels
//!   (`command`). This is ARCHITECTURE.md principle 7.
//! * Wire quirks live at the edges: the `verify-*`/`enroll-*` strings in `status`, the
//!   `/var/lib/fprint` layout in `storage`, the FP3 bytes in [`fprint_fp3`].
//!
//! The whole crate is `#![cfg(target_os = "linux")]`; on other platforms it compiles to
//! nothing and the `fprintd` binary is a stub (see `main.rs`).

#![cfg(target_os = "linux")]
#![forbid(unsafe_code)]

mod actor;
mod authorizer;
mod command;
mod device;
mod error;
mod manager;
mod names;
mod sleep;
mod status;
mod storage;

use std::sync::Arc;

use fprint_core::{Backend, Device as _, DeviceInfo};
use zbus::zvariant::OwnedObjectPath;
use zbus::Connection;

use tokio::task::LocalSet;

use crate::actor::DeviceActor;
use crate::device::Device;
use crate::manager::Manager;

pub use crate::authorizer::{ActionSet, Authorizer, PolkitAction, PolkitAuthorizer};
pub use crate::error::DaemonError;
pub use crate::error::DaemonError as Error;
pub use crate::storage::Store;

/// The D-Bus prefix under which all objects live.
const OBJECT_PREFIX: &str = "/net/reactivated/Fprint";
/// The well-known bus name the daemon owns.
const BUS_NAME: &str = "net.reactivated.Fprint";

/// The assembled daemon: a backend factory, an authorizer, and a print store, ready to be
/// attached to a D-Bus connection.
///
/// The daemon never moves a backend across threads (ARCHITECTURE.md principle 7). Instead it
/// holds a `Send` factory `F: Fn() -> B`; each actor thread — and the one-shot enumeration
/// thread — calls it to build its own `B` locally, so `B` may be `!Send` (the libfprint shim
/// is). Only `Send` values (`DeviceInfo`, the command channels) ever cross a thread boundary.
pub struct Daemon<F> {
    factory: F,
    authz: Arc<Authorizer>,
    store: Arc<Store>,
}

impl<F, B> Daemon<F>
where
    F: Fn() -> B + Clone + Send + Sync + 'static,
    B: Backend,
    B::Device: 'static,
{
    /// Build a daemon with a store rooted from the environment (`STATE_DIRECTORY`).
    pub fn new(factory: F, authz: Arc<Authorizer>) -> Self {
        Daemon {
            factory,
            authz,
            store: Arc::new(Store::from_env()),
        }
    }

    /// Build a daemon with an explicit print store (used by tests).
    pub fn with_store(factory: F, authz: Arc<Authorizer>, store: Store) -> Self {
        Daemon {
            factory,
            authz,
            store: Arc::new(store),
        }
    }

    /// Discover devices, spawn their actors, register the Manager and Device objects on
    /// `builder`, and build the connection. Callers supply the builder so the same assembly
    /// works on the system bus (production) or a private bus (tests).
    pub async fn attach(
        self,
        builder: zbus::connection::Builder<'static>,
    ) -> Result<Connection, DaemonError> {
        // Enumerate once, for the device metadata only. `B` may be `!Send`, so we build it and
        // enumerate on a dedicated blocking thread with its own current-thread runtime, and
        // return just the `Send` `Vec<DeviceInfo>`; the devices themselves are dropped there.
        // Each actor later opens its own device, by id, on its own thread.
        let factory = self.factory.clone();
        let infos: Vec<DeviceInfo> = tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build enumeration runtime");
            let local = LocalSet::new();
            local.block_on(&rt, async move {
                let backend = factory();
                let devices = backend.enumerate().await?;
                Ok::<Vec<DeviceInfo>, DaemonError>(
                    devices.iter().map(|d| d.info().clone()).collect(),
                )
            })
        })
        .await
        .map_err(|e| DaemonError::Internal(format!("enumeration thread panicked: {e}")))??;

        let mut builder = builder;
        let mut paths: Vec<OwnedObjectPath> = Vec::with_capacity(infos.len());
        // Cloned handles for the sleep watcher, which suspends/resumes every device on a logind
        // `PrepareForSleep` — the one place besides the D-Bus objects that drives the actors.
        let mut handles = Vec::with_capacity(infos.len());

        for (index, info) in infos.into_iter().enumerate() {
            let path = format!("{OBJECT_PREFIX}/Device/{index}");
            let opath: OwnedObjectPath = zbus::zvariant::ObjectPath::try_from(path.as_str())
                .map_err(|e| DaemonError::Internal(format!("bad object path: {e}")))?
                .into();

            let handle = DeviceActor::spawn(self.factory.clone(), info.clone());
            handles.push(handle.clone());
            let device = Device::new(info, handle, self.store.clone(), self.authz.clone());
            builder = builder.serve_at(opath.clone(), device)?;
            paths.push(opath);
        }

        let manager = Manager::new(paths);
        let manager_path = OwnedObjectPath::try_from(format!("{OBJECT_PREFIX}/Manager"))
            .map_err(|e| DaemonError::Internal(format!("bad object path: {e}")))?;
        builder = builder.serve_at(manager_path, manager)?;

        let connection = builder.build().await?;
        // Arm logind sleep integration. A no-op where logind is absent (e.g. a private test bus).
        sleep::install(connection.clone(), handles).await;
        Ok(connection)
    }

    /// Attach to the system bus and request the `net.reactivated.Fprint` name.
    pub async fn serve_system(self) -> Result<Connection, DaemonError> {
        let builder = zbus::connection::Builder::system()?.name(BUS_NAME)?;
        self.attach(builder).await
    }
}

/// The `fprintd` binary entry point. Serves `net.reactivated.Fprint` on the system bus using
/// the libfprint shim, until `SIGINT`/`SIGTERM`.
///
/// `--test-mode` swaps PolicyKit for [`Authorizer::Fixed`] granting [`ActionSet::ALL`], for
/// bring-up against a virtual libfprint device without a running PolicyKit daemon. Packaging must
/// never pass it: `cargo xtask unit-verify` refuses a unit whose `ExecStart` names it.
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let test_mode = std::env::args().any(|a| a == "--test-mode");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");

    if let Err(e) = runtime.block_on(serve(test_mode)) {
        tracing::error!("fprintd fatal: {e:?}");
        std::process::exit(1);
    }
}

/// Construct the real backend/authorizer, serve, and wait for shutdown.
async fn serve(test_mode: bool) -> Result<(), DaemonError> {
    let authz: Arc<Authorizer> = if test_mode {
        tracing::warn!("running in --test-mode: PolicyKit checks are DISABLED");
        Arc::new(Authorizer::Fixed(ActionSet::ALL))
    } else {
        Arc::new(Authorizer::Polkit(PolkitAuthorizer::new().await?))
    };

    // The factory builds a fresh libfprint backend on whichever actor thread calls it, so the
    // `!Send` `FpContext` it holds never crosses a thread boundary.
    let factory = || fprint_backend_libfprint::LibfprintBackend::new();

    let daemon = Daemon::new(factory, authz);
    let _connection = daemon.serve_system().await?;
    tracing::info!("fprintd serving {BUS_NAME}");

    wait_for_shutdown().await;
    tracing::info!("fprintd shutting down");
    Ok(())
}

/// Resolve when the process receives `SIGINT` or `SIGTERM`.
async fn wait_for_shutdown() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}
