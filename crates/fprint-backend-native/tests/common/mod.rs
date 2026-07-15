// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A dependency-free, `unsafe`-free async driver shared by the integration tests.
//!
//! We don't want a runtime crate just to poll the virtual device, so this is a minimal
//! executor built on `std::task::Wake`: a waker that unparks the blocked thread. That is
//! enough to drive the one multi-poll method (`enroll`, via its single-shot `yield_now`).

use std::future::Future;
use std::pin::{pin, Pin};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

/// A waker that resumes a parked thread. No `RawWaker`, no `unsafe`.
struct ThreadWaker(Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

fn current_thread_waker() -> Waker {
    Waker::from(Arc::new(ThreadWaker(thread::current())))
}

/// Drive a future to completion by park/unpark polling on the current thread.
pub fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let waker = current_thread_waker();
    let mut cx = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => thread::park(),
        }
    }
}

/// Poll a future at most `n` times without ever blocking.
///
/// Returns `Some(output)` if it completed within `n` polls, else `None` (still pending).
/// Used to drive an enrollment partway and then drop it, exercising cancellation.
// Only the `cancel` test binary uses this; it is dead code in the others that share this module.
#[allow(dead_code)]
pub fn poll_n<F: Future + Unpin>(future: &mut F, n: usize) -> Option<F::Output> {
    let waker = current_thread_waker();
    let mut cx = Context::from_waker(&waker);
    for _ in 0..n {
        if let Poll::Ready(value) = Pin::new(&mut *future).poll(&mut cx) {
            return Some(value);
        }
    }
    None
}
