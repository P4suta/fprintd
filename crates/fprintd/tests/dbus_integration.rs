// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Hardware-free, end-to-end exercise of the `net.reactivated.Fprint` D-Bus surface.
//!
//! It stands the daemon up over `fprint-backend-native`'s virtual backend and an
//! [`Authorizer::AllowAll`], on a bus connection, then drives it as a real client would:
//! `GetDevices` → `Claim` → `EnrollStart` → wait for `enroll-completed` → `VerifyStart` →
//! wait for `verify-match` → `Release`. Because the enrolled print is written to disk as FP3
//! and read back for verification, this also covers the storage + [`fprint_fp3`] round-trip.
//!
//! It is fully self-contained: [`PrivateBus`] spawns its own `dbus-daemon` for the test's
//! lifetime, so a plain `cargo test` works with no ambient D-Bus or `dbus-run-session` wrapper.

#![cfg(target_os = "linux")]

use std::sync::Arc;
use std::time::Duration;

use fprint_backend_native::{
    EnrollScript, FingerId, Scenario, VirtualBackend, VirtualDeviceBuilder,
};
use fprintd::{Authorizer, Daemon, Store};
use futures_util::StreamExt;
use tokio::time::timeout;
use zbus::zvariant::OwnedObjectPath;

/// A private session bus, spawned for the duration of the test so it is self-contained (no
/// ambient `DBUS_SESSION_BUS_ADDRESS` or `dbus-run-session` wrapper required). The daemon and
/// client both connect to it; it is torn down when the guard drops.
struct PrivateBus {
    child: std::process::Child,
}

impl PrivateBus {
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
        // SAFETY-of-correctness: this is the only test in the binary, so the process-global
        // env var is not racing another test.
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
trait Manager {
    fn get_devices(&self) -> zbus::Result<Vec<OwnedObjectPath>>;
    fn get_default_device(&self) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(
    interface = "net.reactivated.Fprint.Device",
    default_service = "net.reactivated.Fprint"
)]
trait Device {
    fn claim(&self, username: &str) -> zbus::Result<()>;
    fn release(&self) -> zbus::Result<()>;
    fn enroll_start(&self, finger_name: &str) -> zbus::Result<()>;
    fn enroll_stop(&self) -> zbus::Result<()>;
    fn verify_start(&self, finger_name: &str) -> zbus::Result<()>;
    fn verify_stop(&self) -> zbus::Result<()>;
    fn list_enrolled_fingers(&self, username: &str) -> zbus::Result<Vec<String>>;

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
}

/// A virtual host-image sensor that enrolls and then recognises finger identity `2`.
fn backend() -> VirtualBackend {
    VirtualBackend::single(
        VirtualDeviceBuilder::host_image_sensor().scenario(
            Scenario::new()
                .enroll(EnrollScript::default().produces(FingerId(2)))
                .present(FingerId(2)),
        ),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enroll_then_verify_over_dbus() {
    // A private session bus, so the test needs no ambient D-Bus (self-contained under a plain
    // `cargo test`). Must outlive both connections.
    let _bus = PrivateBus::start();

    let tmp = std::env::temp_dir().join(format!("fprintd-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);

    // Stand up the daemon on the session bus. The daemon takes a backend *factory*; `backend`
    // is an `fn() -> VirtualBackend`, which is `Fn + Clone + Send + Sync + 'static`, so it
    // serves directly — each actor thread builds its own virtual device from it.
    let daemon = Daemon::with_store(
        backend,
        Arc::new(Authorizer::AllowAll),
        Store::with_root(tmp.clone()),
    );
    let builder = zbus::connection::Builder::session()
        .expect("session bus")
        .name("net.reactivated.Fprint")
        .expect("request name");
    let _daemon_conn = daemon.attach(builder).await.expect("attach daemon");

    // Client side.
    let client = zbus::Connection::session().await.expect("client session");
    let manager = ManagerProxy::new(&client).await.expect("manager proxy");

    let devices = manager.get_devices().await.expect("get devices");
    assert_eq!(devices.len(), 1, "one virtual device expected");
    let device_path = devices[0].clone();

    // Disable property caching: the daemon does not emit PropertiesChanged for
    // `num-enroll-stages` in M1, so each read must hit the wire.
    let device = DeviceProxy::builder(&client)
        .path(device_path)
        .expect("device path")
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
        .expect("device proxy");

    // Before claim, num-enroll-stages is undefined (-1).
    assert_eq!(device.num_enroll_stages().await.expect("stages"), -1);

    device.claim("").await.expect("claim");
    assert_eq!(device.num_enroll_stages().await.expect("stages"), 5);

    // Enroll, waiting for the terminal enroll-completed.
    let mut enroll_stream = device.receive_enroll_status().await.expect("enroll stream");
    device
        .enroll_start("left-index-finger")
        .await
        .expect("enroll start");
    let completed = timeout(Duration::from_secs(5), async {
        while let Some(sig) = enroll_stream.next().await {
            let args = sig.args().expect("enroll args");
            if args.done {
                return args.result.to_string();
            }
        }
        String::from("<stream ended>")
    })
    .await
    .expect("enroll timed out");
    assert_eq!(completed, "enroll-completed");
    device.enroll_stop().await.expect("enroll stop");

    // The finger is now listed.
    let fingers = device.list_enrolled_fingers("").await.expect("list");
    assert_eq!(fingers, vec!["left-index-finger".to_string()]);

    // Verify, waiting for verify-match.
    let mut verify_stream = device.receive_verify_status().await.expect("verify stream");
    device
        .verify_start("left-index-finger")
        .await
        .expect("verify start");
    let result = timeout(Duration::from_secs(5), async {
        while let Some(sig) = verify_stream.next().await {
            let args = sig.args().expect("verify args");
            if args.done {
                return args.result.to_string();
            }
        }
        String::from("<stream ended>")
    })
    .await
    .expect("verify timed out");
    assert_eq!(result, "verify-match");

    device.verify_stop().await.expect("verify stop");
    device.release().await.expect("release");

    let _ = std::fs::remove_dir_all(&tmp);
}
