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
//! ```
//! use fprint_core::{Finger, Minutia, Print, Template};
//!
//! let print = Print::builder()
//!     .template(Template::Nbis(vec![vec![Minutia { x: 12, y: 34, theta: 90 }]]))
//!     .finger(Some(Finger::RightIndex))
//!     .username(Some("alice".into()))
//!     .build();
//!
//! let bytes: Vec<u8> = fprint_fp3::to_bytes(&print)?;
//! assert!(bytes.starts_with(fprint_fp3::MAGIC));
//! assert_eq!(fprint_fp3::from_bytes(&bytes)?, print);
//! # Ok::<(), fprint_fp3::Fp3Error>(())
//! ```
//!
//! ## What round-trips, exactly
//!
//! `from_bytes(to_bytes(p))` reproduces `p` for every [`Print`](fprint_core::Print) whose
//! `finger` is `Some` and whose `driver`/`device_id` are either `None` or non-empty. **Outside
//! that, two distinctions collapse**, because the wire cannot express them:
//!
//! * `finger: None` decodes as `Some(Finger::Unknown)` — the FP3 `y` byte has no "absent".
//! * `driver`/`device_id` of `Some("")` decode as `None` — GVariant `s` is not nullable, so the
//!   empty string is how the format spells "unset".
//!
//! `tests/property.rs` states the exact law, collapse included, and sweeps it. `to_bytes` is
//! partial for two further reasons of its own: [`Template::Undefined`](fprint_core::Template)
//! has no on-disk form, and an [`EnrollDate`](fprint_core::EnrollDate) outside the Julian day's
//! `i32` range has no encoding ([`Fp3Error::DateOutOfRange`]).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod codec;
mod date;
mod error;
mod gvariant;

pub use codec::{from_bytes, to_bytes};
pub use error::Fp3Error;

/// The FP3 container's leading magic bytes (`"FP3"`).
pub const MAGIC: &[u8; 3] = b"FP3";

/// The crate's essentials, for a single glob import.
///
/// Pulls in the two verbs and the error type: `use fprint_fp3::prelude::*;`.
pub mod prelude {
    pub use crate::{from_bytes, to_bytes, Fp3Error};
}
