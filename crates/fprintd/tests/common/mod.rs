// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Shared scaffolding for the D-Bus integration tests: a private bus, the client-side proxies for
//! the two interfaces under test, and a harness that stands a daemon up on that bus under its own
//! well-known name.
//!
//! Cargo compiles this module into every test binary, so each one uses a subset of it — hence the
//! `dead_code` allowance below, which is a consequence of `tests/common` and not a loose end.

#![cfg(target_os = "linux")]
#![allow(dead_code)]

use std::sync::{Arc, Mutex, Weak};

use fprint_backend_native::VirtualBackend;
use fprintd::{ActionSet, Authorizer, Daemon, Store};
use zbus::zvariant::OwnedObjectPath;

/// A private session bus, so the tests are self-contained: no ambient `DBUS_SESSION_BUS_ADDRESS`,
/// no `dbus-run-session` wrapper. The daemon and client both connect to it, and it is killed when
/// the last holder drops.
///
/// **One per test binary, not one per test.** Its address is a process-global environment variable,
/// so two buses in one process would race for it and a client would connect to the wrong one. Use
/// [`PrivateBus::shared`], which hands every test in the binary the same bus; a file that wants
/// several daemons gives each its own well-known name (see [`Harness`]) rather than its own bus.
pub struct PrivateBus {
    child: std::process::Child,
}

/// The one bus per process. `Weak`, so the last test to finish drops the bus and kills the daemon
/// rather than leaving it orphaned at exit.
static SHARED: Mutex<Weak<PrivateBus>> = Mutex::new(Weak::new());

impl PrivateBus {
    /// The bus for this test binary, starting it if no test currently holds one.
    ///
    /// Hold the returned handle for the whole test: dropping the last one tears the bus down.
    pub fn shared() -> Arc<PrivateBus> {
        let mut guard = SHARED.lock().unwrap();
        if let Some(bus) = guard.upgrade() {
            return bus;
        }
        let bus = Arc::new(PrivateBus::start());
        *guard = Arc::downgrade(&bus);
        bus
    }

    fn start() -> Self {
        use std::io::BufRead;
        let mut child = std::process::Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn dbus-daemon (install the `dbus` package)");
        let stdout = child.stdout.take().expect("dbus-daemon stdout");
        let mut address = String::new();
        std::io::BufReader::new(stdout)
            .read_line(&mut address)
            .expect("read bus address");
        // Written under the `SHARED` lock, and only when no bus exists — so nothing else in this
        // process is reading or writing it.
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", address.trim());
        PrivateBus { child }
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[zbus::proxy(
    interface = "net.reactivated.Fprint.Manager",
    default_service = "net.reactivated.Fprint",
    default_path = "/net/reactivated/Fprint/Manager"
)]
pub trait Manager {
    fn get_devices(&self) -> zbus::Result<Vec<OwnedObjectPath>>;
    fn get_default_device(&self) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(
    interface = "net.reactivated.Fprint.Device",
    default_service = "net.reactivated.Fprint"
)]
pub trait Device {
    fn claim(&self, username: &str) -> zbus::Result<()>;
    fn release(&self) -> zbus::Result<()>;
    fn enroll_start(&self, finger_name: &str) -> zbus::Result<()>;
    fn enroll_stop(&self) -> zbus::Result<()>;
    fn verify_start(&self, finger_name: &str) -> zbus::Result<()>;
    fn verify_stop(&self) -> zbus::Result<()>;
    fn list_enrolled_fingers(&self, username: &str) -> zbus::Result<Vec<String>>;
    fn delete_enrolled_fingers(&self, username: &str) -> zbus::Result<()>;
    #[zbus(name = "DeleteEnrolledFingers2")]
    fn delete_enrolled_fingers2(&self) -> zbus::Result<()>;
    fn delete_enrolled_finger(&self, finger_name: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    fn enroll_status(&self, result: String, done: bool) -> zbus::Result<()>;
    #[zbus(signal)]
    fn verify_status(&self, result: String, done: bool) -> zbus::Result<()>;
    #[zbus(signal)]
    fn verify_finger_selected(&self, finger_name: String) -> zbus::Result<()>;
    #[zbus(signal)]
    fn verify_finger_matched(&self, finger_name: String) -> zbus::Result<()>;

    #[zbus(property, name = "num-enroll-stages")]
    fn num_enroll_stages(&self) -> zbus::Result<i32>;
    #[zbus(property, name = "scan-type")]
    fn scan_type(&self) -> zbus::Result<String>;
    #[zbus(property, name = "name")]
    fn name(&self) -> zbus::Result<String>;
}

/// A daemon serving on the shared bus under its own well-known name.
///
/// [`PrivateBus`] is one per test *binary*, but a bus hosts as many names as it is asked for. So a
/// file that needs several daemons — one per authorization scenario — stands each up under its own
/// name rather than needing a bus each. Every proxy here is given an explicit destination, so two
/// harnesses never answer for one another.
pub struct Harness {
    name: String,
    _conn: zbus::Connection,
    store_root: std::path::PathBuf,
}

impl Harness {
    /// Serve `backend` under `net.reactivated.Fprint.<scenario>`, authorizing exactly `grants`.
    pub async fn serve(
        scenario: &str,
        grants: ActionSet,
        backend: fn() -> VirtualBackend,
    ) -> Harness {
        let name = format!("net.reactivated.Fprint.{scenario}");
        let store_root =
            std::env::temp_dir().join(format!("fprintd-{}-{scenario}", std::process::id()));
        let _ = std::fs::remove_dir_all(&store_root);

        let daemon = Daemon::with_store(
            backend,
            Arc::new(Authorizer::Fixed(grants)),
            Store::with_root(store_root.clone()),
        );
        let builder = zbus::connection::Builder::session()
            .expect("session bus")
            .name(name.clone())
            .expect("request name");
        let conn = daemon.attach(builder).await.expect("attach daemon");
        Harness {
            name,
            _conn: conn,
            store_root,
        }
    }

    /// Where this harness's daemon writes prints. Nothing may appear here that no client enrolled.
    pub fn store_root(&self) -> &std::path::Path {
        &self.store_root
    }

    /// A fresh client connection — a distinct unique bus name, which is what the daemon keys a
    /// claim on. Two of these are two clients.
    pub async fn client(&self) -> zbus::Connection {
        zbus::Connection::session().await.expect("client session")
    }

    /// The first device this harness publishes, proxied over `conn`.
    pub async fn device<'a>(&self, conn: &zbus::Connection) -> DeviceProxy<'a> {
        let manager = ManagerProxy::builder(conn)
            .destination(self.name.clone())
            .expect("destination")
            .build()
            .await
            .expect("manager proxy");
        let devices = manager.get_devices().await.expect("get devices");
        assert_eq!(devices.len(), 1, "one virtual device expected");
        self.device_at(conn, devices[0].clone()).await
    }

    /// A device proxy at an explicit path, for a second client onto the same device.
    pub async fn device_at<'a>(
        &self,
        conn: &zbus::Connection,
        path: OwnedObjectPath,
    ) -> DeviceProxy<'a> {
        DeviceProxy::builder(conn)
            .destination(self.name.clone())
            .expect("destination")
            .path(path)
            .expect("path")
            .build()
            .await
            .expect("device proxy")
    }
}
