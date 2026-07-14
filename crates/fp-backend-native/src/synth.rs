// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Deterministic, **non-biometric** template synthesis.
//!
//! # THIS IS A TEST STUB — NOT BIOMETRICS
//!
//! There is no fingerprint image, no MINDTCT, no BOZORTH3, and no matching algorithm here.
//! A [`FingerId`] is mapped to a *fixed* [`Template`] by a trivial encoding, and "matching"
//! is byte equality of two templates. This exists solely so the offline virtual device can
//! exercise fp-core's enroll/verify/identify seam and the fprintd daemon on top of it, on
//! any platform, with zero hardware. Do not mistake any of this for identity verification.
//!
//! The two encodings mirror the two real [`Template`] shapes so downstream code sees the
//! same variants it would from a real driver:
//! * [`TemplateKind::Nbis`] — host-side minutiae (image sensors), one synthetic minutia.
//! * [`TemplateKind::Raw`] — an opaque device blob (match-on-chip sensors).

use crate::scenario::FingerId;
use fp_core::{Minutia, Template};

/// Which [`Template`] shape a virtual device produces.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TemplateKind {
    /// `Template::Nbis` — as a host-side image sensor would enroll.
    Nbis,
    /// `Template::Raw` — as a match-on-chip sensor's opaque handle would look.
    Raw,
}

/// Deterministically map a [`FingerId`] to a template of the requested shape.
///
/// The encoding is intentionally boring and total: equal ids give equal bytes.
pub(crate) fn template_for(kind: TemplateKind, id: FingerId) -> Template {
    match kind {
        TemplateKind::Raw => {
            // An opaque, stable device blob: "VIRT" magic + the id as little-endian bytes.
            //
            // A real match-on-chip driver hands libfprint its `print->data` as a
            // self-describing GVariant, and the FP3 format therefore defines
            // `Template::Raw` as *the opaque serialized variant* (see `docs/fp3-format.md`
            // §RAW). To honour that stack-wide contract — so the enrolled print survives an
            // FP3 round-trip — this stub shapes its blob as the serialized standalone
            // GVariant variant of a byte array (`v` holding `ay`). In GVariant normal form
            // that is simply the array bytes, a `0x00` separator, then the ASCII type
            // signature `"ay"`; no serialization crate (and no knowledge of the FP3
            // *container* format) is needed to emit it.
            let mut blob = Vec::with_capacity(4 + core::mem::size_of::<u64>() + 3);
            blob.extend_from_slice(b"VIRT");
            blob.extend_from_slice(&id.0.to_le_bytes());
            blob.push(0x00);
            blob.extend_from_slice(b"ay");
            Template::Raw(blob)
        }
        TemplateKind::Nbis => {
            // One synthetic "minutia" carrying the id in its coordinates. One enrolled
            // sample (the outer Vec) with one point (the inner Vec).
            Template::Nbis(vec![vec![Minutia {
                x: (id.0 & 0xffff_ffff) as i32,
                y: (id.0 >> 32) as i32,
                theta: 0,
            }]])
        }
    }
}

/// "Match" two templates. **Byte equality only — not a biometric comparison.**
pub(crate) fn matches(enrolled: &Template, scanned: &Template) -> bool {
    enrolled == scanned
}
