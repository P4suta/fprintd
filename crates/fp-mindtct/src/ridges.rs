// SPDX-FileCopyrightText: NIST NBIS — U.S. Government work, public domain (title 17 §105)
//
// SPDX-License-Identifier: LicenseRef-NBIS-PD

//! Neighbor ridge counting (`count_minutiae_ridges`): for each minutia, count the ridges crossing to
//! its nearest neighbours, populating the ridge-count structure.
//!
//! Faithful port of stock NBIS `mindtct/src/lib/mindtct/ridges.c` (the neighbor/ridge stage), plus the
//! two `minutia.c` list operations it opens with — `sort_minutiae_x_y` (`minutia.c` L680) and
//! `rm_dup_minutiae` (`minutia.c` L745). Those two are what fix the **order and count** of the final
//! minutiae list (and therefore the `.xyt` output); the ridge counts themselves are structured data
//! consumed downstream, not part of the `x/y/theta/q` record.
//!
//! Stage outline ([`count_minutiae_ridges`]):
//! 1. [`sort_minutiae_x_y`] — column-oriented sort (rank `x*iw + y`).
//! 2. [`rm_dup_minutiae`] — drop minutiae sharing exact pixel coordinates.
//! 3. For each minutia, [`find_neighbors`] (up to `max_nbrs` closest), [`sort_neighbors`] (vertical
//!    origin, clockwise), and [`ridge_count`] to each — filling the minutia's
//!    [`nbrs`](DetMinutia::nbrs) / [`ridge_counts`](DetMinutia::ridge_counts).
//!
//! The `util.c` helpers this stage relies on (`squared_distance`, `find_incr_position_dbl`,
//! `angle2line`) live in the shared [`crate::util`] module. See `docs/mindtct-algorithm.md`
//! §Bit-exactness.

use crate::detect::contour::{fix_edge_pixel_pair, trace_contour, ScanDir, TraceResult};
use crate::detect::line::line_points;
use crate::detect::DetMinutia;
use crate::num::{bubble_sort_double_inc_2, sort_indices_int_inc};
use crate::params::LfsParms;
use crate::util::{angle2line, find_incr_position_dbl, squared_distance};

/// Sort the minutiae left-to-right then top-to-bottom (column-oriented) — port of stock
/// `sort_minutiae_x_y` (`minutia.c` L680).
///
/// The sort key is the 1-D offset `x*iw + y`, so X is the primary key and Y breaks ties; the stable
/// [`sort_indices_int_inc`] preserves input order among equal keys, matching the reference's stable
/// bubble sort. PORT: stock passes `ih` but never reads it, so it is dropped here (as in
/// `remove::sort_minutiae_y_x`).
fn sort_minutiae_x_y(minutiae: &mut Vec<DetMinutia>, iw: i32) {
    // PORT L694–L699: 1-D column-oriented offsets, then their sorted permutation.
    let mut ranks: Vec<i32> = minutiae.iter().map(|m| (m.x * iw) + m.y).collect();
    let order = sort_indices_int_inc(&mut ranks);

    // PORT L713–L720: rebuild the list in sorted order.
    let newlist: Vec<DetMinutia> = order
        .into_iter()
        .map(|o| minutiae[o as usize].clone())
        .collect();
    *minutiae = newlist;
}

/// Remove minutiae sharing exact pixel coordinates with an adjacent list entry — port of stock
/// `rm_dup_minutiae` (`minutia.c` L745).
///
/// Walks the (already sorted) list back-to-front so removals never disturb not-yet-visited indices;
/// when `list[i]` and `list[i-1]` share `(x, y)`, the earlier one (`i-1`) is dropped. PORT: the stock
/// `remove_minutia` in-place slide is a `Vec::remove`.
fn rm_dup_minutiae(minutiae: &mut Vec<DetMinutia>) {
    // PORT L753: nothing to compare against for an empty (or single) list.
    if minutiae.is_empty() {
        return;
    }
    // PORT L753–L764: work backward, removing the 2nd of each coincident pair.
    let mut i = minutiae.len() - 1;
    while i > 0 {
        if minutiae[i].x == minutiae[i - 1].x && minutiae[i].y == minutiae[i - 1].y {
            // PORT L760: drop `list[i-1]`; the former `list[i]` slides into its place.
            minutiae.remove(i - 1);
        }
        i -= 1;
    }
}

/// Insert a neighbor at `pos`, shifting the tail down and off the fixed-length lists — port of stock
/// `insert_neighbor` (`ridges.c` L416).
///
/// `nbr_list` / `nbr_sqr_dists` are the `max_nbrs`-length working buffers; `nnbrs` is the current fill
/// count. When the lists are not yet full the tail shifts down and `nnbrs` grows; when full the last
/// entry is bumped off. PORT: the stock `-480`/`-481` range guards are provably unreachable here — the
/// only caller derives `pos` from [`find_incr_position_dbl`] over `0..nnbrs`, and only inserts while
/// `nnbrs < max_nbrs` or the new distance beats the last stored one — so they are elided (as the crate
/// elides other unreachable stock error returns).
fn insert_neighbor(
    pos: usize,
    nbr_index: i32,
    nbr_dist2: f64,
    nbr_list: &mut [i32],
    nbr_sqr_dists: &mut [f64],
    nnbrs: &mut usize,
    max_nbrs: usize,
) {
    // PORT L432–L450: start index for the down-shift; grow the count only when there is room.
    let mut i: isize = if *nnbrs < max_nbrs {
        let start = *nnbrs as isize - 1;
        *nnbrs += 1;
        start
    } else {
        // *nnbrs == max_nbrs: bump the last neighbor off (ignore it) to make room.
        *nnbrs as isize - 2
    };

    // PORT L452–L458: shift everything from `i` down one slot until reaching `pos`.
    while i >= pos as isize {
        nbr_list[(i + 1) as usize] = nbr_list[i as usize];
        nbr_sqr_dists[(i + 1) as usize] = nbr_sqr_dists[i as usize];
        i -= 1;
    }

    // PORT L462–L463: drop the new neighbor into the freed slot.
    nbr_list[pos] = nbr_index;
    nbr_sqr_dists[pos] = nbr_dist2;
}

/// Consider `second` as a nearest neighbor of `first`, inserting it in distance order if close enough —
/// port of stock `update_nbr_dists` (`ridges.c` L341).
///
/// PORT: the stock `-470` illegal-position guard is unreachable (see [`insert_neighbor`]) and elided.
fn update_nbr_dists(
    nbr_list: &mut [i32],
    nbr_sqr_dists: &mut [f64],
    nnbrs: &mut usize,
    max_nbrs: usize,
    first: usize,
    second: usize,
    minutiae: &[DetMinutia],
) {
    // PORT L350: position of the maximum (last) stored neighbor.
    let last_nbr = max_nbrs - 1;

    // PORT L356–L358: squared distance between the minutia pair.
    let dist2 = squared_distance(
        minutiae[first].x,
        minutiae[first].y,
        minutiae[second].x,
        minutiae[second].y,
    );

    // PORT L363–L364: room left, or closer than the farthest stored neighbor?
    if *nnbrs < max_nbrs || dist2 < nbr_sqr_dists[last_nbr] {
        // PORT L367: insertion point preserving increasing distance order.
        let pos = find_incr_position_dbl(dist2, &nbr_sqr_dists[..*nnbrs]);
        // PORT L377–L379: insert at `pos`.
        insert_neighbor(
            pos,
            second as i32,
            dist2,
            nbr_list,
            nbr_sqr_dists,
            nnbrs,
            max_nbrs,
        );
    }
    // PORT L387: otherwise not close enough — ignore.
}

/// Locate up to `max_nbrs` closest neighbors of `first`, searched forward through the sorted list —
/// port of stock `find_neighbors` (`ridges.c` L227).
///
/// Because the list is sorted on `x*iw + y`, neighbors are drawn from the same pixel column below the
/// primary and then complete columns to its right; the X-distance is a monotone lower bound on the
/// true distance, so once the lists are full and the X-distance exceeds the farthest stored neighbor,
/// the search stops. Returns the neighbor indices ordered by increasing squared distance (length is
/// the stock `nnbrs`, empty when none qualify).
///
/// PORT: the two out-parameters (`onbr_list`, `onnbrs`) collapse into the returned `Vec`; the
/// fixed-length `nbr_list` / `nbr_sqr_dists` working buffers become `max_nbrs`-length vectors truncated
/// to `nnbrs` on return.
fn find_neighbors(max_nbrs: i32, first: usize, minutiae: &[DetMinutia]) -> Vec<i32> {
    // PORT L236–L250: fixed-length working buffers (indices + their squared distances).
    let max_nbrs = max_nbrs.max(0) as usize;
    let mut nbr_list = vec![0i32; max_nbrs];
    let mut nbr_sqr_dists = vec![0.0f64; max_nbrs];

    // PORT L253–L257: no stored neighbors yet; scan from just past the primary.
    let mut nnbrs: usize = 0;
    let last_nbr = max_nbrs - 1;
    let mut second = first + 1;

    // PORT L265–L294: walk the sorted suffix while neighbors may still qualify.
    while second < minutiae.len() {
        // PORT L271–L272: squared X-distance — a lower bound on the true squared distance.
        let xdist = f64::from(minutiae[second].x - minutiae[first].x);
        let xdist2 = xdist * xdist;

        // PORT L276–L277: lists not full, OR this column is closer than the farthest stored neighbor.
        if nnbrs < max_nbrs || xdist2 < nbr_sqr_dists[last_nbr] {
            // PORT L279–L280: append or insert the candidate.
            update_nbr_dists(
                &mut nbr_list,
                &mut nbr_sqr_dists,
                &mut nnbrs,
                max_nbrs,
                first,
                second,
                minutiae,
            );
        } else {
            // PORT L288–L290: full and this (and every farther) column is too far → stop.
            break;
        }

        // PORT L293: next secondary minutia.
        second += 1;
    }

    // PORT L299–L309: return only the neighbors actually stored.
    nbr_list.truncate(nnbrs);
    nbr_list
}

/// Sort a minutia's neighbors by their bearing from it — vertical origin, clockwise — port of stock
/// `sort_neighbors` (`ridges.c` L488).
///
/// The bearing to each neighbor is [`angle2line`] with coordinates swapped and point order reversed
/// (so direction `0` is vertical and positive runs clockwise), made non-negative and folded onto
/// `[0, 2*PI)`. The stable [`bubble_sort_double_inc_2`] then reorders `nbr_list` by ascending bearing.
///
/// PORT: the by-reference sort of the stock in/out `nbr_list` becomes taking and returning it by value;
/// `fmod(theta, 2*PI)` is `rem_euclid` (the argument is always non-negative here, so they agree).
fn sort_neighbors(mut nbr_list: Vec<i32>, first: usize, minutiae: &[DetMinutia]) -> Vec<i32> {
    // PORT L493: `pi2 = 2*PI`.
    let pi2 = std::f64::consts::PI * 2.0;

    // PORT L503–L517: bearing of the line joining the primary to each neighbor.
    let mut join_thetas: Vec<f64> = nbr_list
        .iter()
        .map(|&nb| {
            let nb = nb as usize;
            // PORT L508–L511: coords swapped / points reversed for the vertical-clockwise convention.
            let mut theta = angle2line(
                minutiae[nb].y,
                minutiae[nb].x,
                minutiae[first].y,
                minutiae[first].x,
            );
            // PORT L514–L515: make positive and fold onto `[0, 2*PI)`.
            theta += pi2;
            theta.rem_euclid(pi2)
        })
        .collect();

    // PORT L520: stable sort the neighbor indices into ascending-bearing order.
    bubble_sort_double_inc_2(&mut join_thetas, &mut nbr_list);

    nbr_list
}

/// Search forward along a pixel trajectory for the `pix1`→`pix2` transition — port of stock
/// `find_transition` (`ridges.c` L709).
///
/// Starting at index `*iptr`, tests each adjacent pixel pair `(i, i+1)`; on the first pair equal to
/// `(pix1, pix2)` it sets `*iptr` to the second pixel's index and returns `true`. Otherwise it sets
/// `*iptr` to `num` (past the end) and returns `false`.
///
/// PORT: stock passes `ih` but only ever indexes with `iw`, so `ih` is dropped. `pts` replaces the
/// parallel `xlist`/`ylist` (x = `.0`, y = `.1`).
fn find_transition(
    iptr: &mut usize,
    pix1: u8,
    pix2: u8,
    pts: &[(i32, i32)],
    num: usize,
    bdata: &[u8],
    iw: i32,
) -> bool {
    // PORT L716–L718: current index and its successor.
    let mut i = *iptr;
    let mut j = i + 1;

    // PORT L721: stop one short of the trajectory end.
    while i < num - 1 {
        // PORT L723–L724: desired transition found?
        let pi = bdata[(pts[i].1 * iw + pts[i].0) as usize];
        let pj = bdata[(pts[j].1 * iw + pts[j].0) as usize];
        if pi == pix1 && pj == pix2 {
            // PORT L727: point at the second pixel of the transition.
            *iptr = j;
            return true;
        }
        // PORT L734–L735: advance to the next pixel pair.
        i += 1;
        j += 1;
    }

    // PORT L741–L742: exhausted the trajectory without a match.
    *iptr = num;
    false
}

/// Decide whether a ridge start/end transition is a genuine ridge crossing — port of stock
/// `validate_ridge_crossing` (`ridges.c` L770).
///
/// Traces the ridge contour outward from the ridge-end edge pixel, both clockwise and
/// counter-clockwise, up to `max_ridge_steps` steps, watching for the ridge-start point. If either
/// trace loops back to the start (or is impossible to seed), the pair is walking on and off the side of
/// one ridge, not crossing — invalid. A crossing is valid only when **both** traces complete without
/// looping.
///
/// PORT: stock's `IGNORE` / `LOOP_FOUND` / `0` `int` returns map onto [`TraceResult`]
/// `Ignore` / `Loop` / `Traced`; "not IGNORE and not LOOP_FOUND" is exactly `matches!(_, Traced)`. The
/// unused `num` parameter and the never-taken `< 0` system-error path are dropped, and the traced
/// contours (which stock immediately frees) are simply discarded.
#[allow(clippy::too_many_arguments)]
fn validate_ridge_crossing(
    ridge_start: usize,
    ridge_end: usize,
    pts: &[(i32, i32)],
    bdata: &[u8],
    iw: i32,
    ih: i32,
    max_ridge_steps: i32,
) -> bool {
    // PORT L780–L783: feature pixel at the ridge end, edge pixel one step before it.
    // PORT L786–L787: nudge a diagonally adjacent pair onto a cardinal one before tracing.
    let (feat_x, feat_y, edge_x, edge_y) = fix_edge_pixel_pair(
        pts[ridge_end].0,
        pts[ridge_end].1,
        pts[ridge_end - 1].0,
        pts[ridge_end - 1].1,
        bdata,
        iw,
    );

    // PORT L800: the ridge-start point is the loop-trigger the traces watch for.
    let x_loop = pts[ridge_start - 1].0;
    let y_loop = pts[ridge_start - 1].1;

    // PORT L797–L802: trace clockwise; a completed (non-looping) trace lets us try the other way.
    let clockwise = trace_contour(
        max_ridge_steps,
        x_loop,
        y_loop,
        feat_x,
        feat_y,
        edge_x,
        edge_y,
        ScanDir::Clockwise,
        bdata,
        iw,
        ih,
    );
    // PORT L818–L819: not IGNORE and not LOOP_FOUND → keep validating.
    if matches!(clockwise, TraceResult::Traced(_)) {
        // PORT L823–L828: now trace counter-clockwise.
        let counter = trace_contour(
            max_ridge_steps,
            x_loop,
            y_loop,
            feat_x,
            feat_y,
            edge_x,
            edge_y,
            ScanDir::CounterClockwise,
            bdata,
            iw,
            ih,
        );
        // PORT L841–L845: both traces completed without looping → a valid crossing.
        if matches!(counter, TraceResult::Traced(_)) {
            return true;
        }
    }

    // PORT L850–L851: failed to validate.
    false
}

/// Count the ridges crossed along the straight trajectory from minutia `first` to `second` — port of
/// stock `ridge_count` (`ridges.c` L547).
///
/// Walks the [`line_points`] trajectory, skips to the first pixel of opposite color, then repeatedly
/// finds a `0`→`1` (ridge start) followed by a `1`→`0` (ridge end) transition, counting each pair that
/// [`validate_ridge_crossing`] accepts as a genuine crossing.
///
/// # Errors
///
/// Propagates the [`line_points`] overflow error (`-412`), unreachable for these trajectories but kept
/// for a faithful transcription. The stock `< 0` validation system-error path has no port analogue.
fn ridge_count(
    first: usize,
    second: i32,
    minutiae: &[DetMinutia],
    bdata: &[u8],
    iw: i32,
    ih: i32,
    lfsparms: &LfsParms,
) -> Result<i32, i32> {
    let second = second as usize;
    let m1 = &minutiae[first];
    let m2 = &minutiae[second];

    // PORT L560–L564: identical coordinates → zero ridges between them.
    if m1.x == m2.x && m1.y == m2.y {
        return Ok(0);
    }

    // PORT L566–L571: contiguous pixel trajectory between the two points.
    let pts = line_points(m1.x, m1.y, m2.x, m2.y)?;
    let num = pts.len();

    // PORT L573–L579: no trajectory points → nothing to count (defensive, unreachable).
    if num == 0 {
        return Ok(0);
    }

    // PORT L581–L593: skip forward to the first pixel opposite the starting pixel's color.
    let prevpix = bdata[(pts[0].1 * iw + pts[0].0) as usize];
    let mut i = 1usize;
    let mut found = false;
    while i < num {
        let curpix = bdata[(pts[i].1 * iw + pts[i].0) as usize];
        if curpix != prevpix {
            found = true;
            break;
        }
        i += 1;
    }

    // PORT L595–L600: never changed color → no ridges to count.
    if !found {
        return Ok(0);
    }

    // PORT L603: ready to count.
    let mut ridge_count = 0;

    // PORT L608–L674: alternately find ridge-start / ridge-end transitions and validate each.
    while i < num {
        // PORT L610–L620: no more 0→1 ridge starts → done.
        if !find_transition(&mut i, 0, 1, &pts, num, bdata, iw) {
            return Ok(ridge_count);
        }
        let ridge_start = i;

        // PORT L627–L637: no matching 1→0 ridge end → done.
        if !find_transition(&mut i, 1, 0, &pts, num, bdata, iw) {
            return Ok(ridge_count);
        }
        let ridge_end = i;

        // PORT L651–L670: a validated crossing bumps the count; otherwise seek the next ridge start.
        if validate_ridge_crossing(
            ridge_start,
            ridge_end,
            &pts,
            bdata,
            iw,
            ih,
            lfsparms.max_ridge_steps,
        ) {
            ridge_count += 1;
        }
    }

    // PORT L683: trajectory exhausted.
    Ok(ridge_count)
}

/// Fill in one minutia's neighbors and their ridge counts — port of stock `count_minutia_ridges`
/// (`ridges.c` L144).
///
/// Finds `first`'s closest neighbors ([`find_neighbors`]), orders them by bearing ([`sort_neighbors`]),
/// counts the ridges to each ([`ridge_count`]), and stores both lists on `minutiae[first]`. A minutia
/// with no neighbors is left with empty lists (stock `num_nbrs == 0`).
///
/// # Errors
///
/// Propagates [`ridge_count`]'s error.
fn count_minutia_ridges(
    first: usize,
    minutiae: &mut [DetMinutia],
    bdata: &[u8],
    iw: i32,
    ih: i32,
    lfsparms: &LfsParms,
) -> Result<(), i32> {
    // PORT L151–L155: up to `max_nbrs` closest neighbors.
    let nbr_list = find_neighbors(lfsparms.max_nbrs, first, minutiae);

    // PORT L160–L164: no neighbors → nothing to store.
    if nbr_list.is_empty() {
        return Ok(());
    }

    // PORT L166–L170: order the neighbors by bearing.
    let nbr_list = sort_neighbors(nbr_list, first, minutiae);

    // PORT L172–L196: ridge count to each neighbor, in sorted order.
    let mut nbr_nridges = Vec::with_capacity(nbr_list.len());
    for &nbr in &nbr_list {
        nbr_nridges.push(ridge_count(first, nbr, minutiae, bdata, iw, ih, lfsparms)?);
    }

    // PORT L199–L201: assign the neighbor indices and ridge counts to the primary minutia.
    minutiae[first].nbrs = nbr_list;
    minutiae[first].ridge_counts = nbr_nridges;

    Ok(())
}

/// Find neighbors and count intervening ridges for every minutia — port of stock
/// `count_minutiae_ridges` (`ridges.c` L93).
///
/// Sorts the list column-oriented ([`sort_minutiae_x_y`]), drops coincident duplicates
/// ([`rm_dup_minutiae`]) — the two steps that fix the **order and count** of the final minutiae list —
/// then, for each minutia but the last, fills its neighbor / ridge-count lists
/// ([`count_minutia_ridges`]). `bdata` is the binary image (`0 == white/valley`, `1 == black/ridge`),
/// `iw`×`ih` pixels.
///
/// # Errors
///
/// Propagates [`count_minutia_ridges`]'s error (`line_points` overflow, unreachable here).
pub(crate) fn count_minutiae_ridges(
    minutiae: &mut Vec<DetMinutia>,
    bdata: &[u8],
    iw: i32,
    ih: i32,
    lfsparms: &LfsParms,
) -> Result<(), i32> {
    // PORT L102–L105: column-oriented sort (x then y).
    sort_minutiae_x_y(minutiae, iw);

    // PORT L107–L110: remove exact-coordinate duplicates.
    rm_dup_minutiae(minutiae);

    // PORT L112–L120: for each remaining minutia but the last, find neighbors and count ridges.
    for i in 0..minutiae.len().saturating_sub(1) {
        count_minutia_ridges(i, minutiae, bdata, iw, ih, lfsparms)?;
    }

    // PORT L123: done.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::{BIFURCATION, RIDGE_ENDING};
    use crate::params::LFSPARMS_V2;

    // Minimal DetMinutia builder — only the fields the ridge stage reads (coordinates, kind, and the
    // neighbor lists it fills) matter here.
    fn mk(x: i32, y: i32) -> DetMinutia {
        DetMinutia {
            x,
            y,
            ex: x,
            ey: y,
            direction: 0,
            reliability: 0.99,
            kind: RIDGE_ENDING,
            appearing: true,
            feature_id: 0,
            nbrs: Vec::new(),
            ridge_counts: Vec::new(),
        }
    }

    #[test]
    fn sort_minutiae_x_y_orders_by_column_then_row() {
        // Deliberately out of order; iw large so x dominates the rank.
        let mut ms = vec![mk(2, 5), mk(0, 9), mk(2, 1), mk(0, 3)];
        sort_minutiae_x_y(&mut ms, 100);
        let coords: Vec<(i32, i32)> = ms.iter().map(|m| (m.x, m.y)).collect();
        assert_eq!(coords, vec![(0, 3), (0, 9), (2, 1), (2, 5)]);
    }

    #[test]
    fn rm_dup_minutiae_drops_coincident_pairs() {
        // Sorted, with a coincident pair (1,1)/(1,1) and a triple (4,4).
        let mut ms = vec![mk(0, 0), mk(1, 1), mk(1, 1), mk(4, 4), mk(4, 4), mk(4, 4)];
        rm_dup_minutiae(&mut ms);
        let coords: Vec<(i32, i32)> = ms.iter().map(|m| (m.x, m.y)).collect();
        assert_eq!(coords, vec![(0, 0), (1, 1), (4, 4)]);
    }

    #[test]
    fn find_neighbors_returns_k_nearest_in_distance_order() {
        // Primary at index 0; suffix candidates, already x_y-sorted.
        // Distances^2 from (0,0): (1,1)=2, (2,0)=4, (3,3)=18, (4,0)=16, (5,1)=26.
        let ms = vec![mk(0, 0), mk(1, 1), mk(2, 0), mk(3, 3), mk(4, 0), mk(5, 1)];
        let nbrs = find_neighbors(5, 0, &ms);
        // Ascending squared distance: 2, 4, 16, 18, 26 → indices 1, 2, 4, 3, 5.
        assert_eq!(nbrs, vec![1, 2, 4, 3, 5]);
    }

    #[test]
    fn find_neighbors_caps_at_max_nbrs_and_breaks_on_x() {
        // Only the three closest are kept when max_nbrs = 3.
        let ms = vec![mk(0, 0), mk(1, 1), mk(2, 0), mk(3, 3), mk(4, 0), mk(5, 1)];
        let nbrs = find_neighbors(3, 0, &ms);
        assert_eq!(nbrs, vec![1, 2, 4]);
    }

    #[test]
    fn sort_neighbors_orders_vertical_then_clockwise() {
        // Primary (10,10); A above (10,0), B right (20,10), C below (10,20).
        let ms = vec![mk(10, 10), mk(10, 0), mk(20, 10), mk(10, 20)];
        // Feed them out of order; expect vertical origin, clockwise: A(1), B(2), C(3).
        let sorted = sort_neighbors(vec![3, 2, 1], 0, &ms);
        assert_eq!(sorted, vec![1, 2, 3]);
    }

    #[test]
    fn find_transition_locates_pair_and_reports_end() {
        // 4-wide, 1-tall image: white white black white → 0 0 1 0.
        let bdata = [0u8, 0, 1, 0];
        let pts = [(0, 0), (1, 0), (2, 0), (3, 0)];
        // 0→1 transition: pair (1,2) → *iptr becomes 2 (the '1').
        let mut i = 0usize;
        assert!(find_transition(&mut i, 0, 1, &pts, 4, &bdata, 4));
        assert_eq!(i, 2);
        // 1→0 transition from there: pair (2,3) → *iptr becomes 3.
        assert!(find_transition(&mut i, 1, 0, &pts, 4, &bdata, 4));
        assert_eq!(i, 3);
        // No further 0→1 → false, *iptr set past the end.
        let mut j = 3usize;
        assert!(!find_transition(&mut j, 0, 1, &pts, 4, &bdata, 4));
        assert_eq!(j, 4);
    }

    #[test]
    fn ridge_count_identical_points_is_zero() {
        let ms = vec![mk(5, 5), mk(5, 5)];
        let bdata = vec![0u8; 25];
        let rc = ridge_count(0, 1, &ms, &bdata, 5, 5, &LFSPARMS_V2).unwrap();
        assert_eq!(rc, 0);
    }

    #[test]
    fn ridge_count_no_transition_is_zero() {
        // All-white image: no color change along the trajectory.
        let ms = vec![mk(1, 2), mk(3, 2)];
        let bdata = vec![0u8; 25];
        let rc = ridge_count(0, 1, &ms, &bdata, 5, 5, &LFSPARMS_V2).unwrap();
        assert_eq!(rc, 0);
    }

    #[test]
    fn ridge_count_counts_a_validated_crossing() {
        // 25x25 image, two full-height black ridge columns at x=6 and x=13.
        // The horizontal trajectory at y=12 skips past column 6 (the first color change) and then
        // counts the column-13 crossing, which validates (a full-height ridge separates the left and
        // right white regions, so neither contour trace loops back).
        let iw = 25i32;
        let ih = 25i32;
        let mut bdata = vec![0u8; (iw * ih) as usize];
        for y in 0..ih {
            bdata[(y * iw + 6) as usize] = 1;
            bdata[(y * iw + 13) as usize] = 1;
        }
        let ms = vec![mk(2, 12), mk(20, 12)];
        let rc = ridge_count(0, 1, &ms, &bdata, iw, ih, &LFSPARMS_V2).unwrap();
        assert_eq!(rc, 1);
    }

    #[test]
    fn count_minutiae_ridges_sorts_dedups_and_fills() {
        // Two coincident minutiae plus others; after sort+dedup the count drops and neighbors fill.
        let iw = 25i32;
        let ih = 25i32;
        let bdata = vec![0u8; (iw * ih) as usize];
        let mut ms = vec![mk(20, 12), mk(2, 12), mk(2, 12), mk(10, 8), mk(15, 4)];
        count_minutiae_ridges(&mut ms, &bdata, iw, ih, &LFSPARMS_V2).unwrap();
        // The coincident (2,12) pair collapses to one; 5 → 4 minutiae, sorted by x then y.
        let coords: Vec<(i32, i32)> = ms.iter().map(|m| (m.x, m.y)).collect();
        assert_eq!(coords, vec![(2, 12), (10, 8), (15, 4), (20, 12)]);
        // Every minutia but the last gets a (possibly empty, but here non-empty) neighbor list, and
        // the two parallel lists always share a length.
        for m in &ms {
            assert_eq!(m.nbrs.len(), m.ridge_counts.len());
        }
        assert!(!ms[0].nbrs.is_empty());
    }

    #[test]
    fn bifurcation_and_ending_kinds_are_both_usable() {
        // Sanity: the builder's kind field is independent of the ridge stage's list operations.
        let mut a = mk(0, 0);
        a.kind = BIFURCATION;
        assert_eq!(a.kind, BIFURCATION);
        assert_eq!(mk(0, 0).kind, RIDGE_ENDING);
    }
}
