// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Driving a future to completion on the calling thread.
//!
//! No runtime crate. `fprint-core`'s traits are native `async fn`, so a test needs somewhere to
//! poll them and nothing more — an executor with a queue, a reactor and a timer would be a large
//! dependency bought to call `poll` in a loop.
//!
//! It parks between polls rather than spinning. A busy-poll happens to work against a future that
//! wakes itself before returning `Pending`, and burns a core forever against one that does not.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

/// Unparks the thread that is blocked on the future.
struct ThreadWaker(Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

/// Poll `future` on this thread until it completes, parking while it is pending.
pub fn block_on<F: Future>(future: F) -> F::Output {
    // Safety is by construction, not by `unsafe`: the future stays on this stack frame and is
    // never moved after this point.
    let mut future = Box::pin(future);
    let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => thread::park(),
        }
    }
}

/// Poll `future` at most `n` times, never blocking. `None` if it is still pending after that.
///
/// The tool for testing that cancellation is dropping the future (ARCHITECTURE.md principle 4):
/// drive an operation partway, drop it, and observe that the backend released the sensor. That
/// needs a poll that *stops*, which [`block_on`] by definition does not offer.
pub fn poll_n<F: Future + Unpin>(future: &mut F, n: usize) -> Option<F::Output> {
    let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
    let mut cx = Context::from_waker(&waker);
    for _ in 0..n {
        if let Poll::Ready(value) = Pin::new(&mut *future).poll(&mut cx) {
            return Some(value);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn a_ready_future_returns_its_value() {
        assert_eq!(block_on(async { 7 }), 7);
    }

    #[test]
    fn a_future_that_yields_once_still_completes() {
        // Pending on the first poll, ready on the second: the park/unpark path, not the fast one.
        struct YieldOnce(bool);
        impl Future for YieldOnce {
            type Output = u8;
            fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u8> {
                if self.0 {
                    Poll::Ready(1)
                } else {
                    self.0 = true;
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            }
        }
        assert_eq!(block_on(YieldOnce(false)), 1);
    }

    #[test]
    fn a_future_woken_from_another_thread_completes() {
        // The property a busy-poll cannot have: nothing wakes this future from inside its own
        // poll, so an executor that spins would never observe the wake and would never yield the
        // core it is spinning on.
        static POLLS: AtomicUsize = AtomicUsize::new(0);
        struct WokenLater(std::sync::mpsc::Receiver<()>);
        impl Future for WokenLater {
            type Output = ();
            fn poll(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
                POLLS.fetch_add(1, Ordering::SeqCst);
                match self.0.try_recv() {
                    Ok(()) => Poll::Ready(()),
                    Err(_) => {
                        let waker = cx.waker().clone();
                        thread::spawn(move || waker.wake());
                        Poll::Pending
                    }
                }
            }
        }
        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            thread::sleep(std::time::Duration::from_millis(10));
            let _ = tx.send(());
        });
        block_on(WokenLater(rx));
        assert!(POLLS.load(Ordering::SeqCst) >= 1);
    }

    #[test]
    fn poll_n_returns_none_while_the_future_is_still_pending() {
        struct Never;
        impl Future for Never {
            type Output = ();
            fn poll(self: std::pin::Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
                Poll::Pending
            }
        }
        // The property `block_on` cannot have: it stops. A cancellation test drives an operation
        // partway and drops it, which needs exactly this.
        assert_eq!(poll_n(&mut Never, 4), None);
    }

    #[test]
    fn poll_n_returns_the_value_once_the_future_completes() {
        struct ReadyOn(usize);
        impl Future for ReadyOn {
            type Output = usize;
            fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<usize> {
                if self.0 == 0 {
                    return Poll::Ready(7);
                }
                self.0 -= 1;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
        assert_eq!(poll_n(&mut ReadyOn(2), 3), Some(7));
        // One poll short is `None`, so the count is the bound it claims to be.
        assert_eq!(poll_n(&mut ReadyOn(2), 2), None);
    }
}
