// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! In-memory stand-in for a match-on-chip sensor's template storage.
//!
//! A real MOC device keeps templates on the sensor and exposes list/delete/clear over its
//! protocol; here that "sensor memory" is a `Vec<Print>` with an optional capacity, behind a
//! shared cell.
//!
//! The cell is what lets a builder model **persistence across sessions**: a real sensor keeps its
//! on-chip slots when the host closes and reopens it (the daemon rebuilds its device handle on
//! every `Claim`), so [`crate::VirtualDeviceBuilder::shared_storage`] hands every device it builds
//! a clone of one `PrintStore` — the same `Arc` — and the slots survive the rebuild. Without it
//! each built device gets its own empty store, which is right for a host sensor and for the many
//! tests that want a fresh device every time.

use std::sync::{Arc, Mutex};

use fprint_core::{Error, Print, Result, Template};

/// The device-resident set of enrolled prints, shared by clone (an `Arc`), so two device handles
/// built to share storage see one another's writes.
#[derive(Clone, Debug, Default)]
pub(crate) struct PrintStore {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug, Default)]
struct Inner {
    prints: Vec<Print>,
    /// `None` ⇒ unbounded (host sensors); `Some(n)` ⇒ a MOC sensor holding at most `n`.
    capacity: Option<usize>,
}

impl PrintStore {
    /// A fresh, empty store with the given capacity. Not shared with any other store.
    pub(crate) fn new(capacity: Option<usize>) -> Self {
        PrintStore {
            inner: Arc::new(Mutex::new(Inner {
                prints: Vec::new(),
                capacity,
            })),
        }
    }

    /// Whether a bounded store has reached its capacity.
    pub(crate) fn is_full(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.capacity.is_some_and(|cap| inner.prints.len() >= cap)
    }

    /// Whether any stored print carries this exact template (the MOC duplicate check).
    pub(crate) fn contains_template(&self, template: &Template) -> bool {
        self.inner
            .lock()
            .unwrap()
            .prints
            .iter()
            .any(|p| &p.template == template)
    }

    /// Store a print, or [`Error::DataFull`] if the sensor is out of slots.
    pub(crate) fn push(&self, print: Print) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        if inner.capacity.is_some_and(|cap| inner.prints.len() >= cap) {
            return Err(Error::DataFull);
        }
        inner.prints.push(print);
        Ok(())
    }

    /// Remove the print with this template, or [`Error::DataNotFound`] if absent.
    pub(crate) fn remove_by_template(&self, template: &Template) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        match inner.prints.iter().position(|p| &p.template == template) {
            Some(idx) => {
                inner.prints.remove(idx);
                Ok(())
            }
            None => Err(Error::DataNotFound),
        }
    }

    /// Erase all stored prints.
    pub(crate) fn clear(&self) {
        self.inner.lock().unwrap().prints.clear();
    }

    /// A snapshot of the stored prints, in insertion order.
    pub(crate) fn snapshot(&self) -> Vec<Print> {
        self.inner.lock().unwrap().prints.clone()
    }
}
