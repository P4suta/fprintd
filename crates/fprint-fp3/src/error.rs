// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Errors surfaced by the FP3 codec.
//!
//! Hand-rolled in the same spirit as `fprint-core`'s error type (no `thiserror`, no upstream
//! GVariant engine to defer to): the codec is a self-contained edge translator, so its
//! failure modes are exactly the ways a byte stream can fail to be a well-formed FP3
//! template — the `"FP3"` magic, the fixed GVariant tuple framing, the three `FpiPrintType`
//! payload kinds, and the per-sample "three equal-length arrays" rule.

/// Crate result alias.
pub type Result<T> = core::result::Result<T, Fp3Error>;

/// Everything that can go wrong turning bytes into a [`Print`](fprint_core::Print) or back.
///
/// The variants map one-to-one to the invariants of the FP3 container documented in
/// `docs/fp3-format.md`.
#[derive(Debug)]
#[non_exhaustive]
pub enum Fp3Error {
    /// The first three bytes were not the ASCII magic `"FP3"`.
    BadMagic,
    /// The buffer is too short to even carry the magic.
    Truncated,
    /// The GVariant framing was malformed: an offset, length, or terminator did not fit the
    /// bytes on hand. The static string names the specific check that failed.
    Malformed(&'static str),
    /// The tuple's `type` field was not a known `FpiPrintType` (or was `UNDEFINED`, which
    /// is never serialized).
    UnknownType(i32),
    /// Asked to serialize a [`Template::Undefined`](fprint_core::Template::Undefined) print —
    /// a fresh, un-enrolled print has no on-disk representation.
    UndefinedTemplate,
    /// The `v` payload did not hold the type the `type` field promised.
    PayloadType,
    /// An NBIS sample's `x`/`y`/`theta` arrays were not all the same length.
    UnevenSampleArrays,
    /// The `finger` byte was outside the `FpFinger` range (`0..=10`).
    BadFinger(u8),
}

impl core::fmt::Display for Fp3Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Fp3Error::BadMagic => f.write_str("not an FP3 blob: bad magic"),
            Fp3Error::Truncated => f.write_str("truncated FP3 blob: shorter than the magic"),
            Fp3Error::Malformed(what) => write!(f, "malformed GVariant framing: {what}"),
            Fp3Error::UnknownType(t) => write!(f, "unknown FpiPrintType: {t}"),
            Fp3Error::UndefinedTemplate => f.write_str("cannot serialize an undefined template"),
            Fp3Error::PayloadType => f.write_str("payload variant did not match the print type"),
            Fp3Error::UnevenSampleArrays => {
                f.write_str("NBIS sample x/y/theta arrays have unequal lengths")
            }
            Fp3Error::BadFinger(b) => write!(f, "invalid finger byte: {b}"),
        }
    }
}

impl std::error::Error for Fp3Error {}
