// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! In-memory stand-in for a match-on-chip sensor's template storage.
//!
//! A real MOC device keeps templates on the sensor and exposes list/delete/clear over its
//! protocol; here that "sensor memory" is just a `Vec<Print>` with an optional capacity.
//! Host-side (image) sensors keep no on-device storage — they use a capacity-less store
//! that is never consulted.

use fprint_core::{Error, Print, Result, Template};

/// The device-resident set of enrolled prints.
#[derive(Clone, Debug, Default)]
pub(crate) struct PrintStore {
    prints: Vec<Print>,
    /// `None` ⇒ unbounded (host sensors); `Some(n)` ⇒ a MOC sensor holding at most `n`.
    capacity: Option<usize>,
}

impl PrintStore {
    /// A fresh, empty store with the given capacity.
    pub(crate) fn new(capacity: Option<usize>) -> Self {
        PrintStore {
            prints: Vec::new(),
            capacity,
        }
    }

    /// Whether a bounded store has reached its capacity.
    pub(crate) fn is_full(&self) -> bool {
        self.capacity.is_some_and(|cap| self.prints.len() >= cap)
    }

    /// Whether any stored print carries this exact template (the MOC duplicate check).
    pub(crate) fn contains_template(&self, template: &Template) -> bool {
        self.prints.iter().any(|p| &p.template == template)
    }

    /// Store a print, or [`Error::DataFull`] if the sensor is out of slots.
    pub(crate) fn push(&mut self, print: Print) -> Result<()> {
        if self.is_full() {
            return Err(Error::DataFull);
        }
        self.prints.push(print);
        Ok(())
    }

    /// Remove the print with this template, or [`Error::DataNotFound`] if absent.
    pub(crate) fn remove_by_template(&mut self, template: &Template) -> Result<()> {
        match self.prints.iter().position(|p| &p.template == template) {
            Some(idx) => {
                self.prints.remove(idx);
                Ok(())
            }
            None => Err(Error::DataNotFound),
        }
    }

    /// Erase all stored prints.
    pub(crate) fn clear(&mut self) {
        self.prints.clear();
    }

    /// A view of the stored prints, in insertion order.
    pub(crate) fn as_slice(&self) -> &[Print] {
        &self.prints
    }
}
