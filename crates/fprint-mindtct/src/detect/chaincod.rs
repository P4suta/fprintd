// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! 8-connected chain-code of a traced contour and its directional turn analysis — faithful port of
//! stock NBIS `mindtct/src/lib/mindtct/chaincod.c` (`chain_code_loop`, `is_chain_clockwise`).
//!
//! A contour is an ordered list of pixel coordinates whose successive points are 8-adjacent.
//! [`chain_code_loop`] encodes each step (including the wrap from the last point back to the first)
//! as one of eight direction codes; [`is_chain_clockwise`] folds the successive direction changes
//! into a signed turn total to decide the loop's winding.

// PORT: stock `NBR8_DIM` (`lfs.h` L500) — the chain-code lookup matrix is `NBR8_DIM`×`NBR8_DIM`.
const NBR8_DIM: i32 = 3;

/// PORT: stock `chaincodes_nbr8` (`globals.c` L250) — the 8-connected chain-code lookup matrix,
/// row-major `NBR8_DIM`×`NBR8_DIM`. Indexed by `(dy + 1) * NBR8_DIM + (dx + 1)` for a step whose
/// per-axis delta `dx`/`dy` lies in `-1..=1`, it maps that neighbor offset to its direction code:
///
/// ```text
///          dx=-1  dx=0  dx=1
///   dy=-1:   3     2     1
///   dy= 0:   4    -1     0
///   dy= 1:   5     6     7
/// ```
///
/// The center cell (`dx == dy == 0`, no movement) holds the sentinel `-1`; a well-formed contour
/// never produces a zero-length step, so it is never selected.
const CHAINCODES_NBR8: [i32; (NBR8_DIM * NBR8_DIM) as usize] = [3, 2, 1, 4, -1, 0, 5, 6, 7];

// PORT: stock `TRUE`/`FALSE` (`lfs.h` L673, L676). `is_chain_clockwise` returns these — or the
// caller-supplied `default_ret`, which is itself an application-specific `int` — so the return type
// stays `i32` rather than `bool` to preserve that three-way contract verbatim.
const TRUE: i32 = 1;
const FALSE: i32 = 0;

/// Convert a feature's contour points into an 8-connected chain-code vector — stock
/// `chain_code_loop` (`chaincod.c` L85).
///
/// Each element is the direction taken between two adjacent contour points, with a final code
/// wrapping the last point back to the first (closing the loop). The returned vector has one code
/// per contour point (stock `onchain == ncontour`).
///
/// `contour_x` and `contour_y` are the parallel coordinate lists; their shared length is the stock
/// `ncontour` (the caller supplies them in lock-step, exactly as the reference does). If the contour
/// has three or fewer points it does not form a loop, so an **empty** vector is returned (stock
/// `onchain = 0` with no allocation) — the port's stand-in for that early exit.
///
/// PORT: the stock `-170` `malloc`-failure path is unreachable here — `Vec` allocation aborts rather
/// than returning an error code — so the return type is the chain itself, not a `Result`.
///
/// # Panics
///
/// Panics on an out-of-bounds `CHAINCODES_NBR8` index if two successive contour points are not
/// 8-adjacent (`|dx| > 1` or `|dy| > 1`), where stock C would read past the matrix. Contour tracing
/// guarantees 8-adjacency, so this cannot occur for a valid contour.
pub(crate) fn chain_code_loop(contour_x: &[i32], contour_y: &[i32]) -> Vec<i32> {
    let ncontour = contour_x.len() as i32;

    // If we don't have at least 3 points in the contour, then we don't have a loop, so return an
    // empty chain (stock sets `onchain = 0` and returns with no allocation).
    if ncontour <= 3 {
        return Vec::new();
    }

    // The chain is the same length as the contour: one code between each pair of adjacent points,
    // including the last-to-first code that completes the loop.
    let mut chain = vec![0_i32; ncontour as usize];

    // For each neighboring point, with `i` on the previous neighbor and `j` on the next, derive the
    // chain-code index from the neighbor deltas. The deltas lie in `-1..=1`, so they are incremented
    // by one to index the lookup matrix.
    let mut i = 0_i32;
    let mut j = 1_i32;
    while i < ncontour - 1 {
        let dx = contour_x[j as usize] - contour_x[i as usize];
        let dy = contour_y[j as usize] - contour_y[i as usize];
        chain[i as usize] = CHAINCODES_NBR8[((dy + 1) * NBR8_DIM + dx + 1) as usize];
        i += 1;
        j += 1;
    }

    // Now derive the chain code between the last and first points in the contour list. (`i` is left
    // at `ncontour - 1` by the loop above, exactly as in the reference.)
    let dx = contour_x[0] - contour_x[i as usize];
    let dy = contour_y[0] - contour_y[i as usize];
    chain[i as usize] = CHAINCODES_NBR8[((dy + 1) * NBR8_DIM + dx + 1) as usize];

    chain
}

/// Decide whether an 8-connected chain-code vector is ordered clockwise — stock
/// `is_chain_clockwise` (`chaincod.c` L156).
///
/// Accumulates the change in direction between each successive pair of codes (including the wrap
/// from the last code back to the first), reducing each delta to its "inner" distance in `-3..=4`
/// so a step across the `7 → 0` seam counts as a small turn rather than a large one. Left-hand turns
/// increment the accumulator, right-hand turns decrement it.
///
/// Returns [`TRUE`] (`1`) when the net turn is negative (more right-hand turns → clockwise),
/// [`FALSE`] (`0`) when it is positive (counter-clockwise), and the caller-supplied `default_ret`
/// when the net turn is exactly zero (direction indeterminate).
///
/// `chain` is the chain-code vector; its length is the stock `nchain`. The caller must pass a
/// non-empty chain — stock's only caller (`is_loop_clockwise`, `loop.c` L515) guards the empty case
/// beforehand and returns its own default, so this routine indexes `chain[0]` unconditionally, just
/// as the reference does.
pub(crate) fn is_chain_clockwise(chain: &[i32], default_ret: i32) -> i32 {
    let nchain = chain.len() as i32;

    // Initialize the turn-accumulator to 0.
    let mut sum = 0_i32;

    // For each neighboring code, compute the difference in direction and accumulate. Each delta is
    // reduced to its inner distance: a delta `>= 4` is a right-hand turn (subtract 8), a delta
    // `<= -4` is a left-hand turn (add 8).
    let mut i = 0_i32;
    let mut j = 1_i32;
    while i < nchain - 1 {
        let mut d = chain[j as usize] - chain[i as usize];
        if d >= 4 {
            d -= 8;
        } else if d <= -4 {
            d += 8;
        }
        sum += d;
        i += 1;
        j += 1;
    }

    // Add the final delta between the last and first codes in the chain. (`i` is left at
    // `nchain - 1` by the loop above.)
    let mut d = chain[0] - chain[i as usize];
    if d >= 4 {
        d -= 8;
    } else if d <= -4 {
        d += 8;
    }
    sum += d;

    // A net turn of zero means the direction is indeterminate → return the caller's default.
    if sum == 0 {
        default_ret
    } else if sum > 0 {
        // More left-hand than right-hand turns → counter-clockwise.
        FALSE
    } else {
        // More right-hand than left-hand turns → clockwise.
        TRUE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A contour of three or fewer points is not a loop, so no chain is produced (stock
    /// `onchain = 0`).
    #[test]
    fn too_few_points_yields_empty_chain() {
        assert!(chain_code_loop(&[], &[]).is_empty());
        assert!(chain_code_loop(&[0, 1], &[0, 0]).is_empty());
        assert!(chain_code_loop(&[0, 1, 2], &[0, 0, 0]).is_empty());
    }

    // A 2×2 pixel square walked clockwise on screen (image coords, `y` increasing downward):
    // right along the top, down the right side, left along the bottom, up the left side.
    const SQUARE_X: [i32; 8] = [0, 1, 2, 2, 2, 1, 0, 0];
    const SQUARE_Y: [i32; 8] = [0, 0, 0, 1, 2, 2, 2, 1];

    /// Each 8-adjacent step maps to the expected direction code, and the wrap step (last → first)
    /// closes the loop. Codes: `0`=E, `2`=N(up), `4`=W, `6`=S(down).
    #[test]
    fn square_contour_encodes_expected_codes() {
        let chain = chain_code_loop(&SQUARE_X, &SQUARE_Y);
        // E,E, S,S, W,W, N,N — the last two include the wrap from (0,1) back to (0,0).
        assert_eq!(chain, vec![0, 0, 6, 6, 4, 4, 2, 2]);
        // One code per contour point (stock `onchain == ncontour`).
        assert_eq!(chain.len(), SQUARE_X.len());
    }

    /// The screen-clockwise square is reported clockwise (`TRUE`); reversing the point order flips
    /// it to counter-clockwise (`FALSE`).
    #[test]
    fn winding_matches_traversal_order() {
        let cw = chain_code_loop(&SQUARE_X, &SQUARE_Y);
        assert_eq!(is_chain_clockwise(&cw, -1), TRUE);

        let mut rx = SQUARE_X;
        let mut ry = SQUARE_Y;
        rx.reverse();
        ry.reverse();
        let ccw = chain_code_loop(&rx, &ry);
        assert_eq!(is_chain_clockwise(&ccw, -1), FALSE);
    }

    /// A chain whose turns cancel to a net zero is indeterminate, so the caller's `default_ret` is
    /// returned unchanged. `[0,4,0,4]` alternates E/W: each `±4` delta folds to the same sign and
    /// the four cancel.
    #[test]
    fn indeterminate_winding_returns_default() {
        let sentinel = 42;
        assert_eq!(is_chain_clockwise(&[0, 4, 0, 4], sentinel), sentinel);
    }

    /// The inner-distance reduction folds a step across the `7 → 0` seam into a single unit turn
    /// rather than a seven-unit one, so a chain rotating steadily through all eight directions winds
    /// consistently (here counter-clockwise, `+8`).
    #[test]
    fn seam_crossing_folds_to_inner_distance() {
        let chain = [0, 1, 2, 3, 4, 5, 6, 7];
        assert_eq!(is_chain_clockwise(&chain, -1), FALSE);
    }
}
