// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Errors surfaced by the FP3 codec.
//!
//! `Display` and the `Error` impl come from `thiserror`. The failure modes are the ways a byte
//! stream can fail to be a well-formed FP3 template: the `"FP3"` magic, the fixed GVariant tuple
//! framing, the three `FpiPrintType` payload kinds, the per-sample "three equal-length arrays"
//! rule, and the Julian day's range.

use fprint_core::EnrollDate;

/// Crate result alias.
pub type Result<T> = core::result::Result<T, Fp3Error>;

/// Everything that can go wrong turning bytes into a [`Print`](fprint_core::Print) or back.
///
/// The variants map one-to-one to the invariants of the FP3 container documented in
/// `docs/fp3-format.md`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Fp3Error {
    /// The first three bytes were not the ASCII magic `"FP3"`.
    #[error("not an FP3 blob: bad magic")]
    BadMagic,
    /// The buffer is too short to even carry the magic.
    #[error("truncated FP3 blob: shorter than the magic")]
    Truncated,
    /// The GVariant framing was malformed: an offset, length, or terminator did not fit the
    /// bytes on hand. The static string names the specific check that failed.
    #[error("malformed GVariant framing: {0}")]
    Malformed(&'static str),
    /// The tuple's `type` field was not a known `FpiPrintType` (or was `UNDEFINED`, which
    /// is never serialized).
    #[error("unknown FpiPrintType: {0}")]
    UnknownType(i32),
    /// Asked to serialize a [`Template::Undefined`](fprint_core::Template::Undefined) print —
    /// a fresh, un-enrolled print has no on-disk representation.
    #[error("cannot serialize an undefined template")]
    UndefinedTemplate,
    /// The `v` payload did not hold the type the `type` field promised.
    #[error("payload variant did not match the print type")]
    PayloadType,
    /// An NBIS sample's `x`/`y`/`theta` arrays were not all the same length.
    #[error("NBIS sample x/y/theta arrays have unequal lengths")]
    UnevenSampleArrays,
    /// The `finger` byte was outside the `FpFinger` range (`0..=10`).
    #[error("invalid finger byte: {0}")]
    BadFinger(u8),
    /// The [`EnrollDate`](fprint_core::EnrollDate) has no FP3 Julian day: an `i32` year spans
    /// further than an `i32` count of days, so dates outside
    /// `-5879610-06-23 ..= 5879611-07-11` are unrepresentable, as is the single date whose
    /// Julian day would collide with the `G_MININT32` "unset" sentinel.
    #[error("enroll date {}-{}-{} has no FP3 Julian day", .0.year, .0.month, .0.day)]
    DateOutOfRange(EnrollDate),
}
