use flamewm_api::{Point, Rect, Size};

pub const EDGE_THRESHOLD: i32 = 16;
pub const CORNER_THRESHOLD: i32 = 32;
pub const SIDE_DWELL_MS: u64 = 400;

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
    Center,
    Maximize,
}

impl SnapTarget {
    #[must_use]
    pub const fn is_side_half(self) -> bool {
        matches!(self, Self::LeftHalf | Self::RightHalf)
    }
}

/// Pointer-to-target mapping for active window drags.
///
/// Corners take precedence over side-edge workspace dwell. Bottom-center has no snap target.
#[must_use]
pub fn detect_target(pointer: Point, output: Rect) -> SnapTarget {
    let left = pointer.x >= output.x && pointer.x < output.x + EDGE_THRESHOLD;
    let right = pointer.x >= output.right() - EDGE_THRESHOLD && pointer.x < output.right();
    let top = pointer.y >= output.y && pointer.y < output.y + CORNER_THRESHOLD;
    let bottom = pointer.y >= output.bottom() - CORNER_THRESHOLD && pointer.y < output.bottom();
    let middle_x = !left && !right;

    match (left, right, top, bottom, middle_x) {
        (true, _, true, _, _) => SnapTarget::TopLeftQuarter,
        (_, true, true, _, _) => SnapTarget::TopRightQuarter,
        (true, _, _, true, _) => SnapTarget::BottomLeftQuarter,
        (_, true, _, true, _) => SnapTarget::BottomRightQuarter,
        (_, _, true, _, true) => SnapTarget::Maximize,
        (_, _, _, true, true) => SnapTarget::None,
        (true, _, _, _, _) => SnapTarget::LeftHalf,
        (_, true, _, _, _) => SnapTarget::RightHalf,
        _ => SnapTarget::None,
    }
}

/// Single geometry authority for both preview and commit.
#[must_use]
pub fn geometry_for(target: SnapTarget, work_area: Rect, current: Size) -> Rect {
    let width = work_area.width;
    let height = work_area.height;
    let left_width = width / 2;
    let right_width = width - left_width;
    let top_height = height / 2;
    let bottom_height = height - top_height;

    match target {
        SnapTarget::LeftHalf => Rect::new(work_area.x, work_area.y, left_width, height),
        SnapTarget::RightHalf => {
            Rect::new(work_area.x + left_width, work_area.y, right_width, height)
        }
        SnapTarget::TopHalf => Rect::new(work_area.x, work_area.y, width, top_height),
        SnapTarget::BottomHalf => {
            Rect::new(work_area.x, work_area.y + top_height, width, bottom_height)
        }
        SnapTarget::TopLeftQuarter => Rect::new(work_area.x, work_area.y, left_width, top_height),
        SnapTarget::TopRightQuarter => Rect::new(
            work_area.x + left_width,
            work_area.y,
            right_width,
            top_height,
        ),
        SnapTarget::BottomLeftQuarter => Rect::new(
            work_area.x,
            work_area.y + top_height,
            left_width,
            bottom_height,
        ),
        SnapTarget::BottomRightQuarter => Rect::new(
            work_area.x + left_width,
            work_area.y + top_height,
            right_width,
            bottom_height,
        ),
        SnapTarget::Center => {
            let target_width = if current.width > 0 {
                current.width
            } else {
                left_width
            };
            let target_height = if current.height > 0 {
                current.height
            } else {
                top_height
            };
            Rect::new(
                work_area.x + (width - target_width) / 2,
                work_area.y + (height - target_height) / 2,
                target_width,
                target_height,
            )
        }
        SnapTarget::Maximize => work_area,
        SnapTarget::None => Rect::new(work_area.x, work_area.y, current.width, current.height),
    }
}

#[must_use]
pub fn should_dwell_side_edge(
    target: SnapTarget,
    pointer: Point,
    output: Rect,
    elapsed_ms: u64,
) -> bool {
    if elapsed_ms < SIDE_DWELL_MS || !target.is_side_half() {
        return false;
    }
    match target {
        SnapTarget::LeftHalf => pointer.x >= output.x && pointer.x < output.x + EDGE_THRESHOLD,
        SnapTarget::RightHalf => {
            pointer.x >= output.right() - EDGE_THRESHOLD && pointer.x < output.right()
        }
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceEdgeDirection {
    Previous,
    Next,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DragSnapUpdate {
    pub preview: SnapTarget,
    pub workspace_request: Option<WorkspaceEdgeDirection>,
}

/// Active-drag snap/dwell session. Dwell expiry never destroys a valid snap candidate by itself.
/// The candidate resets only after an actual workspace transition is committed or the pointer
/// leaves the target. This directly protects the V8 side-edge release invariant.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DragSnapSession {
    preview: SnapTarget,
    side_entered_ms: Option<u64>,
    workspace_request_sent: bool,
}

impl DragSnapSession {
    #[must_use]
    pub const fn preview(&self) -> SnapTarget {
        self.preview
    }

    #[must_use]
    pub fn update(&mut self, pointer: Point, output: Rect, now_ms: u64) -> DragSnapUpdate {
        let target = detect_target(pointer, output);
        if target != self.preview {
            self.preview = target;
            self.side_entered_ms = target.is_side_half().then_some(now_ms);
            self.workspace_request_sent = false;
        }

        let mut workspace_request = None;
        if target.is_side_half() && !self.workspace_request_sent {
            if let Some(entered) = self.side_entered_ms {
                if now_ms.saturating_sub(entered) >= SIDE_DWELL_MS {
                    workspace_request = match target {
                        SnapTarget::LeftHalf => Some(WorkspaceEdgeDirection::Previous),
                        SnapTarget::RightHalf => Some(WorkspaceEdgeDirection::Next),
                        _ => None,
                    };
                    self.workspace_request_sent = workspace_request.is_some();
                }
            }
        }

        DragSnapUpdate {
            preview: self.preview,
            workspace_request,
        }
    }

    /// Call only after the workspace manager actually committed the requested transition.
    pub fn workspace_switch_committed(&mut self) {
        self.preview = SnapTarget::None;
        self.side_entered_ms = None;
        self.workspace_request_sent = false;
    }

    pub fn cancel(&mut self) {
        self.workspace_switch_committed();
    }

    #[must_use]
    pub const fn release_target(&self) -> SnapTarget {
        self.preview
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SnapState {
    pub current: SnapTarget,
    pub unsnapped_outer: Option<Rect>,
}

impl SnapState {
    pub fn snap(&mut self, target: SnapTarget, current_outer: Rect) {
        if target == SnapTarget::None {
            return;
        }
        if self.current == SnapTarget::None && self.unsnapped_outer.is_none() {
            self.unsnapped_outer = Some(current_outer);
        }
        self.current = target;
    }

    pub fn manual_resize(&mut self) {
        self.current = SnapTarget::None;
        self.unsnapped_outer = None;
    }

    #[must_use]
    pub fn consume_drag_away(&mut self, surviving_work_area: Rect) -> Option<Rect> {
        let restore = self
            .unsnapped_outer
            .map(|rect| rect.clamp_inside(surviving_work_area));
        self.manual_resize();
        restore
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_and_commit_geometry_share_negative_origin_math() {
        let work = Rect::new(-1920, 44, 1920, 1036);
        let right = geometry_for(SnapTarget::RightHalf, work, Size::new(800, 600));
        assert_eq!(right, Rect::new(-960, 44, 960, 1036));
    }

    #[test]
    fn corners_win_over_side_dwell() {
        let output = Rect::new(0, 0, 1920, 1080);
        let pointer = Point::new(0, 0);
        let target = detect_target(pointer, output);
        assert_eq!(target, SnapTarget::TopLeftQuarter);
        assert!(!should_dwell_side_edge(target, pointer, output, 1000));
    }

    #[test]
    fn first_snap_preserves_original_floating_geometry_across_snap_to_snap() {
        let mut state = SnapState::default();
        let floating = Rect::new(100, 100, 900, 600);
        state.snap(SnapTarget::LeftHalf, floating);
        state.snap(SnapTarget::RightHalf, Rect::new(0, 0, 960, 1080));
        assert_eq!(state.unsnapped_outer, Some(floating));
    }

    #[test]
    fn manual_resize_exits_snap_state() {
        let mut state = SnapState::default();
        state.snap(SnapTarget::LeftHalf, Rect::new(100, 100, 900, 600));
        state.manual_resize();
        assert_eq!(state, SnapState::default());
    }

    #[test]
    fn dwell_expiry_without_committed_workspace_switch_keeps_release_snap_candidate() {
        let output = Rect::new(0, 0, 1920, 1080);
        let pointer = Point::new(0, 500);
        let mut session = DragSnapSession::default();
        assert_eq!(
            session.update(pointer, output, 100).preview,
            SnapTarget::LeftHalf
        );
        let update = session.update(pointer, output, 500);
        assert_eq!(
            update.workspace_request,
            Some(WorkspaceEdgeDirection::Previous)
        );
        assert_eq!(session.release_target(), SnapTarget::LeftHalf);
    }

    #[test]
    fn actual_workspace_switch_commit_clears_old_snap_candidate() {
        let output = Rect::new(0, 0, 1920, 1080);
        let pointer = Point::new(1919, 500);
        let mut session = DragSnapSession::default();
        let _ = session.update(pointer, output, 0);
        let _ = session.update(pointer, output, 400);
        session.workspace_switch_committed();
        assert_eq!(session.release_target(), SnapTarget::None);
    }

    #[test]
    fn drag_away_clamps_restore_to_surviving_output() {
        let mut state = SnapState::default();
        state.snap(SnapTarget::LeftHalf, Rect::new(2600, 100, 1200, 800));
        let restore = state
            .consume_drag_away(Rect::new(0, 0, 1920, 1080))
            .expect("saved floating geometry");
        assert_eq!(restore, Rect::new(720, 100, 1200, 800));
        assert_eq!(state, SnapState::default());
    }
}
