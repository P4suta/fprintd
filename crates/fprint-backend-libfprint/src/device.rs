// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! [`LibfprintDevice`]: an `fprint-core` [`Device`] backed by a C-libfprint `FpDevice`.
//!
//! The `FpDevice` is `!Send` and every libfprint entry point is a blocking `*_sync` call, so the
//! device lives on a dedicated [worker thread](crate::worker) and this type is only a `Send`
//! handle to it: a channel of [`Job`]s plus the cached [`DeviceInfo`]. Each operation submits a
//! job carrying a fresh [`gio::Cancellable`] and awaits the worker's reply; the [`CancelOnDrop`]
//! guard held across that await fires the cancellable if the future is dropped, cancelling the
//! `*_sync` on the worker cross-thread (core principle 4). One operation at a time is guaranteed
//! by `&mut self`, matching libfprint's single-in-flight contract.

use std::future::Future;
use std::pin::Pin;
use std::sync::mpsc::Sender;
use std::task::Poll;
use std::thread::JoinHandle;

use fprint_core::{
    Device, DeviceFeature, DeviceInfo, EnrollProgress, Error, FingerStatus, IdentifyOutcome, Print,
    Result, VerifyOutcome,
};
use futures_channel::mpsc::{self, UnboundedSender};
use futures_channel::oneshot;
use futures_core::Stream;
use gio::prelude::CancellableExt;
use gio::Cancellable;

use crate::worker::{self, Job};

/// A fingerprint reader driven through the C libfprint.
///
/// Obtained from [`crate::LibfprintBackend`]; call [`Device::open`] before any operation. The
/// reader itself lives on an internal worker thread — this handle is `Send`, and dropping it
/// releases the sensor and joins that thread.
pub struct LibfprintDevice {
    jobs: Sender<Job>,
    info: DeviceInfo,
    worker: Option<JoinHandle<()>>,
}

impl LibfprintDevice {
    /// Spawn the worker for the device described by `info` and return a handle to it. The worker
    /// re-finds the device by `info.id` in its own context; `info` is the caller-thread getter
    /// snapshot, refreshed from the worker on [`Device::open`].
    pub(crate) fn spawn(info: DeviceInfo) -> Self {
        let (jobs, rx) = std::sync::mpsc::channel();
        let id = info.id.clone();
        let worker = std::thread::Builder::new()
            .name(format!("libfprint-worker-{}", id.as_str()))
            .spawn(move || worker::run(id, rx))
            .expect("spawn libfprint worker thread");
        LibfprintDevice {
            jobs,
            info,
            worker: Some(worker),
        }
    }

    /// Send a job to the worker. Fails only if the worker thread has already stopped.
    fn submit(&self, job: Job) -> Result<()> {
        self.jobs
            .send(job)
            .map_err(|_| Error::Other("libfprint worker thread stopped".into()))
    }

    /// Build a fresh cancellable and its drop-guard, open a reply channel, and submit the job the
    /// caller shapes from those. The returned guard must be held across the await so a dropped
    /// future cancels the in-flight `*_sync`.
    fn start<T>(
        &self,
        job: impl FnOnce(Cancellable, oneshot::Sender<Result<T>>) -> Job,
    ) -> Result<(CancelOnDrop, oneshot::Receiver<Result<T>>)> {
        let cancel = Cancellable::new();
        let guard = CancelOnDrop(cancel.clone());
        let (reply_tx, reply_rx) = oneshot::channel();
        self.submit(job(cancel, reply_tx))?;
        Ok((guard, reply_rx))
    }

    /// [`start`](Self::start) for a streaming operation, adding the progress channel the worker
    /// pushes reports to.
    fn start_streaming<T, P>(
        &self,
        job: impl FnOnce(Cancellable, UnboundedSender<P>, oneshot::Sender<Result<T>>) -> Job,
    ) -> Result<(
        CancelOnDrop,
        oneshot::Receiver<Result<T>>,
        mpsc::UnboundedReceiver<P>,
    )> {
        let cancel = Cancellable::new();
        let guard = CancelOnDrop(cancel.clone());
        let (event_tx, event_rx) = mpsc::unbounded();
        let (reply_tx, reply_rx) = oneshot::channel();
        self.submit(job(cancel, event_tx, reply_tx))?;
        Ok((guard, reply_rx, event_rx))
    }
}

impl Drop for LibfprintDevice {
    fn drop(&mut self) {
        // Release the sensor on the worker, then wait for it to finish so the reader is free
        // before the process moves on. Errors mean the worker already stopped.
        let _ = self.jobs.send(Job::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Fires its cancellable when dropped. Held across an operation's await so that dropping the
/// operation future cancels the in-flight `*_sync` on the worker. Fire-and-detach: it never joins
/// — the worker's `*_sync` returns `Cancelled` and the worker moves on to the next job.
struct CancelOnDrop(Cancellable);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// Await a job's reply. A dropped reply sender (the worker was torn down) reads as cancellation.
async fn await_reply<T>(reply: oneshot::Receiver<Result<T>>) -> Result<T> {
    match reply.await {
        Ok(outcome) => outcome,
        Err(oneshot::Canceled) => Err(Error::Cancelled),
    }
}

/// Await a streaming job's reply while draining its progress channel, invoking `on_event` on this
/// (caller) thread as each report arrives — delivering every report exactly once.
///
/// The worker pushes every progress report from inside the `*_sync` call and sends the reply only
/// after it returns, so once the reply is observable the channel holds all remaining reports. Each
/// wake drains what has arrived before checking the reply (live progress), and a final drain once
/// the reply is here catches the last report, which would otherwise race the reply and be lost.
async fn await_streaming<T, P>(
    mut reply: oneshot::Receiver<Result<T>>,
    mut events: mpsc::UnboundedReceiver<P>,
    mut on_event: impl FnMut(P),
) -> Result<T> {
    std::future::poll_fn(move |cx| {
        // Deliver what has arrived so far. Stops on `Pending` (nothing more yet) or `None` (the
        // worker dropped the sender when the `*_sync` returned).
        while let Poll::Ready(Some(event)) = Pin::new(&mut events).poll_next(cx) {
            on_event(event);
        }
        match Pin::new(&mut reply).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(reply) => {
                // The reply lands after every report was pushed, so the channel now holds any that
                // arrived in the gap above; drain them before finishing.
                while let Poll::Ready(Some(event)) = Pin::new(&mut events).poll_next(cx) {
                    on_event(event);
                }
                Poll::Ready(match reply {
                    Ok(outcome) => outcome,
                    Err(oneshot::Canceled) => Err(Error::Cancelled),
                })
            }
        }
    })
    .await
}

impl Device for LibfprintDevice {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    async fn open(&mut self) -> Result<()> {
        let (_guard, reply) = self.start(|cancel, reply| Job::Open { cancel, reply })?;
        self.info = await_reply(reply).await?;
        Ok(())
    }

    async fn close(&mut self) -> Result<()> {
        let (_guard, reply) = self.start(|cancel, reply| Job::Close { cancel, reply })?;
        await_reply(reply).await
    }

    async fn enroll<F: FnMut(EnrollProgress)>(
        &mut self,
        template: Print,
        on_progress: F,
    ) -> Result<Print> {
        let (_guard, reply, progress) =
            self.start_streaming(|cancel, progress, reply| Job::Enroll {
                template,
                cancel,
                progress,
                reply,
            })?;
        await_streaming(reply, progress, on_progress).await
    }

    async fn verify_with_status<F: FnMut(FingerStatus)>(
        &mut self,
        enrolled: &Print,
        on_status: F,
    ) -> Result<VerifyOutcome> {
        let enrolled = enrolled.clone();
        let (_guard, reply, status) =
            self.start_streaming(|cancel, status, reply| Job::Verify {
                enrolled,
                cancel,
                status,
                reply,
            })?;
        await_streaming(reply, status, on_status).await
    }

    async fn identify_with_status<F: FnMut(FingerStatus)>(
        &mut self,
        gallery: &[Print],
        on_status: F,
    ) -> Result<IdentifyOutcome> {
        let gallery = gallery.to_vec();
        let (_guard, reply, status) =
            self.start_streaming(|cancel, status, reply| Job::Identify {
                gallery,
                cancel,
                status,
                reply,
            })?;
        await_streaming(reply, status, on_status).await
    }

    async fn list_prints(&mut self) -> Result<Vec<Print>> {
        if !self.has_feature(DeviceFeature::STORAGE_LIST) {
            return Err(Error::NotSupported);
        }
        let (_guard, reply) = self.start(|cancel, reply| Job::ListPrints { cancel, reply })?;
        await_reply(reply).await
    }

    async fn delete_print(&mut self, stored: &Print) -> Result<()> {
        if !self.has_feature(DeviceFeature::STORAGE_DELETE) {
            return Err(Error::NotSupported);
        }
        let print = stored.clone();
        let (_guard, reply) = self.start(|cancel, reply| Job::DeletePrint {
            print,
            cancel,
            reply,
        })?;
        await_reply(reply).await
    }

    async fn clear_storage(&mut self) -> Result<()> {
        if !self.has_feature(DeviceFeature::STORAGE_CLEAR) {
            return Err(Error::NotSupported);
        }
        let (_guard, reply) = self.start(|cancel, reply| Job::ClearStorage { cancel, reply })?;
        await_reply(reply).await
    }

    async fn suspend(&mut self) -> Result<()> {
        let (_guard, reply) = self.start(|cancel, reply| Job::Suspend { cancel, reply })?;
        await_reply(reply).await
    }

    async fn resume(&mut self) -> Result<()> {
        let (_guard, reply) = self.start(|cancel, reply| Job::Resume { cancel, reply })?;
        await_reply(reply).await
    }
}
