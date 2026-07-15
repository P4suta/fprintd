// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The `Device` object must publish the shape the sensor settled on at open, not the enumerated
//! one.
//!
//! A backend may only learn a reader's scan type, features and enroll-stage count when it opens
//! it: the libfprint shim re-reads `DeviceInfo` in `Device::open`, and
//! `fprint-backend-native` models the same split via [`DeviceShape`]. The daemon opens the
//! device on `Claim`. The `num-enroll-stages` property is already specified as `-1` until
//! claimed for this reason.
//!
//! The modelled device is a UPEK TouchStrip (`0483:2016`, driver `upekts`): a swipe reader, and
//! the only libfprint driver with 3 enroll stages rather than 5.
//!
//! Separate from `dbus_integration` because [`common::PrivateBus`] is one per test binary.

#![cfg(target_os = "linux")]

mod common;
use common::PrivateBus;

use std::sync::Arc;

use fprint_backend_native::{DeviceShape, VirtualBackend, VirtualDeviceBuilder};
use fprint_core::{DeviceFeature, ScanType};
use fprintd::{Authorizer, Daemon, Store};
use zbus::zvariant::OwnedObjectPath;

#[zbus::proxy(
    interface = "net.reactivated.Fprint.Manager",
    default_service = "net.reactivated.Fprint",
    default_path = "/net/reactivated/Fprint/Manager"
)]
trait Manager {
    fn get_devices(&self) -> zbus::Result<Vec<OwnedObjectPath>>;
}

#[zbus::proxy(
    interface = "net.reactivated.Fprint.Device",
    default_service = "net.reactivated.Fprint"
)]
trait Device {
    fn claim(&self, username: &str) -> zbus::Result<()>;
    fn release(&self) -> zbus::Result<()>;

    #[zbus(property, name = "num-enroll-stages")]
    fn num_enroll_stages(&self) -> zbus::Result<i32>;
    #[zbus(property, name = "scan-type")]
    fn scan_type(&self) -> zbus::Result<String>;
    #[zbus(property, name = "name")]
    fn name(&self) -> zbus::Result<String>;
}

/// A swipe reader whose shape is only known once it is open — the `upekts` case.
fn backend() -> VirtualBackend {
    VirtualBackend::single(
        VirtualDeviceBuilder::host_image_sensor()
            .name("UPEK TouchStrip")
            .scan_type(ScanType::Swipe)
            .enroll_stages(3)
            .features(DeviceFeature::CAPTURE | DeviceFeature::VERIFY | DeviceFeature::IDENTIFY)
            .probe_reports(DeviceShape {
                scan_type: ScanType::Press,
                features: DeviceFeature::CAPTURE,
                enroll_stages: 5,
            }),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claim_publishes_the_settled_shape() {
    let _bus = PrivateBus::start();

    let tmp = std::env::temp_dir().join(format!("fprintd-shape-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);

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

    let client = zbus::Connection::session().await.expect("client session");
    let manager = ManagerProxy::new(&client).await.expect("manager proxy");
    let devices = manager.get_devices().await.expect("get devices");
    assert_eq!(devices.len(), 1);

    // No property caching: the daemon emits no PropertiesChanged, so every read must hit the
    // wire, or this would assert against zbus's cache rather than the daemon.
    let device = DeviceProxy::builder(&client)
        .path(devices[0].clone())
        .expect("device path")
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
        .expect("device proxy");

    // Unclaimed: stages are undefined; the name does not need an open device.
    assert_eq!(device.num_enroll_stages().await.expect("stages"), -1);
    assert_eq!(device.name().await.expect("name"), "UPEK TouchStrip");

    device.claim("").await.expect("claim");

    // Claimed, so the device is open. These are the settled values, not the probed 5 / "press".
    assert_eq!(
        device.num_enroll_stages().await.expect("stages"),
        3,
        "num-enroll-stages must be the settled value, not the enumerated one"
    );
    assert_eq!(
        device.scan_type().await.expect("scan type"),
        "swipe",
        "scan-type must be the settled value, not the enumerated one"
    );

    device.release().await.expect("release");
    let _ = std::fs::remove_dir_all(&tmp);
}
