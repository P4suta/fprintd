// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Shared test scaffolding: a deterministic byte source, generators over it, and an executor.
//!
//! This crate is a **dev-dependency and nothing else**. It appears in no shipped artifact, and no
//! crate may take it as a normal dependency — `cargo xtask lint` refuses that, and
//! `cargo xtask publish-check` proves it of the packaged manifests.
//!
//! ## Why it depends on nothing
//!
//! `fprint-bozorth3` and `fprint-mindtct` define their own `Minutia` rather than depend on
//! `fprint-core`, because their input — the `xyt` triple — is an interoperability fact rather than
//! a domain type. This crate draws the same line and gets the same reward: it yields
//! `(i32, i32, i32)` triples and `Vec<u8>` images, which each caller maps to its own types in a
//! line. So it can depend on nothing, and nothing it touches gains a dependency cycle with its own
//! tests. A `Print` generator would break that, and belongs in `fprint-fp3`'s tests instead.
//!
//! ## Determinism
//!
//! Every generator is driven by a [`ByteSource`]. [`Lcg`] is the one used at `cargo test` time:
//! seeded, reproducible, and printable in a failure message. A coverage-guided fuzzer supplies the
//! same generators from its own bytes by implementing the trait over its input, so one generator
//! serves both without either tool reaching into this crate.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod bytes;
pub mod exec;
pub mod gen;

pub use bytes::{ByteSource, Lcg};
pub use exec::{block_on, poll_n};
