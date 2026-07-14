// SPDX-FileCopyrightText: 2026 libfprint-rs (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A minimal, hand-rolled GVariant serializer/parser — only the type-shapes the FP3
//! container needs, laid out in GVariant *normal form*, little-endian.
//!
//! GVariant packs values without a framing byte per value: sizes are recovered from
//! alignment plus a table of *framing offsets* appended to each variable-width container.
//! This module implements exactly that, for the handful of shapes FP3 uses — fixed scalars
//! (`i`/`y`/`b`), strings (`s`), maybe-strings (`ms`), fixed-element arrays (`ai`),
//! variable-element arrays (`a(aiaiai)`), tuples, and variants (`v`) — and nothing more.
//! There is no general type-driven engine here; the shapes are named directly by the
//! codec above.
//!
//! ## Writing
//!
//! A [`Val`] is a fully serialized value carrying its alignment and whether its type is
//! fixed-width (the two facts a parent container needs to place it and to decide framing).
//! Values compose bottom-up — [`tuple`]/[`array`] take their children and emit the padded
//! body plus the framing-offset table.
//!
//! ## Reading
//!
//! The reader is a set of bounds-checked extractors. [`walk_tuple`] splits a tuple's bytes
//! into its member slices given a static description of the members; the scalar/string/array
//! readers turn a member slice into a value. Every access is checked and yields
//! [`Fp3Error::Malformed`] rather than panicking.

use crate::error::{Fp3Error, Result};

// ---- framing arithmetic ---------------------------------------------------------------

/// Round `pos` up to the next multiple of `align` (a power of two: 1, 4, or 8 here).
fn align_up(pos: usize, align: usize) -> usize {
    (pos + align - 1) & !(align - 1)
}

/// Pad `buf` with zero bytes until its length is a multiple of `align`.
fn pad(buf: &mut Vec<u8>, align: usize) {
    while buf.len() % align != 0 {
        buf.push(0);
    }
}

/// The width (in bytes) GVariant uses to store framing offsets in a container of the given
/// total `size`: the smallest of 1/2/4/8 that can address any byte in it. Zero for an empty
/// container. This is the reader's rule — the inverse of [`chosen_offset_size`].
fn offset_size(size: usize) -> usize {
    if size == 0 {
        0
    } else if size <= 0xff {
        1
    } else if size <= 0xffff {
        2
    } else if size <= 0xffff_ffff {
        4
    } else {
        8
    }
}

/// The writer's dual of [`offset_size`]: the smallest offset width such that a body of
/// `body` bytes plus `n` framing offsets of that width still fits in the width itself
/// (adding the table can push the total across a boundary, so the choice is made against
/// the *grown* size — GLib's `gvs_calculate_total_size`).
fn chosen_offset_size(body: usize, n: usize) -> usize {
    if n == 0 {
        0
    } else if body + n <= 0xff {
        1
    } else if body + 2 * n <= 0xffff {
        2
    } else if body + 4 * n <= 0xffff_ffff {
        4
    } else {
        8
    }
}

/// Append `value` as a little-endian offset of `width` bytes.
fn push_offset(buf: &mut Vec<u8>, value: usize, width: usize) {
    let le = (value as u64).to_le_bytes();
    buf.extend_from_slice(&le[..width]);
}

/// Read a little-endian framing offset of `width` bytes at `pos`, bounds-checked.
fn read_offset(slice: &[u8], pos: usize, width: usize) -> Result<usize> {
    let end = pos
        .checked_add(width)
        .ok_or(Fp3Error::Malformed("offset position overflow"))?;
    let raw = slice
        .get(pos..end)
        .ok_or(Fp3Error::Malformed("framing offset out of range"))?;
    let mut acc = 0u64;
    for (i, &b) in raw.iter().enumerate() {
        acc |= u64::from(b) << (8 * i);
    }
    Ok(acc as usize)
}

// ---- writing --------------------------------------------------------------------------

/// A serialized GVariant value: its bytes, its alignment, and whether its *type* is
/// fixed-width. Parents read `align` to place it and `fixed` to decide whether it needs a
/// framing offset.
pub(crate) struct Val {
    bytes: Vec<u8>,
    align: usize,
    fixed: bool,
}

impl Val {
    /// Consume the value, yielding its serialized bytes.
    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// A GVariant `i` (int32): 4 bytes little-endian, alignment 4, fixed-width.
pub(crate) fn int32(v: i32) -> Val {
    Val { bytes: v.to_le_bytes().to_vec(), align: 4, fixed: true }
}

/// A GVariant `y` (byte): alignment 1, fixed-width.
pub(crate) fn byte(v: u8) -> Val {
    Val { bytes: vec![v], align: 1, fixed: true }
}

/// A GVariant `b` (boolean): one byte `0x00`/`0x01`, alignment 1, fixed-width.
pub(crate) fn boolean(v: bool) -> Val {
    Val { bytes: vec![u8::from(v)], align: 1, fixed: true }
}

/// A GVariant `s` (string): the UTF-8 bytes plus one `0x00` terminator, alignment 1.
pub(crate) fn string(s: &str) -> Val {
    let mut bytes = s.as_bytes().to_vec();
    bytes.push(0);
    Val { bytes, align: 1, fixed: false }
}

/// A GVariant `ms` (maybe of a variable-size string). `Nothing` is zero bytes; `Just` is
/// the string's serialized bytes followed by one extra `0x00`.
pub(crate) fn maybe_string(s: Option<&str>) -> Val {
    match s {
        None => Val { bytes: Vec::new(), align: 1, fixed: false },
        Some(s) => {
            let mut bytes = s.as_bytes().to_vec();
            bytes.push(0); // the string's own terminator
            bytes.push(0); // the maybe's presence byte
            Val { bytes, align: 1, fixed: false }
        }
    }
}

/// The empty `a{sv}` reserved vardict: always zero bytes, alignment 8.
pub(crate) fn empty_vardict() -> Val {
    Val { bytes: Vec::new(), align: 8, fixed: false }
}

/// A GVariant `ai` (array of fixed-width int32): elements concatenated, no framing table
/// (their count is recovered as `size / 4`). Alignment 4.
pub(crate) fn int32_array(xs: &[i32]) -> Val {
    let mut bytes = Vec::with_capacity(xs.len() * 4);
    for &x in xs {
        bytes.extend_from_slice(&x.to_le_bytes());
    }
    Val { bytes, align: 4, fixed: false }
}

/// A GVariant `a T` for variable-width `T`: each element padded to `elem_align`, then a
/// framing-offset table giving each element's end position (in element order). An empty
/// array is zero bytes.
pub(crate) fn array(elems: Vec<Val>, elem_align: usize) -> Val {
    if elems.is_empty() {
        return Val { bytes: Vec::new(), align: elem_align, fixed: false };
    }
    let mut body = Vec::new();
    let mut ends = Vec::with_capacity(elems.len());
    for e in &elems {
        pad(&mut body, elem_align);
        body.extend_from_slice(&e.bytes);
        ends.push(body.len());
    }
    let width = chosen_offset_size(body.len(), ends.len());
    for &end in &ends {
        push_offset(&mut body, end, width);
    }
    Val { bytes: body, align: elem_align, fixed: false }
}

/// A GVariant tuple `(...)`: members laid out in order, each padded to its alignment; a
/// framing offset (its end position) recorded for every variable-width member except the
/// last, then appended in *reverse* member order. An all-fixed tuple is padded to its own
/// alignment and is itself fixed-width; a variable-width tuple whose content is empty (its
/// single member an empty array) serializes to zero bytes.
pub(crate) fn tuple(members: Vec<Val>, align: usize) -> Val {
    let len = members.len();
    let mut body = Vec::new();
    let mut var_ends = Vec::new();
    for (i, m) in members.iter().enumerate() {
        pad(&mut body, m.align);
        body.extend_from_slice(&m.bytes);
        if !m.fixed && i + 1 != len {
            var_ends.push(body.len());
        }
    }
    if members.iter().all(|m| m.fixed) {
        pad(&mut body, align);
        return Val { bytes: body, align, fixed: true };
    }
    let width = chosen_offset_size(body.len(), var_ends.len());
    for &end in var_ends.iter().rev() {
        push_offset(&mut body, end, width);
    }
    Val { bytes: body, align, fixed: false }
}

/// A GVariant `v` (variant): the child's serialized bytes, a `0x00` separator, then the
/// child's type-signature ASCII (no trailing NUL). Alignment 8.
pub(crate) fn variant(child: Val, signature: &[u8]) -> Val {
    let mut bytes = child.bytes;
    bytes.push(0);
    bytes.extend_from_slice(signature);
    Val { bytes, align: 8, fixed: false }
}

/// A `v` whose already-serialized bytes are supplied verbatim (a driver's opaque, standalone
/// variant preserved byte-for-byte). Alignment 8.
pub(crate) fn raw_variant(bytes: Vec<u8>) -> Val {
    Val { bytes, align: 8, fixed: false }
}

// ---- reading --------------------------------------------------------------------------

/// A static description of one tuple member: its alignment, and its fixed size if the type
/// is fixed-width (`None` for variable-width members).
pub(crate) struct Spec {
    align: usize,
    size: Option<usize>,
}

impl Spec {
    /// A fixed-width member of the given alignment and byte size.
    pub(crate) const fn fixed(align: usize, size: usize) -> Spec {
        Spec { align, size: Some(size) }
    }

    /// A variable-width member of the given alignment.
    pub(crate) const fn var(align: usize) -> Spec {
        Spec { align, size: None }
    }
}

/// Split a tuple's serialized `slice` into its member byte-slices, given the members'
/// static [`Spec`]s. Fixed members are read at their size; variable non-final members are
/// bounded by framing offsets (stored in reverse at the tail); the final variable member
/// runs to the start of the framing table.
pub(crate) fn walk_tuple<'a>(slice: &'a [u8], specs: &[Spec]) -> Result<Vec<&'a [u8]>> {
    let size = slice.len();
    let len = specs.len();
    let n_offsets = specs
        .iter()
        .enumerate()
        .filter(|(i, s)| s.size.is_none() && *i + 1 != len)
        .count();
    let width = offset_size(size);
    let table = n_offsets
        .checked_mul(width)
        .ok_or(Fp3Error::Malformed("tuple offset table overflow"))?;
    let body_size = size
        .checked_sub(table)
        .ok_or(Fp3Error::Malformed("tuple shorter than its framing table"))?;

    let mut out = Vec::with_capacity(len);
    let mut pos = 0usize;
    let mut consumed = 0usize;
    for (i, spec) in specs.iter().enumerate() {
        pos = align_up(pos, spec.align);
        let end = match spec.size {
            Some(sz) => pos + sz,
            None if i + 1 == len => body_size,
            None => {
                consumed += 1;
                read_offset(slice, size - consumed * width, width)?
            }
        };
        if pos > end || end > slice.len() {
            return Err(Fp3Error::Malformed("tuple member out of range"));
        }
        out.push(&slice[pos..end]);
        pos = end;
    }
    Ok(out)
}

/// Split a `v`'s serialized `slice` into `(child_value, signature)`. The signature is the
/// tail after the final `0x00` (signatures contain no NUL, so the last `0x00` is the
/// separator); the child value is everything before it.
pub(crate) fn split_variant(slice: &[u8]) -> Result<(&[u8], &[u8])> {
    let sep = slice
        .iter()
        .rposition(|&b| b == 0)
        .ok_or(Fp3Error::Malformed("variant missing signature separator"))?;
    Ok((&slice[..sep], &slice[sep + 1..]))
}

/// Read a GVariant `i` (int32) from a 4-byte member slice.
pub(crate) fn read_i32(slice: &[u8]) -> Result<i32> {
    let bytes: [u8; 4] = slice
        .get(..4)
        .ok_or(Fp3Error::Malformed("int32 too short"))?
        .try_into()
        .expect("slice of length 4 converts to [u8; 4]");
    Ok(i32::from_le_bytes(bytes))
}

/// Read a GVariant `y` (byte) from a 1-byte member slice.
pub(crate) fn read_byte(slice: &[u8]) -> Result<u8> {
    slice
        .first()
        .copied()
        .ok_or(Fp3Error::Malformed("byte member empty"))
}

/// Read a GVariant `b` (boolean) from a 1-byte member slice (nonzero ⇒ true).
pub(crate) fn read_bool(slice: &[u8]) -> Result<bool> {
    Ok(read_byte(slice)? != 0)
}

/// Read a GVariant `s` (string): UTF-8 bytes followed by a `0x00` terminator.
pub(crate) fn read_string(slice: &[u8]) -> Result<String> {
    let utf8 = slice
        .strip_suffix(&[0])
        .ok_or(Fp3Error::Malformed("string not nul-terminated"))?;
    core::str::from_utf8(utf8)
        .map(str::to_owned)
        .map_err(|_| Fp3Error::Malformed("string not valid utf-8"))
}

/// Read a GVariant `ms` (maybe-string): `Nothing` (empty slice) ⇒ `None`; `Just` ⇒ the
/// string with its trailing maybe-presence byte stripped.
pub(crate) fn read_maybe_string(slice: &[u8]) -> Result<Option<String>> {
    if slice.is_empty() {
        return Ok(None);
    }
    let inner = &slice[..slice.len() - 1];
    Ok(Some(read_string(inner)?))
}

/// Read a GVariant `ai` (array of int32) from a member slice whose length is a multiple of 4.
pub(crate) fn read_i32_array(slice: &[u8]) -> Result<Vec<i32>> {
    if slice.len() % 4 != 0 {
        return Err(Fp3Error::Malformed("int32 array size not a multiple of 4"));
    }
    Ok(slice
        .chunks_exact(4)
        .map(|c| i32::from_le_bytes(c.try_into().expect("chunk of length 4")))
        .collect())
}

/// Read a GVariant `a T` for variable-width `T`, decoding each element slice with `f`.
///
/// The framing table sits at the tail: its last offset marks the end of the final element
/// (and the start of the table), from which the element count follows. An empty array is
/// zero bytes; the degenerate one-byte `0x00` tuple-minimum encoding of an empty array
/// (whose final offset reads as 0) is likewise treated as empty.
pub(crate) fn read_var_array<T, F>(slice: &[u8], elem_align: usize, mut f: F) -> Result<Vec<T>>
where
    F: FnMut(&[u8]) -> Result<T>,
{
    if slice.is_empty() {
        return Ok(Vec::new());
    }
    let size = slice.len();
    let width = offset_size(size);
    let last_end = read_offset(slice, size - width, width)?;
    if last_end == 0 {
        return Ok(Vec::new());
    }
    if last_end > size {
        return Err(Fp3Error::Malformed("array framing offset past end"));
    }
    let table = size - last_end;
    if table % width != 0 {
        return Err(Fp3Error::Malformed("array framing table misaligned"));
    }
    let n = table / width;

    let mut out = Vec::with_capacity(n);
    let mut prev = 0usize;
    for k in 0..n {
        let end = read_offset(slice, last_end + k * width, width)?;
        let start = align_up(prev, elem_align);
        if start > end || end > last_end {
            return Err(Fp3Error::Malformed("array element out of range"));
        }
        out.push(f(&slice[start..end])?);
        prev = end;
    }
    Ok(out)
}
