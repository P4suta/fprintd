// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! # fprint-core
//!
//! The GObject-free core of an fprintd-compatible fingerprint stack: the domain model
//! (fingers, device capabilities, prints/templates) plus the [`Backend`]/[`Device`] traits
//! that a concrete backend implements.
//!
//! It contains **no** device drivers, **no** USB code, and **no** matching algorithms —
//! those live in downstream crates:
//!
//! * `fprint-backend-libfprint` implements [`Backend`] by wrapping the C libfprint, and
//! * `fprint-backend-native` implements the same trait with pure-Rust drivers + transport.
//!
//! The fprintd-compatible daemon (`fprintd`) depends only on these traits, so the backend
//! can be swapped without touching the daemon.
//!
//! Enum *values* that cross a wire boundary (the FP3 template format, fprintd's per-finger
//! file names) mirror libfprint's C enums exactly — see [`Finger`] — so the stack stays
//! interoperable with existing `/var/lib/fprint` stores. The device-capability enums
//! ([`DeviceFeature`], [`ScanType`]) mirror their libfprint counterparts too. The wire
//! *vocabularies* themselves (the `net.reactivated.Fprint` finger-name and status strings)
//! live at the daemon edge, not here (`ARCHITECTURE.md` principle 3).
//!
//! # Implementing a backend
//!
//! A backend is a type that hands out [`Device`]s. Both traits are native `async fn` traits with
//! no `dyn` and no runtime, so **a whole backend is a struct and two impl blocks** — this one is
//! compiled and run as a doctest, depending on nothing but `std`.
//!
//! ```
//! # use std::future::Future;
//! # use std::sync::Arc;
//! # use std::task::{Context, Poll, Wake, Waker};
//! # struct Noop;
//! # impl Wake for Noop {
//! #     fn wake(self: Arc<Self>) {}
//! # }
//! # /// Poll a future to completion on this thread; the futures below never pend.
//! # fn block_on<F: Future>(future: F) -> F::Output {
//! #     let mut future = Box::pin(future);
//! #     let waker = Waker::from(Arc::new(Noop));
//! #     let mut cx = Context::from_waker(&waker);
//! #     loop {
//! #         match future.as_mut().poll(&mut cx) {
//! #             Poll::Ready(value) => return value,
//! #             Poll::Pending => unreachable!("this example never pends"),
//! #         }
//! #     }
//! # }
//! use fprint_core::{
//!     Backend, Device, DeviceFeature, DeviceId, DeviceInfo, DriverId, EnrollProgress, Error,
//!     Finger, IdentifyOutcome, Print, Result, ScanType, Template, VerifyOutcome,
//! };
//!
//! /// A match-on-chip reader holding one template.
//! struct Demo {
//!     info: DeviceInfo,
//!     stored: Option<Print>,
//! }
//!
//! impl Device for Demo {
//!     fn info(&self) -> &DeviceInfo {
//!         &self.info
//!     }
//!
//!     async fn enroll<F: FnMut(EnrollProgress)>(
//!         &mut self,
//!         mut template: Print,
//!         mut on_progress: F,
//!     ) -> Result<Print> {
//!         let total_stages = self.info.enroll_stages;
//!         for completed_stages in 1..=total_stages {
//!             on_progress(EnrollProgress::new(completed_stages, total_stages));
//!         }
//!         template.template = Template::Raw(b"on-sensor handle".to_vec());
//!         template.driver = Some(self.info.driver.clone());
//!         template.device_stored = true;
//!         self.stored = Some(template.clone());
//!         Ok(template)
//!     }
//!
//!     async fn verify(&mut self, enrolled: &Print) -> Result<VerifyOutcome> {
//!         Ok(VerifyOutcome::new(self.stored.as_ref() == Some(enrolled), None))
//!     }
//!
//!     // The remaining operations are stubs here; a real backend talks to the sensor.
//! #   async fn open(&mut self) -> Result<()> { Ok(()) }
//! #   async fn close(&mut self) -> Result<()> { Ok(()) }
//! #   async fn identify(&mut self, _gallery: &[Print]) -> Result<IdentifyOutcome> {
//! #       Err(Error::NotSupported)
//! #   }
//! #   async fn list_prints(&mut self) -> Result<Vec<Print>> {
//! #       Ok(self.stored.clone().into_iter().collect())
//! #   }
//! #   async fn delete_print(&mut self, _print: &Print) -> Result<()> { self.stored = None; Ok(()) }
//! #   async fn clear_storage(&mut self) -> Result<()> { self.stored = None; Ok(()) }
//! #   async fn suspend(&mut self) -> Result<()> { Ok(()) }
//! #   async fn resume(&mut self) -> Result<()> { Ok(()) }
//! }
//!
//! struct DemoBackend;
//!
//! impl Backend for DemoBackend {
//!     type Device = Demo;
//!
//!     async fn enumerate(&self) -> Result<Vec<Demo>> {
//!         Ok(vec![Demo {
//!             info: DeviceInfo::new(
//!                 DeviceId::new("demo-0"),
//!                 DriverId::new("demo"),
//!                 "Demo Reader",
//!                 ScanType::Press,
//!                 DeviceFeature::VERIFY | DeviceFeature::STORAGE,
//!                 3,
//!             ),
//!             stored: None,
//!         }])
//!     }
//!
//!     async fn open(&self, id: &DeviceId) -> Result<Demo> {
//!         self.enumerate()
//!             .await?
//!             .into_iter()
//!             .find(|d| &d.info().id == id)
//!             .ok_or(Error::NotFound)
//!     }
//! }
//!
//! # fn main() {
//! block_on(async {
//!     let mut device = DemoBackend.open(&DeviceId::new("demo-0")).await.unwrap();
//!     device.open().await.unwrap();
//!     assert!(device.has_feature(DeviceFeature::STORAGE));
//!     assert!(device.info().features.is_match_on_chip());
//!
//!     let mut seen = Vec::new();
//!     let print = device
//!         .enroll(Print::new_for_enroll(Finger::RightIndex), |p| seen.push(p.completed_stages))
//!         .await
//!         .unwrap();
//!
//!     assert_eq!(seen, [1, 2, 3]);
//!     assert_eq!(print.finger, Some(Finger::RightIndex));
//!     assert!(print.is_compatible_with_driver(&DriverId::new("demo")));
//!     assert!(device.verify(&print).await.unwrap().matched);
//! });
//! # }
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod device;
mod error;
mod feature;
mod finger;
mod print;

pub use device::{
    Backend, Device, DeviceId, DeviceInfo, DriverId, EnrollProgress, IdentifyOutcome, VerifyOutcome,
};
pub use error::{Error, Result, RetryReason};
pub use feature::{DeviceFeature, FingerStatus, ScanType, Temperature};
pub use finger::Finger;
pub use print::{EnrollDate, Minutia, Print, PrintBuilder, Template};

/// The names a backend or client reaches for most: the traits, the model, and the result types.
///
/// ```
/// use fprint_core::prelude::*;
/// ```
pub mod prelude {
    pub use crate::{
        Backend, Device, DeviceFeature, DeviceId, DeviceInfo, DriverId, EnrollProgress, Error,
        Finger, IdentifyOutcome, Minutia, Print, PrintBuilder, Result, ScanType, Template,
        VerifyOutcome,
    };
}
