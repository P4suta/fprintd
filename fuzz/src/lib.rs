// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The bridge from a fuzzer's input to `fprint-testkit`'s generators.
//!
//! `fprint_testkit::ByteSource` exists so one set of generators serves both `cargo test` (driven
//! by `Lcg`) and a coverage-guided fuzzer (driven by its input). The adapter lives here rather
//! than in the testkit because the orphan rule puts it here — **which is the right side**: the
//! testkit depends on nothing, and `arbitrary` stays outside the published tree.

#![forbid(unsafe_code)]

use arbitrary::Unstructured;
use fprint_testkit::ByteSource;

/// An [`Unstructured`] seen as a [`ByteSource`].
pub struct Bytes<'a, 'u>(pub &'a mut Unstructured<'u>);

impl ByteSource for Bytes<'_, '_> {
    fn fill(&mut self, buf: &mut [u8]) {
        // `fill_buffer` zero-pads once the input runs out and cannot fail, which is exactly the
        // contract `ByteSource::fill` states: a finite source pads rather than leaving `buf` short,
        // so a generator's shape never depends on how much input is left.
        let _ = self.0.fill_buffer(buf);
    }
}
