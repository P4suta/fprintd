// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Frozen fuzz findings: **an image with either dimension under 25 pixels can panic
//! [`detect_minutiae`], though its size is consistent and its precondition is met.**
//!
//! Found by `fuzz/fuzz_targets/mindtct_detect.rs`. The inputs need no fuzzer to reproduce and none
//! to state — the trigger is the **geometry**, and the pixels only decide whether the hole is
//! stepped in — so they are frozen here as ordinary `#[test]`s: no nightly, no libfuzzer, no
//! corpus, no network, every platform. There is no artifact file to keep beside this one.
//!
//! ## The one cause
//!
//! `src/maps.rs:317` computes the window origin's limits, and `:329` clamps to them:
//!
//! ```text
//! xmaxlimit = pw - pad - windowsize - 1        // :317
//! win_x = win_x.max(xminlimit).min(xmaxlimit)  // :329
//! ```
//!
//! With the shipping V2 geometry (`pad` 13, `windowsize` 24, `blocksize` 8) and `pw == iw + 26`,
//! this is `xmaxlimit == iw - 12` against `xminlimit == pad == 13`. **For any image narrower than
//! 25 the limits are inverted** — `xmaxlimit < xminlimit` — and since `.min()` is applied last it
//! wins: `win_x` lands on `xmaxlimit`, below the minimum the `.max()` just enforced, and below zero
//! once `iw < 12`. The same arithmetic holds for `win_y` and the height.
//!
//! `src/maps.rs:324` says "dft_offset is always >= 0 for V2: pad > windowoffset". That is true of
//! `dft_offset`, and says nothing about `win_x`/`win_y` — the clamp on the next line drives those
//! negative on its own.
//!
//! ## The two places it lands
//!
//! Both consume the resulting `low_contrast_offset` as an index and both take it `as usize`, so a
//! negative offset wraps to a huge one rather than indexing backwards:
//!
//! * **`src/block.rs:165`** (`low_contrast_block`'s histogram). An 8×8 image reaches `block.rs:170`
//!   with index `18446744073709551476` — that is `-140 == (-4 * 34) + (-4)` — against a 1156-byte
//!   padded image. Reachable with a **flat** fill.
//! * **`src/maps.rs:97`** (`sum_rot_block_rows`, the DFT rotated-grid sums). Needs a block with
//!   enough contrast to get past the low-contrast test, so a flat image never reaches it and only
//!   noise does.
//!
//! ## The reachable set
//!
//! Swept over every shape in `0..=40` squared against `iw >= 8 && ih >= 8 && (iw < 25 || ih < 25)`:
//! **no panic falls outside it**. It is an over-approximation, not an iff — most shapes inside it
//! survive, because a negative `win_x` still needs a positive `win_y * pw` too small to mask it,
//! and then a block that reaches the indexing at all.
//!
//! Below 8 in either dimension `block_offsets` returns `Err(-80)` first and the caller answers with
//! the empty list `src/lib.rs` promises. From 25×25 up nothing panicked over 399 seeds. **A
//! realistic fingerprint is far outside the set**, which is why the golden corpus never saw this.
//!
//! ## Recorded, not fixed
//!
//! Recorded because that is what the code does, following `tests/bounds.rs` and
//! `fprint-bozorth3`'s `tests/totality.rs`. The clamp is faithful to the stock C, which computes
//! the same negative offset and reads `pdata[-140]` — an out-of-bounds read the C does silently and
//! Rust's bounds check turns into a panic. Ordering the clamp correctly changes which blocks the
//! port marks low-contrast on small images, and that is a decision for the port's owner rather than
//! a fold-in to a test suite. Recorded so that changing it is a deliberate act with a failing test
//! attached.
//!
//! **Reordering the clamp is not that fix.** Applying `.min(max)` before `.max(min)` holds the
//! origin at `pad`, and an 8×8 image then indexes 1156 into a 1156-byte padded image — the read runs
//! off the *end* instead of the start, and `detect_minutiae` still panics. No ordering helps: below
//! 25 pixels there is nowhere to put a 24-pixel window, so the limits have no satisfiable value
//! between them and the fix has to reject or special-case the image, not clamp it harder. These
//! tests name the wrapped offset precisely so that a reorder fails them rather than passing quietly.
//!
//! ## What this corrects
//!
//! `tests/bounds.rs` states that a *degenerate but consistent* image (`data.len() == width * height`)
//! returns an empty list, and that only an inconsistent one panics. **That law has a hole**: 512×8
//! is consistent and panics. `bounds.rs` never tried a shape in the band — it tests 1×1, one-pixel
//! strips and 512×512, which straddle it.
//!
//! ## Limits
//!
//! These pin the cause and the edges, not a minutia count: a count for an image this size is not a
//! property anyone should depend on. The `[`a_noise_image_sixteen_wide_panics_in_the_dft_sums`]`
//! seed is one witness, not the boundary of the data condition — the geometry is the finding.

use fprint_mindtct::{detect_minutiae, GrayImage};
use fprint_testkit::{gen, Lcg};

/// The message `f` panics with, failing if `f` returns.
///
/// `#[should_panic(expected = "index out of bounds")]` is too coarse for this file. A window that
/// merely runs off the end of the padded image spells that same message, so such a test passes
/// whether the offset wrapped or not — and stays green once the clamp is fixed, which is the
/// opposite of what recording a finding is for. These tests name the offset instead.
fn panic_message(f: impl FnOnce() + std::panic::UnwindSafe) -> String {
    let Err(payload) = std::panic::catch_unwind(f) else {
        panic!("detect_minutiae returned: the recorded panic is gone, so this test records nothing")
    };
    if let Some(s) = payload.downcast_ref::<&str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    panic!("panicked with a payload that is not a string")
}

/// The index a negative padded-image offset wraps to when taken `as usize`.
///
/// Spelled as the arithmetic rather than the 20-digit constant it prints as: the negative offset is
/// the finding, and computing the wrap keeps these true on a 32-bit target too.
fn wrapped(offset: isize) -> String {
    format!("the index is {}", offset as usize)
}

/// A flat mid-gray image of exactly `width * height` bytes — inside [`GrayImage`]'s precondition.
///
/// A flat block is low contrast, so this reaches the histogram and never the DFT sums.
fn detect_flat(width: usize, height: usize) -> usize {
    let data = vec![0x80u8; width * height];
    let img = GrayImage::new(&data, width, height, 500).expect("buffer holds the image");
    detect_minutiae(img).len()
}

/// A noise image of exactly `width * height` bytes, from the seed a failure can be replayed with.
fn detect_noise(width: usize, height: usize, seed: u64) -> usize {
    let data = gen::gray_image(&mut Lcg::new(seed), width, height);
    let img = GrayImage::new(&data, width, height, 500).expect("buffer holds the image");
    detect_minutiae(img).len()
}

/// The shape the fuzzer reported first, reduced to its geometry: flat, so the pixels are not part
/// of it. Lands in `low_contrast_block`'s histogram.
#[test]
fn a_flat_eight_by_eight_image_panics_on_the_wrapped_window_offset() {
    let msg = panic_message(|| {
        let _ = detect_flat(8, 8);
    });
    assert!(
        msg.contains(&wrapped(-140)),
        "expected the window offset to wrap from -140 == (-4 * 34) + (-4), the inverted clamp \
         driving both win_x and win_y to -4; got: {msg}"
    );
}

/// Width does not save a short image: `win_y` is negative on its own here, and `win_x` sits at the
/// minimum the clamp enforced — `-2139 == (-4 * 538) + 13`.
#[test]
fn a_flat_full_width_image_eight_rows_tall_panics() {
    let msg = panic_message(|| {
        let _ = detect_flat(512, 8);
    });
    assert!(
        msg.contains(&wrapped(-2139)),
        "expected the window offset to wrap from -2139 == (-4 * 538) + 13, height alone driving \
         win_y negative; got: {msg}"
    );
}

/// The second landing site, behind the first: a 16×16 noise image has blocks with enough contrast
/// to enter the DFT path, where `sum_rot_block_rows` indexes with the same wrapped offset.
///
/// Noise rather than a flat fill is the whole point — a flat block is low contrast and stops one
/// stage earlier, which is why `tests/bounds.rs`'s flat cases could not have found this.
#[test]
fn a_noise_image_sixteen_wide_panics_in_the_dft_sums() {
    let msg = panic_message(|| {
        let _ = detect_noise(16, 16, 1);
    });
    assert!(
        msg.contains(&wrapped(-26)),
        "expected a rotated-grid sum to index with the wrapped offset -26; got: {msg}"
    );
}

/// The lower edge: below 8, `block_offsets` rejects the image before the clamp runs, so the answer
/// is the empty list `src/lib.rs` promises rather than a panic.
#[test]
fn an_image_under_eight_pixels_is_rejected_before_the_clamp() {
    assert_eq!(detect_flat(24, 7), 0);
    assert_eq!(detect_flat(7, 24), 0);
    assert_eq!(detect_noise(24, 7, 1), 0);
}

/// The upper edge, and the reason a real image is safe: from 25 in both dimensions the clamp is the
/// right way round, so no offset can go negative whatever the pixels say.
///
/// Termination is the claim, not a count — an image this size may legitimately detect a minutia or
/// none, and pinning either would say something this file does not know.
#[test]
fn an_image_at_least_twenty_five_in_both_dimensions_is_outside_the_band() {
    for seed in 1..40 {
        let _ = detect_noise(25, 25, seed);
    }
    let _ = detect_noise(32, 32, 1);
    let _ = detect_noise(64, 64, 1);
}

/// A fingerprint is nowhere near the band. The finding is a robustness bug at the edge of the
/// domain, not a break in the detector — `tests/golden.rs` is unaffected.
#[test]
fn a_realistic_image_is_far_outside_the_band() {
    // Termination and the absence of a panic are the claim; the count of a noise field is not.
    let _ = detect_noise(320, 480, 1);
}
