// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The per-device worker thread.
//!
//! libfprint's `FpContext`/`FpDevice` are `!Send` glib objects, and its only entry points are the
//! blocking `*_sync` calls, which park the calling thread inside `g_main_context_iteration`. This
//! thread is that parked thread: it owns the objects and runs every `*_sync`, so the operation
//! future on the caller thread merely waits on a `Send` reply channel. Because the device never
//! leaves this thread, only `Send` values ([`Job`] inputs, a [`gio::Cancellable`], the domain
//! results) cross the boundary — and a dropped future fires the cancellable from the caller thread
//! to wake the parked `*_sync` here (core principle 4).
//!
//! Jobs are serviced strictly one at a time, matching libfprint's single-in-flight contract.

use std::os::raw::c_void;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use fprint_core::{
    DeviceId, DeviceInfo, EnrollProgress, Error, FingerStatus, IdentifyOutcome, Print, Result,
    Temperature, VerifyOutcome,
};
use futures_channel::mpsc::UnboundedSender;
use futures_channel::oneshot;
use gio::Cancellable;

use crate::ffi::{FpContext, FpDevice, FpPrint};
use crate::progress::{on_enroll_progress, on_match_status, EnrollSink, StatusSink};
use crate::{convert, print};

/// One unit of work for the worker. Each carries its inputs, the [`Cancellable`] the caller fires
/// on drop, a oneshot reply, and — for the streaming operations — a progress sender. Every field
/// is `Send`; the `!Send` device stays on the worker.
pub(crate) enum Job {
    Open {
        cancel: Cancellable,
        reply: oneshot::Sender<Result<DeviceInfo>>,
    },
    Close {
        cancel: Cancellable,
        reply: oneshot::Sender<Result<()>>,
    },
    Enroll {
        template: Print,
        cancel: Cancellable,
        progress: UnboundedSender<EnrollProgress>,
        reply: oneshot::Sender<Result<Print>>,
    },
    Verify {
        enrolled: Print,
        cancel: Cancellable,
        status: UnboundedSender<FingerStatus>,
        reply: oneshot::Sender<Result<VerifyOutcome>>,
    },
    Identify {
        gallery: Vec<Print>,
        cancel: Cancellable,
        status: UnboundedSender<FingerStatus>,
        reply: oneshot::Sender<Result<IdentifyOutcome>>,
    },
    ListPrints {
        cancel: Cancellable,
        reply: oneshot::Sender<Result<Vec<Print>>>,
    },
    DeletePrint {
        print: Print,
        cancel: Cancellable,
        reply: oneshot::Sender<Result<()>>,
    },
    ClearStorage {
        cancel: Cancellable,
        reply: oneshot::Sender<Result<()>>,
    },
    Suspend {
        cancel: Cancellable,
        reply: oneshot::Sender<Result<()>>,
    },
    Resume {
        cancel: Cancellable,
        reply: oneshot::Sender<Result<()>>,
    },
    /// Sent by the handle's `Drop`: release the sensor and end the loop.
    Shutdown,
}

/// The worker's serial loop. It builds its own `FpContext` (the objects are thread-affine, so it
/// cannot borrow the caller's), re-finds the device by id, then services one [`Job`] at a time
/// until told to shut down or the handle drops its sender. The sensor is closed here, on this
/// thread, before it returns.
///
/// `temperature` is the cell the handle's live [`Device::temperature`](fprint_core::Device::temperature)
/// getter reads: after each job that has an open device, the worker samples the sensor's thermal
/// state and publishes it here, so the `Send` handle can answer the getter without a round-trip.
pub(crate) fn run(id: DeviceId, jobs: Receiver<Job>, temperature: Arc<Mutex<Option<Temperature>>>) {
    let ctx = FpContext::new();
    let device = find_device(&ctx, &id);
    let dev = device.as_ref();

    for job in jobs {
        match job {
            Job::Shutdown => break,
            Job::Open { cancel, reply } => {
                let _ = reply.send(with(dev, |d| {
                    d.open_sync(Some(&cancel)).map_err(convert::from_gerror)?;
                    // Features, scan type and stage count firm up only once the device is open.
                    Ok(convert::device_info(d))
                }));
            }
            Job::Close { cancel, reply } => {
                let _ = reply.send(with(dev, |d| {
                    d.close_sync(Some(&cancel)).map_err(convert::from_gerror)
                }));
            }
            Job::Enroll {
                template,
                cancel,
                progress,
                reply,
            } => {
                let _ = reply.send(with(dev, |d| {
                    let fp = print::core_to_fp(&template, d);
                    let sink = EnrollSink {
                        tx: progress,
                        total: d.nr_enroll_stages().max(0) as u32,
                        temperature: temperature.clone(),
                    };
                    let enrolled = d
                        .enroll_sync(
                            fp,
                            Some(&cancel),
                            Some(on_enroll_progress),
                            std::ptr::addr_of!(sink) as *mut c_void,
                        )
                        .map_err(convert::from_gerror)?;
                    print::fp_to_core(&enrolled)
                }));
            }
            Job::Verify {
                enrolled,
                cancel,
                status,
                reply,
            } => {
                let _ = reply.send(with(dev, |d| {
                    let fp = print::core_to_fp_for_match(&enrolled)?;
                    let sink = StatusSink {
                        tx: status,
                        temperature: temperature.clone(),
                    };
                    let (matched, scanned) = d
                        .verify_sync(
                            &fp,
                            Some(&cancel),
                            Some(on_match_status),
                            std::ptr::addr_of!(sink) as *mut c_void,
                        )
                        .map_err(convert::from_gerror)?;

                    // Match-on-chip sensors do not surface the live scan; a `None` here means "no
                    // scan surfaced" rather than a failure.
                    Ok(VerifyOutcome::new(
                        matched,
                        scanned.and_then(|s| print::fp_to_core(&s).ok()),
                    ))
                }));
            }
            Job::Identify {
                gallery,
                cancel,
                status,
                reply,
            } => {
                let _ = reply.send(with(dev, |d| {
                    let fps = gallery
                        .iter()
                        .map(print::core_to_fp_for_match)
                        .collect::<Result<Vec<FpPrint>>>()?;
                    let sink = StatusSink {
                        tx: status,
                        temperature: temperature.clone(),
                    };
                    let (matched, scanned) = d
                        .identify_sync(
                            &fps,
                            Some(&cancel),
                            Some(on_match_status),
                            std::ptr::addr_of!(sink) as *mut c_void,
                        )
                        .map_err(convert::from_gerror)?;

                    // Recover the gallery index by comparing the matched print's serialization to
                    // each candidate's — libfprint hands back the matching `FpPrint`, not its index.
                    let match_index = match matched {
                        Some(m) => {
                            let needle = m.serialize().map_err(convert::from_gerror)?;
                            fps.iter().position(|p| {
                                p.serialize().ok().as_deref() == Some(needle.as_slice())
                            })
                        }
                        None => None,
                    };
                    Ok(IdentifyOutcome::new(
                        match_index,
                        scanned.and_then(|s| print::fp_to_core(&s).ok()),
                    ))
                }));
            }
            Job::ListPrints { cancel, reply } => {
                let _ = reply.send(with(dev, |d| {
                    d.list_prints_sync(Some(&cancel))
                        .map_err(convert::from_gerror)?
                        .iter()
                        .map(print::fp_to_core)
                        .collect()
                }));
            }
            Job::DeletePrint {
                print: stored,
                cancel,
                reply,
            } => {
                let _ = reply.send(with(dev, |d| {
                    let fp = print::core_to_fp_for_match(&stored)?;
                    d.delete_print_sync(&fp, Some(&cancel))
                        .map_err(convert::from_gerror)
                }));
            }
            Job::ClearStorage { cancel, reply } => {
                let _ = reply.send(with(dev, |d| {
                    d.clear_storage_sync(Some(&cancel))
                        .map_err(convert::from_gerror)
                }));
            }
            Job::Suspend { cancel, reply } => {
                let _ = reply.send(with(dev, |d| {
                    d.suspend_sync(Some(&cancel)).map_err(convert::from_gerror)
                }));
            }
            Job::Resume { cancel, reply } => {
                let _ = reply.send(with(dev, |d| {
                    d.resume_sync(Some(&cancel)).map_err(convert::from_gerror)
                }));
            }
        }

        // Publish the sensor's live thermal state for the handle's `Device::temperature` getter.
        // Only an open device reports it; before `open` the cell stays `None` (unknown).
        if let Some(d) = dev {
            if d.is_open() {
                *temperature.lock().unwrap() = convert::temperature(d);
            }
        }
    }

    if let Some(d) = dev {
        if d.is_open() {
            // Release the sensor before the thread — and the objects — go away.
            let _ = d.close_sync(None);
        }
    }
}

/// Run `f` against the worker's device, or report [`Error::NotFound`] if the device vanished
/// between the caller-thread enumeration and this thread re-finding it.
fn with<T>(dev: Option<&FpDevice>, f: impl FnOnce(&FpDevice) -> Result<T>) -> Result<T> {
    match dev {
        Some(d) => f(d),
        None => Err(Error::NotFound),
    }
}

/// Re-find the device by id in this thread's own context. Mirrors [`crate::LibfprintBackend`]'s
/// open-by-id lookup: match on libfprint's `device_id`, falling back to the driver id when
/// `device_id` is empty (as it is for the virtual debug devices).
fn find_device(ctx: &FpContext, id: &DeviceId) -> Option<FpDevice> {
    ctx.devices().into_iter().find(|dev| {
        let device_id = dev.device_id();
        if device_id.is_empty() {
            dev.driver() == id.as_str()
        } else {
            device_id == id.as_str()
        }
    })
}
