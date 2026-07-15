// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A dependency-free, `unsafe`-free executor shared by the crate's in-module unit tests.
//!
//! The integration tests get their driver from `tests/common`, but a few `#[cfg(test)]` modules
//! inside `src` need to poll a `FrameSource` future too (e.g. to observe capture ordering). Rather
//! than pull in a runtime, this mirrors `tests/common`: a [`std::task::Wake`] waker (so no
//! `RawWaker`, no `unsafe`, honouring `#![forbid(unsafe_code)]`) driving a busy re-poll loop. The
//! only pending point in this crate's futures is `yield_now`, which resolves on the second poll, so
//! the loop terminates immediately.

#![cfg(test)]

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

/// A waker that does nothing on wake; the executor busy-polls, so no wakeup signal is needed.
struct NoopWake;

impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
    fn wake_by_ref(self: &Arc<Self>) {}
}

/// Drive a future to completion by busy-polling on the current thread.
pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(NoopWake));
    let mut cx = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
            return value;
        }
    }
}
