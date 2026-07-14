// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A dependency-free, `unsafe`-free async driver for the integration tests.
//!
//! Same shape as `fp-backend-native`'s test helper: a minimal executor built on
//! `std::task::Wake` (a waker that unparks the blocked thread). No runtime crate, no
//! `RawWaker`, no `unsafe` — enough to drive the composite device's one multi-poll method
//! (`enroll`, via the native device's single-shot `yield_now`).


use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

/// A waker that resumes a parked thread.
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
