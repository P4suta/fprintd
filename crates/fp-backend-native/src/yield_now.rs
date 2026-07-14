// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A dependency-free, single-shot cooperative yield.
//!
//! The virtual device needs no async runtime: every trait method is straight-line and
//! resolves on its first poll — except [`crate::VirtualDevice::enroll`], which must span
//! several polls so that dropping the future mid-enrollment cancels the operation (the
//! project's cancellation model, see `ARCHITECTURE.md` principle 4). To model "one capture
//! stage per poll" without pulling in `tokio`/`futures`, enroll awaits [`yield_now`] once
//! per stage.
//!
//! [`YieldNow`] is deliberately `Unpin` (its only field is a `bool`), so its `poll` can
//! mutate through `Pin<&mut Self>` with no `unsafe` — honouring the crate's
//! `#![forbid(unsafe_code)]`.

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

/// A future that returns `Pending` exactly once, then `Ready`.
///
/// The first poll records that it has yielded, re-arms the waker (so a runtime immediately
/// re-polls), and returns `Pending`; the second poll returns `Ready(())`.
pub(crate) struct YieldNow(bool);

impl Future for YieldNow {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            Poll::Ready(())
        } else {
            // `Self: Unpin`, so this field write through `Pin<&mut Self>` needs no `unsafe`.
            self.0 = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

/// Yield control back to the poller exactly once. See the module docs.
pub(crate) fn yield_now() -> YieldNow {
    YieldNow(false)
}
