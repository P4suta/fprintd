// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! # fprint-fp3
//!
//! The FP3 fingerprint-template codec: the edge translator between libfprint/fprintd's
//! on-disk `"FP3"` blob and the [`fprint_core::Print`] domain model.
//!
//! An FP3 blob is the three ASCII bytes `"FP3"` followed by a little-endian, normal-form
//! GVariant value of type `(issbymsmsia{sv}v)` (see `docs/fp3-format.md`). Everything
//! wire-specific — the magic, the GVariant signature, the Julian-day dates with their
//! `G_MININT32` sentinel, the maybe-strings, the NBIS `(a(aiaiai))` payload — lives here
//! and never leaks up into `fprint-core` (`ARCHITECTURE.md` principle 3).
//!
//! The public surface is two verbs:
//!
//! ```no_run
//! use fprint_core::Print;
//! # fn demo(print: &Print, blob: &[u8]) -> fprint_fp3::Result<()> {
//! let bytes: Vec<u8> = fprint_fp3::to_bytes(print)?;
//! let print: Print = fprint_fp3::from_bytes(blob)?;
//! # let _ = bytes; let _ = print; Ok(())
//! # }
//! ```
//!
//! For every [`Print`](fprint_core::Print) this crate can serialize, `from_bytes(to_bytes(p))`
//! reproduces `p` exactly.

#![forbid(unsafe_code)]

mod codec;
mod date;
mod error;
mod gvariant;

pub use codec::{from_bytes, to_bytes};
pub use error::{Fp3Error, Result};

/// The FP3 container's leading magic bytes (`"FP3"`).
pub const MAGIC: &[u8; 3] = b"FP3";
