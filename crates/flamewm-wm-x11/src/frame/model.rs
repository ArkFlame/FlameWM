//! Frame model: regions, controls, resize edges, placement state.
//!
//! Pure value objects. No X calls, no x11rb.

use super::coords::RootRect;

/// Which part of the frame a pointer hit belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameRegion {
    Client,
    TitleDrag,
    Control(FrameControl),
    Resize(ResizeEdges),
}

/// Title-bar control button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameControl {
    Minimize,
    MaximizeRestore,
    Close,
}

/// Bitflags-style resize edge set. Exactly one canonical constructor per
/// direction/corner combination (8 total).
///
/// Single owner for all resize-edge geometry: `wm`, layout, resources,
/// session, and geometry all consume this type. Hit thickness shared with
/// the frame child layout (`RESIZE_T == 6`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResizeEdges {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

impl Default for ResizeEdges {
    fn default() -> Self {
        Self::none()
    }
}

impl ResizeEdges {
    const fn of(left: bool, right: bool, top: bool, bottom: bool) -> Self {
        Self {
            left,
            right,
            top,
            bottom,
        }
    }

    #[must_use]
    pub const fn none() -> Self {
        Self::of(false, false, false, false)
    }

    #[must_use]
    pub const fn left() -> Self {
        Self::of(true, false, false, false)
    }

    #[must_use]
    pub const fn right() -> Self {
        Self::of(false, true, false, false)
    }

    #[must_use]
    pub const fn top() -> Self {
        Self::of(false, false, true, false)
    }

    #[must_use]
    pub const fn bottom() -> Self {
        Self::of(false, false, false, true)
    }

    #[must_use]
    pub const fn top_left() -> Self {
        Self::of(true, false, true, false)
    }

    #[must_use]
    pub const fn top_right() -> Self {
        Self::of(false, true, true, false)
    }

    #[must_use]
    pub const fn bottom_left() -> Self {
        Self::of(true, false, false, true)
    }

    #[must_use]
    pub const fn bottom_right() -> Self {
        Self::of(false, true, false, true)
    }

    #[cfg(test)]
    #[must_use]
    pub const fn is_empty(self) -> bool {
        !(self.left || self.right || self.top || self.bottom)
    }

    /// Nonempty alias kept for shared hit vocabulary.
    #[must_use]
    pub const fn is_corner(self) -> bool {
        (self.left as u8 + self.right as u8) == 1 && (self.top as u8 + self.bottom as u8) == 1
    }

    /// Edge hit-test for a frame-local point against an outer size.
    /// Resize drag geometry with a 160x96 minimum. Pure helper over the
    /// frame `outer` rect.
    #[cfg(test)]
    #[must_use]
    pub fn resized_rect(
        original: crate::geometry::Rect,
        edges: Self,
        dx: i32,
        dy: i32,
    ) -> crate::geometry::Rect {
        const MIN_WIDTH: i32 = 160;
        const MIN_HEIGHT: i32 = 96;
        let mut left = original.x;
        let mut top = original.y;
        let mut right = original.right();
        let mut bottom = original.bottom();
        if edges.left {
            left = left.saturating_add(dx).min(right.saturating_sub(MIN_WIDTH));
        }
        if edges.right {
            right = right.saturating_add(dx).max(left.saturating_add(MIN_WIDTH));
        }
        if edges.top {
            top = top
                .saturating_add(dy)
                .min(bottom.saturating_sub(MIN_HEIGHT));
        }
        if edges.bottom {
            bottom = bottom
                .saturating_add(dy)
                .max(top.saturating_add(MIN_HEIGHT));
        }
        crate::geometry::Rect::new(
            left,
            top,
            u32::try_from(right.saturating_sub(left)).unwrap_or(MIN_WIDTH as u32),
            u32::try_from(bottom.saturating_sub(top)).unwrap_or(MIN_HEIGHT as u32),
        )
    }

    /// Semantic resize cursor for an edge set.
    #[cfg(test)]
    #[must_use]
    pub fn cursor_kind(self) -> flamewm_render_core::CursorKind {
        use flamewm_render_core::CursorKind;
        match (self.left || self.right, self.top || self.bottom) {
            (true, true) => {
                if self.top == self.left {
                    CursorKind::ResizeNorthWestSouthEast
                } else {
                    CursorKind::ResizeNorthEastSouthWest
                }
            }
            (true, false) => CursorKind::ResizeHorizontal,
            (false, true) => CursorKind::ResizeVertical,
            (false, false) => CursorKind::Default,
        }
    }
}

/// Snap target for the snapped placement mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SnapTarget {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Window placement mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlacementMode {
    Floating,
    Snapped(SnapTarget),
    Maximized,
    Fullscreen,
}

/// Value-only snapshot of a placement (resume target).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlacementSnapshot {
    pub mode: PlacementMode,
    pub rect: RootRect,
}

/// Value-only placement state: current rect plus the floating restore rect
/// and an optional resume snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlacementState {
    pub mode: PlacementMode,
    pub current: RootRect,
    pub floating_restore: RootRect,
    pub resume: Option<PlacementSnapshot>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::coords::{FrameRect, RootPoint};

    #[test]
    fn resize_edges_helpers_and_corners() {
        assert!(ResizeEdges::none().is_empty());
        assert!(!ResizeEdges::left().is_empty());
        assert!(!ResizeEdges::left().is_corner());
        assert!(!ResizeEdges::top().is_corner());
        assert!(ResizeEdges::top_left().is_corner());
        assert!(ResizeEdges::top_right().is_corner());
        assert!(ResizeEdges::bottom_left().is_corner());
        assert!(ResizeEdges::bottom_right().is_corner());
        // Opposite pairs are not corners.
        let both_h = ResizeEdges {
            left: true,
            right: true,
            top: false,
            bottom: false,
        };
        assert!(!both_h.is_corner());
        assert!(!both_h.is_empty());
    }

    #[test]
    fn placement_transitions_are_value_only() {
        let floating = RootRect::new(500, 200, 400, 300);
        let maximized = RootRect::new(0, 0, 1920, 1080);
        let mut state = PlacementState {
            mode: PlacementMode::Floating,
            current: floating,
            floating_restore: floating,
            resume: None,
        };
        // Maximize: remember floating rect, move current.
        state.resume = Some(PlacementSnapshot {
            mode: state.mode,
            rect: state.current,
        });
        state.mode = PlacementMode::Maximized;
        state.current = maximized;
        assert_eq!(state.floating_restore, floating);
        assert_eq!(
            state.resume,
            Some(PlacementSnapshot {
                mode: PlacementMode::Floating,
                rect: floating,
            })
        );
        // Restore from resume snapshot.
        let snap = state.resume.expect("resume snapshot");
        state.mode = snap.mode;
        state.current = snap.rect;
        state.resume = None;
        assert_eq!(state.mode, PlacementMode::Floating);
        assert_eq!(state.current, floating);
        assert_eq!(state.resume, None);
    }

    #[test]
    fn snapped_and_fullscreen_modes_hold_rects() {
        let left_rect = RootRect::new(0, 0, 960, 1080);
        let state = PlacementState {
            mode: PlacementMode::Snapped(SnapTarget::Left),
            current: left_rect,
            floating_restore: RootRect::new(500, 200, 400, 300),
            resume: Some(PlacementSnapshot {
                mode: PlacementMode::Floating,
                rect: RootRect::new(500, 200, 400, 300),
            }),
        };
        assert_eq!(state.mode, PlacementMode::Snapped(SnapTarget::Left));
        assert_eq!(state.current, left_rect);
        let fs = PlacementState {
            mode: PlacementMode::Fullscreen,
            current: RootRect::new(0, 0, 1920, 1080),
            floating_restore: state.floating_restore,
            resume: state.resume,
        };
        assert_eq!(fs.mode, PlacementMode::Fullscreen);
        // Regions carry their payloads.
        let region = FrameRegion::Resize(ResizeEdges::bottom_right());
        assert_eq!(region, FrameRegion::Resize(ResizeEdges::bottom_right()));
        let control = FrameRegion::Control(FrameControl::Close);
        assert_eq!(control, FrameRegion::Control(FrameControl::Close));
        // Coords bridge check with nonzero origin (500,200).
        let origin = RootPoint::new(500, 200);
        let frame = FrameRect::new(0, 0, 400, 300);
        assert_eq!(frame.to_root(origin), state.floating_restore);
    }
}

/// X `border_width` of a frame window for its placement mode.
///
/// Only floating frames show the configured border. Snapped, maximized and
/// fullscreen frames are borderless so the border-inclusive X extent equals
/// the model rect (interior == outer == target). Pure: no X state.
#[must_use]
pub fn frame_border_for_mode(mode: PlacementMode, configured: u16) -> u16 {
    match mode {
        PlacementMode::Floating => configured,
        PlacementMode::Snapped(_) | PlacementMode::Maximized | PlacementMode::Fullscreen => 0,
    }
}

#[cfg(test)]
mod frame_border_tests {
    use super::*;

    #[test]
    fn floating_keeps_configured_border() {
        assert_eq!(frame_border_for_mode(PlacementMode::Floating, 1), 1);
        assert_eq!(frame_border_for_mode(PlacementMode::Floating, 2), 2);
    }

    #[test]
    fn snapped_maximized_fullscreen_are_borderless() {
        for target in [
            SnapTarget::Left,
            SnapTarget::Right,
            SnapTarget::Top,
            SnapTarget::Bottom,
            SnapTarget::TopLeft,
            SnapTarget::TopRight,
            SnapTarget::BottomLeft,
            SnapTarget::BottomRight,
        ] {
            assert_eq!(frame_border_for_mode(PlacementMode::Snapped(target), 1), 0);
        }
        assert_eq!(frame_border_for_mode(PlacementMode::Maximized, 1), 0);
        assert_eq!(frame_border_for_mode(PlacementMode::Fullscreen, 1), 0);
    }

    #[test]
    fn configured_zero_stays_zero() {
        assert_eq!(frame_border_for_mode(PlacementMode::Floating, 0), 0);
        assert_eq!(frame_border_for_mode(PlacementMode::Maximized, 0), 0);
    }
}
