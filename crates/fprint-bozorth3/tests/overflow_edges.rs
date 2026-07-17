// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `match_score` must not panic on *any* input, as its docs promise — including out-of-contract xyt
//! whose extreme thetas or coordinates would overflow the reference's `int` angle/coordinate
//! arithmetic. The kernel does that arithmetic with `wrapping_*` (matching the stock C's silent int
//! wraparound), so the "never panics on any input" contract holds even under the overflow-checks
//! the test and fuzz profiles keep on. The `totality.rs` sweep exercises realistic worst cases but
//! stops short of the i32 edges these reach.

use fprint_bozorth3::{match_score, Minutia};

/// A dozen minutiae — comfortably over the computable floor — packed within the distance guard so
/// their pairs reach the angle arithmetic, carrying `i32::MIN`/`i32::MAX` thetas. `theta_kj - theta`
/// overflows `i32` for these unless it wraps.
#[test]
fn extreme_thetas_do_not_panic() {
    let probe: Vec<Minutia> = (0..12)
        .map(|i| Minutia::from_xyt(i, i, if i % 2 == 0 { i32::MIN } else { i32::MAX }))
        .collect();
    // Returns a score (any value) rather than panicking under overflow-checks.
    let _ = match_score(&probe, &probe);
}

/// Alternating extreme coordinates make `x[j] - x[k]` / `y[j] - y[k]` overflow `i32` unless wrapped.
#[test]
fn extreme_coordinates_do_not_panic() {
    let probe: Vec<Minutia> = (0..12)
        .map(|i| {
            if i % 2 == 0 {
                Minutia::from_xyt(i32::MIN, i32::MAX, 45)
            } else {
                Minutia::from_xyt(i32::MAX, i32::MIN, 300)
            }
        })
        .collect();
    let _ = match_score(&probe, &probe);

    // And against a distinct gallery whose thetas are extreme too, so both overflow sites fire in
    // one cross-comparison.
    let gallery: Vec<Minutia> = (0..12)
        .map(|i| Minutia::from_xyt(i32::MIN + i, i32::MAX - i, i32::MIN))
        .collect();
    let _ = match_score(&probe, &gallery);
}
