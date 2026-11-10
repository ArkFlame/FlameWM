//! Pure window geometry policy.
//!
//! This crate owns no server, display, or renderer state.  Backends feed it the current
//! geometry and apply the returned geometry on their owning thread.

use flamewm_api::{Point, Rect, Size};

/// Default `edgeSnap.thresholdPx`; the settings default is the same value.
pub const EDGE_THRESHOLD: i32 = 16;
/// Corner zones are twice the edge zone; this is the default `EDGE_THRESHOLD` expansion.
pub const CORNER_THRESHOLD: i32 = 2 * EDGE_THRESHOLD;

pub mod dock_reserve;
pub mod geometry_memory;
pub mod keyboard_tile;
pub mod strut;
pub mod window_rules;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum SnapTarget {
    #[default]
    None,
    LeftHalf,
    RightHalf,
    TopHalf,
    BottomHalf,
    TopLeftQuarter,
    TopRightQuarter,
    BottomLeftQuarter,
    BottomRightQuarter,
    Maximize,
}

#[must_use]
pub fn snap_target(pointer: Point, work_area: Rect) -> SnapTarget {
    snap_target_in_output(pointer, work_area, work_area)
}

/// Minimum visible extent on one axis: a quarter of the window, rounded up.
fn min_visible(extent: i32) -> i32 {
    extent.max(0).saturating_add(3) / 4
}

/// Start of a span of `len` so that at least a quarter stays inside `[lo, hi)`.
fn keep_span(start: i32, len: i32, lo: i32, hi: i32) -> i32 {
    let min_start = lo.saturating_sub(len.saturating_sub(min_visible(len)));
    let max_start = hi.saturating_sub(min_visible(len)).max(min_start);
    start.clamp(min_start, max_start)
}

/// Keep a rectangle within `bounds` with at least a quarter of each axis visible.
///
/// The bounds are the physical output rectangle, not a dock-reduced work area.
#[must_use]
pub fn keep_visible(rect: Rect, bounds: Rect) -> Rect {
    Rect::new(
        keep_span(
            rect.x,
            rect.width,
            bounds.x,
            bounds.x.saturating_add(bounds.width),
        ),
        keep_span(
            rect.y,
            rect.height,
            bounds.y,
            bounds.y.saturating_add(bounds.height),
        ),
        rect.width,
        rect.height,
    )
}

/// Brings `rect` fully inside `area`: the extent is capped to `area` (and raised to `min_extent`),
/// then the rect slides back in from the edge it crosses. Restores reappear where they were saved,
/// so they use this stricter rule instead of the quarter-visible drag rule of [`keep_visible`].
/// Backends choose `min_extent`: X11 restores keep at least 1 px, Wayland allows 0.
#[must_use]
pub fn fit_into(rect: Rect, area: Rect, min_extent: i32) -> Rect {
    let w = rect.width.min(area.width).max(min_extent);
    let h = rect.height.min(area.height).max(min_extent);
    let fit = |start: i32, len: i32, lo: i32, span: i32| {
        let hi = lo.saturating_add(span.saturating_sub(len)).max(lo);
        start.clamp(lo, hi)
    };
    Rect::new(
        fit(rect.x, w, area.x, area.width),
        fit(rect.y, h, area.y, area.height),
        w,
        h,
    )
}

/// Snap detection for a pointer on an output whose work area may be reserved by docks.
/// Left/top zones are measured from the work area; right/bottom zones extend to the
/// output edge so a pointer over a bottom/right dock band still reaches the physical corner.
#[must_use]
pub fn snap_target_in_output(pointer: Point, output: Rect, work_area: Rect) -> SnapTarget {
    snap_target_in_output_with(pointer, output, work_area, EDGE_THRESHOLD)
}

/// [`snap_target_in_output`] with an explicit `edgeSnap.thresholdPx`. The edge zone is `edge_px`
/// (clamped to at least 1) and the corner zone is twice that, so `EDGE_THRESHOLD` reproduces the
/// default behaviour exactly.
#[must_use]
pub fn snap_target_in_output_with(
    pointer: Point,
    output: Rect,
    work_area: Rect,
    edge_px: i32,
) -> SnapTarget {
    let edge = edge_px.max(1);
    let corner = edge.saturating_mul(2);
    let left = pointer.x >= work_area.x && pointer.x < work_area.x.saturating_add(edge);
    let right = pointer.x >= work_area.right().saturating_sub(edge)
        && pointer.x < work_area.right().max(output.right());
    let top = pointer.y >= work_area.y && pointer.y < work_area.y.saturating_add(corner);
    let bottom = pointer.y >= work_area.bottom().saturating_sub(corner)
        && pointer.y < work_area.bottom().max(output.bottom());
    match (left, right, top, bottom) {
        (true, _, true, _) => SnapTarget::TopLeftQuarter,
        (_, true, true, _) => SnapTarget::TopRightQuarter,
        (true, _, _, true) => SnapTarget::BottomLeftQuarter,
        (_, true, _, true) => SnapTarget::BottomRightQuarter,
        (_, _, true, _) => SnapTarget::Maximize,
        (_, _, _, true) => SnapTarget::None,
        (true, _, _, _) => SnapTarget::LeftHalf,
        (_, true, _, _) => SnapTarget::RightHalf,
        _ => SnapTarget::None,
    }
}

/// Snapped geometry for drag previews. Mirrors the committed frame snap rectangles: the
/// half extent is `floor(size / 2)` (min 1) and the right/bottom halves start at
/// `size - half` so odd work areas preview exactly what a release commits.
#[must_use]
pub fn snap_geometry(target: SnapTarget, work_area: Rect, current: Size) -> Rect {
    let half_w = (work_area.width / 2).max(1);
    let half_h = (work_area.height / 2).max(1);
    let right_x = work_area
        .x
        .saturating_add(work_area.width.saturating_sub(half_w));
    let bottom_y = work_area
        .y
        .saturating_add(work_area.height.saturating_sub(half_h));
    match target {
        SnapTarget::None => Rect::new(work_area.x, work_area.y, current.width, current.height),
        SnapTarget::LeftHalf => Rect::new(work_area.x, work_area.y, half_w, work_area.height),
        SnapTarget::RightHalf => Rect::new(right_x, work_area.y, half_w, work_area.height),
        SnapTarget::TopHalf => Rect::new(work_area.x, work_area.y, work_area.width, half_h),
        SnapTarget::BottomHalf => Rect::new(work_area.x, bottom_y, work_area.width, half_h),
        SnapTarget::TopLeftQuarter => Rect::new(work_area.x, work_area.y, half_w, half_h),
        SnapTarget::TopRightQuarter => Rect::new(right_x, work_area.y, half_w, half_h),
        SnapTarget::BottomLeftQuarter => Rect::new(work_area.x, bottom_y, half_w, half_h),
        SnapTarget::BottomRightQuarter => Rect::new(right_x, bottom_y, half_w, half_h),
        SnapTarget::Maximize => work_area,
    }
}

#[must_use]
pub fn apply_window_margin(tile: Rect, work_area: Rect, margin: i32) -> Rect {
    let margin = margin.max(0);
    if margin == 0 {
        return tile;
    }
    let leading_inner = margin - margin / 2;
    let trailing_inner = margin / 2;
    let left = if tile.x <= work_area.x {
        margin
    } else {
        leading_inner
    };
    let top = if tile.y <= work_area.y {
        margin
    } else {
        leading_inner
    };
    let right = if tile.right() >= work_area.right() {
        margin
    } else {
        trailing_inner
    };
    let bottom = if tile.bottom() >= work_area.bottom() {
        margin
    } else {
        trailing_inner
    };
    let width = tile.width.saturating_sub(left + right);
    let height = tile.height.saturating_sub(top + bottom);
    if width < 1 || height < 1 {
        return tile;
    }
    Rect::new(
        tile.x.saturating_add(left),
        tile.y.saturating_add(top),
        width,
        height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rect {
        Rect::new(0, 0, 1920, 1080)
    }

    #[test]
    fn keep_visible_retains_a_ceil_quarter_on_each_axis() {
        let rect = Rect::new(1169, -10_000, 880, 300);
        let output = Rect::new(0, 0, 1350, 900);
        assert_eq!(keep_visible(rect, output), Rect::new(1130, -225, 880, 300));
        assert_eq!(
            keep_visible(Rect::new(1169, 0, 881, 300), output),
            Rect::new(1129, 0, 881, 300),
            "a quarter of an odd extent rounds up",
        );
    }

    #[test]
    fn keep_visible_handles_negative_origins_and_saturated_bounds() {
        assert_eq!(
            keep_visible(
                Rect::new(-10_000, -10_000, 400, 300),
                Rect::new(-1920, 50, 1920, 1080),
            ),
            Rect::new(-2220, -175, 400, 300)
        );
        assert_eq!(
            keep_visible(
                Rect::new(i32::MAX, 0, 40, 40),
                Rect::new(i32::MAX - 10, 0, 100, 100),
            ),
            Rect::new(i32::MAX - 10, 0, 40, 40)
        );
    }

    #[test]
    fn edge_and_corner_precedence() {
        assert_eq!(
            snap_target(Point::new(0, 0), area()),
            SnapTarget::TopLeftQuarter
        );
        assert_eq!(
            snap_target(Point::new(1000, 0), area()),
            SnapTarget::Maximize
        );
        assert_eq!(
            snap_target(Point::new(0, 540), area()),
            SnapTarget::LeftHalf
        );
        assert_eq!(
            snap_target(Point::new(1000, 1079), area()),
            SnapTarget::None
        );
    }

    #[test]
    fn bottom_dock_physical_corners_snap_to_quarters() {
        // 1280x800 output with a 48px bottom dock: work area is 0,0,1280,752.
        let output = Rect::new(0, 0, 1280, 800);
        let work = Rect::new(0, 0, 1280, 752);
        assert_eq!(
            snap_target_in_output(Point::new(0, 799), output, work),
            SnapTarget::BottomLeftQuarter
        );
        assert_eq!(
            snap_target_in_output(Point::new(1279, 799), output, work),
            SnapTarget::BottomRightQuarter
        );
        assert_eq!(
            snap_target_in_output(Point::new(0, 751), output, work),
            SnapTarget::BottomLeftQuarter
        );
        assert_eq!(
            snap_target_in_output(Point::new(640, 799), output, work),
            SnapTarget::None
        );
        assert_eq!(
            snap_geometry(SnapTarget::BottomLeftQuarter, work, Size::new(800, 600)),
            Rect::new(0, 376, 640, 376)
        );
    }

    #[test]
    fn default_threshold_matches_the_legacy_constants() {
        let samples = [
            Point::new(0, 0),
            Point::new(15, 31),
            Point::new(16, 32),
            Point::new(1279, 799),
            Point::new(1263, 767),
            Point::new(640, 400),
        ];
        for p in samples {
            assert_eq!(
                snap_target_in_output(p, area(), area()),
                snap_target_in_output_with(p, area(), area(), EDGE_THRESHOLD),
            );
        }
        assert_eq!(CORNER_THRESHOLD, 32);
    }

    #[test]
    fn edge_threshold_widens_side_and_corner_zones() {
        let work = area();
        // Default 16: x=20 is outside the left edge zone.
        assert_eq!(
            snap_target_in_output_with(Point::new(20, 540), work, work, 16),
            SnapTarget::None
        );
        // 32 px: the same point is inside the left edge zone.
        assert_eq!(
            snap_target_in_output_with(Point::new(20, 540), work, work, 32),
            SnapTarget::LeftHalf
        );
        // Corner zone is 2 * edge: the top band is 32 px at the default, 64 px at 32.
        assert_eq!(
            snap_target_in_output_with(Point::new(10, 50), work, work, 16),
            SnapTarget::LeftHalf
        );
        assert_eq!(
            snap_target_in_output_with(Point::new(10, 50), work, work, 32),
            SnapTarget::TopLeftQuarter
        );
        assert_eq!(
            snap_target_in_output_with(Point::new(10, 50), work, work, 4),
            SnapTarget::None
        );
    }

    #[test]
    fn edge_threshold_is_clamped_to_at_least_one_pixel() {
        let work = area();
        assert_eq!(
            snap_target_in_output_with(Point::new(0, 540), work, work, 0),
            SnapTarget::LeftHalf
        );
        assert_eq!(
            snap_target_in_output_with(Point::new(0, 540), work, work, -50),
            SnapTarget::LeftHalf
        );
        assert_eq!(
            snap_target_in_output_with(Point::new(1, 540), work, work, 0),
            SnapTarget::None
        );
    }

    #[test]
    fn negative_origin_geometry_is_exact() {
        let work = Rect::new(-1920, 44, 1920, 1036);
        assert_eq!(
            snap_geometry(SnapTarget::RightHalf, work, Size::new(800, 600)),
            Rect::new(-960, 44, 960, 1036)
        );
    }

    #[test]
    fn odd_work_area_halves_match_committed_extent() {
        let work = Rect::new(0, 0, 1921, 1081);
        let current = Size::new(800, 600);
        assert_eq!(
            snap_geometry(SnapTarget::RightHalf, work, current),
            Rect::new(961, 0, 960, 1081)
        );
        assert_eq!(
            snap_geometry(SnapTarget::BottomHalf, work, current),
            Rect::new(0, 541, 1921, 540)
        );
        assert_eq!(
            snap_geometry(SnapTarget::BottomRightQuarter, work, current),
            Rect::new(961, 541, 960, 540)
        );
    }

    #[test]
    fn window_margin_insets_outer_edges_fully_and_splits_shared_edges() {
        let work = Rect::new(0, 44, 1920, 1036);
        let current = Size::new(800, 600);
        let left = apply_window_margin(snap_geometry(SnapTarget::LeftHalf, work, current), work, 2);
        let right =
            apply_window_margin(snap_geometry(SnapTarget::RightHalf, work, current), work, 2);
        assert_eq!(left, Rect::new(2, 46, 957, 1032));
        assert_eq!(right, Rect::new(961, 46, 957, 1032));
        assert_eq!(right.x - left.right(), 2);
        assert_eq!(
            apply_window_margin(work, work, 2),
            Rect::new(2, 46, 1916, 1032)
        );
    }

    #[test]
    fn window_margin_of_odd_width_keeps_the_exact_gap_between_tiles() {
        let work = Rect::new(0, 0, 1000, 800);
        let current = Size::new(800, 600);
        let top_left = apply_window_margin(
            snap_geometry(SnapTarget::TopLeftQuarter, work, current),
            work,
            5,
        );
        let bottom_right = apply_window_margin(
            snap_geometry(SnapTarget::BottomRightQuarter, work, current),
            work,
            5,
        );
        assert_eq!(bottom_right.x - top_left.right(), 5);
        assert_eq!(bottom_right.y - top_left.bottom(), 5);
        assert_eq!(top_left.x, 5);
        assert_eq!(work.right() - bottom_right.right(), 5);
    }

    #[test]
    fn zero_or_oversized_margin_keeps_the_tile() {
        let work = Rect::new(0, 0, 10, 10);
        assert_eq!(apply_window_margin(work, work, 0), work);
        assert_eq!(apply_window_margin(work, work, -3), work);
        assert_eq!(apply_window_margin(work, work, 8), work);
    }
}
