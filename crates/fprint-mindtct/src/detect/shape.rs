// SPDX-FileCopyrightText: 2026 fprintd (pure-Rust) contributors
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Per-row shape representation of a closed loop contour — port of stock NBIS
//! `mindtct/src/lib/mindtct/shape.c` (`alloc_shape`, `shape_from_contour`, `sort_row_on_x`, and the
//! implicit `free_shape` folded into `Vec` ownership).
//!
//! [`fill_loop`](super::loops::fill_loop) needs, for each scanline the loop spans, the loop's contour
//! points on that row sorted left-to-right so it can fill between them while skipping the gaps that
//! concavities open up. A [`Shape`] holds one [`Row`] per scanline of the loop's bounding box, each
//! carrying the distinct x-coordinates of the contour points on that row. See
//! `docs/mindtct-algorithm.md`.

use crate::num::bubble_sort_int_inc;

use super::contour::contour_limits;

/// One scanline of a [`Shape`] — the port's analogue of the stock `ROW` (`lfs.h`), one field per
/// member that survives the `Vec` port.
///
/// PORT: stock `ROW` carries `y`, `xs`, `alloc`, and `npts`; `alloc`/`npts` collapse into the `Vec`'s
/// capacity/length, leaving `y` and `xs`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    /// Y-coordinate shared by every contour point on this row (stock `y`).
    pub y: i32,
    /// X-coordinates of the row's contour points, left-to-right after [`shape_from_contour`] sorts
    /// them (stock `xs` / `npts`).
    pub xs: Vec<i32>,
}

/// A loop contour reorganized by scanline — the port's analogue of the stock `SHAPE` (`lfs.h`).
///
/// PORT: stock `SHAPE` carries `ymin`, `ymax`, `nrows`, `alloc`, and `rows`; `nrows`/`alloc` collapse
/// into the `rows` `Vec`. `free_shape` is `Vec` `Drop`, so it has no port-side routine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Shape {
    /// One row per scanline of the loop's bounding box, top (`ymin`) to bottom (`ymax`).
    pub rows: Vec<Row>,
}

/// Allocate a shape spanning scanlines `ymin..=ymax`, each row initially empty — port of stock
/// `alloc_shape` (`shape.c` L84).
///
/// PORT: the reference pre-sizes each row's `xs` to `xmax - xmin + 1` (the maximum contiguous pixels
/// possible on a row); here that becomes the `Vec` capacity hint, and the `-250..=-253` malloc-failure
/// paths are unreachable (allocation aborts) and elided. `xmax` is used only for that hint.
fn alloc_shape(xmin: i32, ymin: i32, xmax: i32, ymax: i32) -> Shape {
    // PORT L91–L96: rows span the shape's y-limits; each may hold up to `xmax - xmin + 1` points.
    let alloc_rows = ymax - ymin + 1;
    let alloc_pts = (xmax - xmin + 1).max(0) as usize;

    // PORT L127–L166: one empty row per scanline, carrying its y-coordinate.
    let mut rows = Vec::with_capacity(alloc_rows.max(0) as usize);
    for i in 0..alloc_rows {
        rows.push(Row {
            y: ymin + i,
            xs: Vec::with_capacity(alloc_pts),
        });
    }

    Shape { rows }
}

/// Reorganize a closed-loop contour into a per-row [`Shape`] — port of stock `shape_from_contour`
/// (`shape.c` L250).
///
/// Bins each contour point onto the row of its y-coordinate (skipping x-coordinates already recorded
/// for that row, as complex "pinching" contours revisit points), then sorts each row left-to-right on
/// x. `contour_x`/`contour_y` are the loop's contour points (assumed non-empty by the caller).
///
/// # Errors
///
/// `Err(-260)` on the stock "row overflow" path — unreachable here (rows are sized to the bounding
/// box), retained for a faithful transcription.
pub(crate) fn shape_from_contour(contour_x: &[i32], contour_y: &[i32]) -> Result<Shape, i32> {
    // PORT L262–L263: bounding box of the contour.
    let (xmin, ymin, xmax, ymax) = contour_limits(contour_x, contour_y);

    // PORT L266: allocate the empty per-scanline shape.
    let mut shape = alloc_shape(xmin, ymin, xmax, ymax);

    // PORT L269–L295: bin each contour point onto its row, de-duplicating x-coordinates.
    for i in 0..contour_x.len() {
        // PORT L274–L277: rows are indexed relative to the top-most scanline.
        let row = &mut shape.rows[(contour_y[i] - ymin) as usize];
        // PORT L282–L294: record the x-coordinate unless the row already holds it.
        if !row.xs.contains(&contour_x[i]) {
            // PORT L284–L289: a full row is impossible given the bounding-box sizing.
            if row.xs.len() >= row.xs.capacity() {
                return Err(-260);
            }
            row.xs.push(contour_x[i]);
        }
    }

    // PORT L298–L300: sort each row's points increasing on x.
    for row in &mut shape.rows {
        sort_row_on_x(row);
    }

    // PORT L303–L306: return the shape.
    Ok(shape)
}

/// Sort a row's x-coordinates left-to-right — port of stock `sort_row_on_x` (`shape.c` L317).
///
/// A stable increasing bubble sort, verbatim from the reference (the point count is small).
fn sort_row_on_x(row: &mut Row) {
    // PORT L322: increasing bubble sort of the row's x-coords.
    bubble_sort_int_inc(&mut row.xs);
}
