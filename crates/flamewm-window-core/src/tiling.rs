//! Pure auto-tiling layouts.
//!
//! Every function maps a work area and a window count to a list of [`Rect`] tiles; no server,
//! display, or backend state is read or mutated. Tiles are emitted in window order (index `i`
//! of the input corresponds to tile `i` of the result for non-degenerate inputs).
//!
//! With `gap == 0` the tiles form an exact partition of `work_area`: they never overlap and their
//! combined area equals the work area. With `gap > 0` the same partition is inset so that every
//! shared edge is separated by exactly `gap` pixels and the outer edge is inset by `gap` as well
//! (the same convention as [`crate::apply_window_margin`]). Degenerate slots thinner than the gap
//! are dropped rather than producing invalid rectangles.

use flamewm_api::Rect;

/// Split `rect` into `count` horizontal bands (stacked top to bottom) with exact integer
/// coverage: remainder pixels are given to the first rows.
fn split_rows(rect: Rect, count: usize) -> Vec<Rect> {
    if count == 0 {
        return Vec::new();
    }
    let count_i = count as i32;
    let base = rect.height / count_i;
    let rem = rect.height % count_i;
    let mut y = rect.y;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let height = base + i32::from(i < rem as usize);
        out.push(Rect::new(rect.x, y, rect.width, height));
        y += height;
    }
    out
}

/// Split `rect` into `count` vertical bands (side by side, left to right) with exact integer
/// coverage: remainder pixels are given to the first columns.
fn split_cols(rect: Rect, count: usize) -> Vec<Rect> {
    if count == 0 {
        return Vec::new();
    }
    let count_i = count as i32;
    let base = rect.width / count_i;
    let rem = rect.width % count_i;
    let mut x = rect.x;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let width = base + i32::from(i < rem as usize);
        out.push(Rect::new(x, rect.y, width, rect.height));
        x += width;
    }
    out
}

/// Inset a zero-gap `tile` so shared edges leave `gap` pixels and edges on the work-area border
/// are inset by the full `gap`. Returns `None` when the inset leaves no valid area.
fn with_gap(tile: Rect, work_area: Rect, gap: i32) -> Option<Rect> {
    if gap <= 0 {
        return Some(tile);
    }
    let leading = gap - gap / 2; // ceil(gap / 2): left/top of a shared edge
    let trailing = gap / 2; // floor(gap / 2): right/bottom of a shared edge
    let left = if tile.x <= work_area.x { gap } else { leading };
    let top = if tile.y <= work_area.y { gap } else { leading };
    let right = if tile.right() >= work_area.right() {
        gap
    } else {
        trailing
    };
    let bottom = if tile.bottom() >= work_area.bottom() {
        gap
    } else {
        trailing
    };
    let width = tile.width.saturating_sub(left.saturating_add(right));
    let height = tile.height.saturating_sub(top.saturating_add(bottom));
    if width <= 0 || height <= 0 {
        return None;
    }
    Some(Rect::new(
        tile.x.saturating_add(left),
        tile.y.saturating_add(top),
        width,
        height,
    ))
}

/// Fraction of `extent` taken by the first half of a dwindle split, clamped so neither side is
/// negative and the two halves always sum back to `extent`.
fn split_extent(extent: i32, ratio: f32) -> i32 {
    let first = (extent as f32 * ratio).round() as i32;
    first.clamp(0, extent)
}

/// Master-stack layout: up to `master_count` masters occupy a left column of width
/// `master_factor * work_area.width`; the remaining windows stack in the right column. Each
/// column is divided into equal-height bands. `master_factor` is clamped to `[0.0, 1.0]` and
/// `master_count` to `[0, n]`.
#[must_use]
pub fn master_stack(
    work_area: Rect,
    n: usize,
    master_count: usize,
    master_factor: f32,
    gap: i32,
) -> Vec<Rect> {
    if n == 0 || !work_area.is_valid() {
        return Vec::new();
    }
    let masters = master_count.min(n);
    let stack = n - masters;
    let factor = master_factor.clamp(0.0, 1.0);
    let master_w = if stack == 0 {
        work_area.width
    } else if masters == 0 {
        0
    } else {
        ((work_area.width as f32) * factor).round().clamp(0.0, work_area.width as f32) as i32
    };

    let mut tiles = Vec::with_capacity(n);
    if masters > 0 {
        let area = Rect::new(work_area.x, work_area.y, master_w, work_area.height);
        tiles.extend(split_rows(area, masters));
    }
    if stack > 0 {
        let area = Rect::new(
            work_area.x + master_w,
            work_area.y,
            work_area.width - master_w,
            work_area.height,
        );
        tiles.extend(split_rows(area, stack));
    }
    tiles
        .into_iter()
        .filter_map(|tile| with_gap(tile, work_area, gap))
        .collect()
}

/// Columns layout: `column_count` equal-width columns (clamped to `[1, n]`), each split into
/// equal-height bands. Windows fill columns left to right, top to bottom; the extra windows of an
/// uneven division go to the leftmost columns so every column stays fully occupied.
#[must_use]
pub fn columns(work_area: Rect, n: usize, column_count: usize, gap: i32) -> Vec<Rect> {
    if n == 0 || !work_area.is_valid() {
        return Vec::new();
    }
    let cols = column_count.clamp(1, n);
    let base = n / cols;
    let rem = n % cols;
    let mut tiles = Vec::with_capacity(n);
    for (i, col) in split_cols(work_area, cols).into_iter().enumerate() {
        let count = base + usize::from(i < rem);
        tiles.extend(split_rows(col, count));
    }
    tiles
        .into_iter()
        .filter_map(|tile| with_gap(tile, work_area, gap))
        .collect()
}

/// Monocle layout: one maximized window is visible at a time and the rest are stacked directly
/// behind it, so all `n` tiles share the same geometry (the work area inset by `gap` on every
/// side). Overlap is intentional here; there is no master/stack or grid partition.
#[must_use]
pub fn monocle(work_area: Rect, n: usize, gap: i32) -> Vec<Rect> {
    if n == 0 || !work_area.is_valid() {
        return Vec::new();
    }
    match with_gap(work_area, work_area, gap) {
        Some(full) => vec![full; n],
        None => Vec::new(),
    }
}

/// Dwindle layout: a deterministic binary-space spiral. Starting from the whole work area, each
/// split alternates between a vertical and a horizontal cut at `split_ratio` (clamped to
/// `[0.05, 0.95]`), peeling off one tile and recursing into the remainder. The last window takes
/// the final remainder.
#[must_use]
pub fn dwindle(work_area: Rect, n: usize, split_ratio: f32, gap: i32) -> Vec<Rect> {
    if n == 0 || !work_area.is_valid() {
        return Vec::new();
    }
    let ratio = split_ratio.clamp(0.05, 0.95);
    let mut tiles = Vec::with_capacity(n);
    let mut current = work_area;
    let mut vertical = true;
    for _ in 0..n - 1 {
        let (first, rest) = if vertical {
            let w = split_extent(current.width, ratio);
            (
                Rect::new(current.x, current.y, w, current.height),
                Rect::new(current.x + w, current.y, current.width - w, current.height),
            )
        } else {
            let h = split_extent(current.height, ratio);
            (
                Rect::new(current.x, current.y, current.width, h),
                Rect::new(current.x, current.y + h, current.width, current.height - h),
            )
        };
        tiles.push(first);
        current = rest;
        vertical = !vertical;
    }
    tiles.push(current);
    tiles
        .into_iter()
        .filter_map(|tile| with_gap(tile, work_area, gap))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No two tiles share any pixel. Zero-area tiles never intersect.
    fn assert_no_overlap(rects: &[Rect]) {
        for (i, a) in rects.iter().enumerate() {
            for b in &rects[i + 1..] {
                assert!(!a.intersects(*b), "tiles overlap: {a:?} and {b:?}");
            }
        }
    }

    /// With `gap == 0` the tiles exactly cover the work area and stay inside it.
    fn assert_fills_area(rects: &[Rect], area: Rect) {
        let covered: i64 = rects
            .iter()
            .map(|r| i64::from(r.width) * i64::from(r.height))
            .sum();
        assert_eq!(
            covered,
            i64::from(area.width) * i64::from(area.height),
            "tiles do not cover {area:?}: {rects:?}"
        );
        for r in rects {
            assert!(
                r.x >= area.x && r.y >= area.y && r.right() <= area.right() && r.bottom() <= area.bottom(),
                "tile {r:?} escapes {area:?}"
            );
        }
    }

    /// With `gap > 0` every tile stays inside the work area inset by `gap`.
    fn assert_inside_inset(rects: &[Rect], area: Rect, gap: i32) {
        for r in rects {
            assert!(
                r.x >= area.x + gap
                    && r.y >= area.y + gap
                    && r.right() <= area.right() - gap
                    && r.bottom() <= area.bottom() - gap,
                "tile {r:?} is not inset by {gap} inside {area:?}"
            );
        }
    }

    // ------------------------------------------------------------------ master_stack

    #[test]
    fn master_stack_zero_gap_never_overlaps_and_fills_area() {
        let area = Rect::new(40, 24, 1920, 1080);
        for n in 1..=10 {
            for masters in 0..=n + 1 {
                for factor in [0.2_f32, 0.5, 0.55, 0.8] {
                    let tiles = master_stack(area, n, masters, factor, 0);
                    assert_eq!(tiles.len(), n, "n={n} masters={masters} factor={factor}");
                    assert_no_overlap(&tiles);
                    assert_fills_area(&tiles, area);
                }
            }
        }
    }

    #[test]
    fn master_stack_exact_half_split_rects() {
        let area = Rect::new(0, 0, 1000, 500);
        let tiles = master_stack(area, 2, 1, 0.5, 0);
        assert_eq!(
            tiles,
            vec![Rect::new(0, 0, 500, 500), Rect::new(500, 0, 500, 500)]
        );
    }

    #[test]
    fn master_stack_two_masters_three_stack_exact_rects() {
        let area = Rect::new(0, 0, 1000, 600);
        let tiles = master_stack(area, 5, 2, 0.5, 0);
        assert_eq!(
            tiles,
            vec![
                Rect::new(0, 0, 500, 300),
                Rect::new(0, 300, 500, 300),
                Rect::new(500, 0, 500, 200),
                Rect::new(500, 200, 500, 200),
                Rect::new(500, 400, 500, 200),
            ]
        );
    }

    #[test]
    fn master_stack_gaps_are_uniform_between_and_around_tiles() {
        let area = Rect::new(0, 0, 1000, 600);
        let tiles = master_stack(area, 2, 1, 0.5, 10);
        assert_eq!(
            tiles,
            vec![Rect::new(10, 10, 485, 580), Rect::new(505, 10, 485, 580)]
        );
        assert_eq!(tiles[1].x - tiles[0].right(), 10);
        assert_no_overlap(&tiles);
        assert_inside_inset(&tiles, area, 10);
    }

    #[test]
    fn master_stack_handles_degenerate_counts_and_factors() {
        let area = Rect::new(0, 0, 101, 51);
        assert!(master_stack(area, 0, 1, 0.5, 0).is_empty());
        // master_count beyond n means every window is a master (full-width stack of rows).
        let all_masters = master_stack(area, 3, 9, 0.5, 0);
        assert_eq!(all_masters.len(), 3);
        assert!(all_masters.iter().all(|r| r.width == area.width));
        assert_no_overlap(&all_masters);
        assert_fills_area(&all_masters, area);
        // master_count == 0 means every window is a stack window.
        let all_stack = master_stack(area, 3, 0, 0.5, 0);
        assert_eq!(all_stack.len(), 3);
        assert!(all_stack.iter().all(|r| r.width == area.width));
        assert_fills_area(&all_stack, area);
        // Factors outside [0, 1] are clamped and still cover the area.
        let clamped = master_stack(area, 2, 1, 5.0, 0);
        assert_no_overlap(&clamped);
        assert_fills_area(&clamped, area);
    }

    #[test]
    fn master_stack_respects_negative_origins_and_odd_remainders() {
        let area = Rect::new(-1920, 30, 1001, 701);
        let tiles = master_stack(area, 4, 1, 0.55, 0);
        assert_eq!(tiles.len(), 4);
        assert_no_overlap(&tiles);
        assert_fills_area(&tiles, area);
        let stack_height: i32 = tiles[1..].iter().map(|r| r.height).sum();
        assert_eq!(stack_height, area.height, "stack rows tile the full height");
    }

    // ------------------------------------------------------------------------ columns

    #[test]
    fn columns_zero_gap_never_overlaps_and_fills_area() {
        let area = Rect::new(0, 0, 1920, 1080);
        for n in 1..=12 {
            for cols in 1..=n + 2 {
                let tiles = columns(area, n, cols, 0);
                assert_eq!(tiles.len(), n, "n={n} cols={cols}");
                assert_no_overlap(&tiles);
                assert_fills_area(&tiles, area);
            }
        }
    }

    #[test]
    fn columns_even_grid_exact_rects() {
        let area = Rect::new(0, 0, 900, 600);
        let tiles = columns(area, 6, 3, 0);
        assert_eq!(
            tiles,
            vec![
                Rect::new(0, 0, 300, 300),
                Rect::new(0, 300, 300, 300),
                Rect::new(300, 0, 300, 300),
                Rect::new(300, 300, 300, 300),
                Rect::new(600, 0, 300, 300),
                Rect::new(600, 300, 300, 300),
            ]
        );
    }

    #[test]
    fn columns_uneven_counts_fill_every_column() {
        let area = Rect::new(0, 0, 900, 600);
        let tiles = columns(area, 5, 3, 0);
        assert_eq!(
            tiles,
            vec![
                Rect::new(0, 0, 300, 300),
                Rect::new(0, 300, 300, 300),
                Rect::new(300, 0, 300, 300),
                Rect::new(300, 300, 300, 300),
                Rect::new(600, 0, 300, 600),
            ]
        );
    }

    #[test]
    fn columns_gaps_are_uniform_between_and_around_tiles() {
        let area = Rect::new(0, 0, 600, 400);
        let tiles = columns(area, 4, 2, 8);
        assert_eq!(
            tiles,
            vec![
                Rect::new(8, 8, 288, 188),
                Rect::new(8, 204, 288, 188),
                Rect::new(304, 8, 288, 188),
                Rect::new(304, 204, 288, 188),
            ]
        );
        assert_eq!(tiles[2].x - tiles[0].right(), 8); // the shared vertical edge is exactly the gap
        assert_no_overlap(&tiles);
        assert_inside_inset(&tiles, area, 8);
    }

    #[test]
    fn columns_clamps_column_count_and_empty_input() {
        let area = Rect::new(0, 0, 100, 100);
        assert!(columns(area, 0, 3, 0).is_empty());
        // More columns than windows degenerates to one window per column.
        let wide = columns(area, 2, 7, 0);
        assert_eq!(wide.len(), 2);
        assert_eq!(wide[0], Rect::new(0, 0, 50, 100));
        assert_eq!(wide[1], Rect::new(50, 0, 50, 100));
        // A zero column count is clamped to one.
        let single = columns(area, 3, 0, 0);
        assert_eq!(single.len(), 3);
        assert!(single.iter().all(|r| r.width == area.width));
        assert_fills_area(&single, area);
    }

    // ------------------------------------------------------------------------ monocle

    #[test]
    fn monocle_stacks_every_window_over_the_full_area() {
        let area = Rect::new(0, 0, 1280, 720);
        let tiles = monocle(area, 4, 0);
        assert_eq!(tiles, vec![area; 4]);
    }

    #[test]
    fn monocle_gap_insets_every_window_uniformly() {
        let area = Rect::new(10, 20, 1280, 720);
        let tiles = monocle(area, 3, 6);
        let expected = Rect::new(16, 26, 1268, 708);
        assert_eq!(tiles, vec![expected; 3]);
        assert_inside_inset(&tiles, area, 6);
    }

    #[test]
    fn monocle_zero_windows_is_empty() {
        assert!(monocle(Rect::new(0, 0, 100, 100), 0, 0).is_empty());
    }

    // ------------------------------------------------------------------------ dwindle

    #[test]
    fn dwindle_zero_gap_never_overlaps_and_fills_area() {
        let area = Rect::new(0, 0, 1920, 1080);
        for n in 1..=16 {
            for ratio in [0.3_f32, 0.5, 0.7] {
                let tiles = dwindle(area, n, ratio, 0);
                assert_eq!(tiles.len(), n, "n={n} ratio={ratio}");
                assert_no_overlap(&tiles);
                assert_fills_area(&tiles, area);
            }
        }
    }

    #[test]
    fn dwindle_two_windows_are_vertical_halves() {
        let area = Rect::new(0, 0, 1000, 400);
        let tiles = dwindle(area, 2, 0.5, 0);
        assert_eq!(
            tiles,
            vec![Rect::new(0, 0, 500, 400), Rect::new(500, 0, 500, 400)]
        );
    }

    #[test]
    fn dwindle_three_windows_form_the_classic_spiral() {
        let area = Rect::new(0, 0, 400, 400);
        let tiles = dwindle(area, 3, 0.5, 0);
        assert_eq!(
            tiles,
            vec![
                Rect::new(0, 0, 200, 400),
                Rect::new(200, 0, 200, 200),
                Rect::new(200, 200, 200, 200),
            ]
        );
    }

    #[test]
    fn dwindle_honors_a_biased_split_ratio() {
        let area = Rect::new(0, 0, 1000, 1000);
        let tiles = dwindle(area, 2, 0.6, 0);
        assert_eq!(
            tiles,
            vec![Rect::new(0, 0, 600, 1000), Rect::new(600, 0, 400, 1000)]
        );
    }

    #[test]
    fn dwindle_gaps_are_uniform_and_do_not_overlap() {
        let area = Rect::new(0, 0, 400, 400);
        let tiles = dwindle(area, 4, 0.5, 2);
        assert_eq!(tiles.len(), 4);
        assert_no_overlap(&tiles);
        assert_inside_inset(&tiles, area, 2);
        // Shared edge between the first (left) tile and its neighbour is exactly the gap.
        assert_eq!(tiles[1].x - tiles[0].right(), 2);
    }

    #[test]
    fn dwindle_clamps_ratio_and_empty_input() {
        let area = Rect::new(0, 0, 200, 200);
        assert!(dwindle(area, 0, 0.5, 0).is_empty());
        for ratio in [-1.0_f32, 0.0, 1.0, 50.0] {
            let tiles = dwindle(area, 3, ratio, 0);
            assert_eq!(tiles.len(), 3);
            assert_no_overlap(&tiles);
            assert_fills_area(&tiles, area);
        }
    }
}
