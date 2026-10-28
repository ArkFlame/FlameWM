//! Pure window geometry policy.
//!
//! This crate owns no server, display, or renderer state.  Backends feed it the current
//! geometry and apply the returned geometry on their owning thread.

use flamewm_api::{Point, Rect, Size};

pub const EDGE_THRESHOLD: i32 = 16;
pub const CORNER_THRESHOLD: i32 = 32;

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

/// Snap detection for a pointer on an output whose work area may be reserved by docks.
/// Left/top zones are measured from the work area; right/bottom zones extend to the
/// output edge so a pointer over a bottom/right dock band still reaches the physical corner.
#[must_use]
pub fn snap_target_in_output(pointer: Point, output: Rect, work_area: Rect) -> SnapTarget {
    let left = pointer.x >= work_area.x && pointer.x < work_area.x + EDGE_THRESHOLD;
    let right = pointer.x >= work_area.right() - EDGE_THRESHOLD
        && pointer.x < work_area.right().max(output.right());
    let top = pointer.y >= work_area.y && pointer.y < work_area.y + CORNER_THRESHOLD;
    let bottom = pointer.y >= work_area.bottom() - CORNER_THRESHOLD
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
        let left = apply_window_margin(
            snap_geometry(SnapTarget::LeftHalf, work, current),
            work,
            2,
        );
        let right = apply_window_margin(
            snap_geometry(SnapTarget::RightHalf, work, current),
            work,
            2,
        );
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
