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

impl SnapTarget {
    #[must_use]
    pub const fn is_snapped(self) -> bool {
        !matches!(self, Self::None)
    }
}

#[must_use]
pub fn snap_target(pointer: Point, work_area: Rect) -> SnapTarget {
    let left = pointer.x >= work_area.x && pointer.x < work_area.x + EDGE_THRESHOLD;
    let right = pointer.x >= work_area.right() - EDGE_THRESHOLD && pointer.x < work_area.right();
    let top = pointer.y >= work_area.y && pointer.y < work_area.y + CORNER_THRESHOLD;
    let bottom =
        pointer.y >= work_area.bottom() - CORNER_THRESHOLD && pointer.y < work_area.bottom();
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
    let right_x = work_area.x.saturating_add(work_area.width.saturating_sub(half_w));
    let bottom_y = work_area.y.saturating_add(work_area.height.saturating_sub(half_h));
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowGeometryState {
    pub outer: Rect,
    pub restore: Rect,
    pub snap: SnapTarget,
    pub maximized: bool,
}

impl WindowGeometryState {
    #[must_use]
    pub const fn new(outer: Rect) -> Self {
        Self {
            outer,
            restore: outer,
            snap: SnapTarget::None,
            maximized: false,
        }
    }

    /// Capture floating geometry only once; changing snap targets must not destroy it.
    pub fn snap_to(&mut self, target: SnapTarget, work_area: Rect) {
        if !target.is_snapped() {
            return;
        }
        if !self.snap.is_snapped() && !self.maximized {
            self.restore = self.outer;
        }
        self.outer = snap_geometry(target, work_area, self.outer.size());
        self.snap = target;
        self.maximized = target == SnapTarget::Maximize;
    }

    pub fn resize_to(&mut self, outer: Rect) {
        self.outer = outer;
        self.restore = outer;
        self.snap = SnapTarget::None;
        self.maximized = false;
    }

    /// Restore a maximized window while keeping the saved floating rectangle recoverable.
    #[must_use]
    pub fn restore_maximized(&mut self, work_area: Rect) -> Rect {
        let restored = self.restore.clamp_inside(work_area);
        self.outer = restored;
        self.snap = SnapTarget::None;
        self.maximized = false;
        restored
    }

    /// Leave a snapped window under the pointer. The pointer keeps the same relative position
    /// in the restored rectangle, rather than jumping to its old top-left corner.
    #[must_use]
    pub fn drag_away(&mut self, pointer: Point, work_area: Rect) -> Rect {
        let snapped = self.outer;
        let restored = self.restore.clamp_inside(work_area);
        let next = Rect::new(
            pointer.x - ratio_offset(pointer.x - snapped.x, snapped.width, restored.width),
            pointer.y - ratio_offset(pointer.y - snapped.y, snapped.height, restored.height),
            restored.width,
            restored.height,
        )
        .clamp_inside(work_area);
        self.outer = next;
        self.restore = next;
        self.snap = SnapTarget::None;
        self.maximized = false;
        next
    }
}

fn ratio_offset(position: i32, source: i32, destination: i32) -> i32 {
    if source <= 0 {
        return 0;
    }
    ((i64::from(position) * i64::from(destination)) / i64::from(source)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rect {
        Rect::new(0, 0, 1920, 1080)
    }

    #[test]
    fn drag_away_preserves_pointer_ratio() {
        let mut state = WindowGeometryState::new(Rect::new(100, 100, 900, 600));
        state.snap_to(SnapTarget::LeftHalf, area());
        let restored = state.drag_away(Point::new(300, 540), area());
        assert_eq!(restored, Rect::new(19, 240, 900, 600));
    }

    #[test]
    fn first_snap_restore_survives_snap_to_snap() {
        let mut state = WindowGeometryState::new(Rect::new(100, 100, 900, 600));
        state.snap_to(SnapTarget::LeftHalf, area());
        state.snap_to(SnapTarget::RightHalf, area());
        assert_eq!(state.restore, Rect::new(100, 100, 900, 600));
    }

    #[test]
    fn resize_exits_snap() {
        let mut state = WindowGeometryState::new(Rect::new(100, 100, 900, 600));
        state.snap_to(SnapTarget::LeftHalf, area());
        state.resize_to(Rect::new(120, 140, 800, 500));
        assert_eq!(state.snap, SnapTarget::None);
    }

    #[test]
    fn maximize_restore_is_clamped() {
        let mut state = WindowGeometryState::new(Rect::new(-100, 100, 900, 600));
        state.snap_to(SnapTarget::Maximize, area());
        assert_eq!(state.restore_maximized(area()), Rect::new(0, 100, 900, 600));
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
    fn titlebar_pointer_remains_recoverable() {
        let mut state = WindowGeometryState::new(Rect::new(100, 100, 900, 600));
        state.snap_to(SnapTarget::TopLeftQuarter, area());
        let restored = state.drag_away(Point::new(80, 20), area());
        assert!(restored.contains(Point::new(80, 20)));
    }
}
