//! Interaction session: pure F05/F07 state machine.
//!
//! No X calls, no x11rb. Tracks an in-progress move, resize, or
//! title-bar control press plus the maximized-restore anchor helper.

use super::coords::{RootPoint, RootRect};
use super::model::{FrameControl, PlacementSnapshot, ResizeEdges};

/// Anchor of the pointer within the frame when a move starts.
///
/// `frac_num / frac_den` is the horizontal position of the pointer across
/// the frame width, clamped to `0..=1`. `vertical_offset` is the pointer's
/// distance below the frame top.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MoveAnchor {
    pub frac_num: u32,
    pub frac_den: u32,
    pub vertical_offset: i32,
    activated: bool,
}

impl MoveAnchor {
    #[must_use]
    pub const fn new(frac_num: u32, frac_den: u32, vertical_offset: i32) -> Self {
        Self {
            frac_num,
            frac_den,
            vertical_offset,
            activated: false,
        }
    }

    pub(crate) fn activate(&mut self) {
        self.activated = true;
    }

    #[must_use]
    pub(crate) const fn is_activated(self) -> bool {
        self.activated
    }

    /// Horizontal anchor fraction clamped to `0..=1`.
    #[must_use]
    pub fn fraction(self) -> f32 {
        if self.frac_den == 0 {
            return 0.0;
        }
        let f = self.frac_num as f32 / self.frac_den as f32;
        f.clamp(0.0, 1.0)
    }
}

/// In-progress move session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MoveSession {
    pub client: u32,
    pub start_pointer: RootPoint,
    pub start_placement: PlacementSnapshot,
    pub anchor: MoveAnchor,
    pub grab_window: u32,
}

/// In-progress resize session. `edges` are locked at begin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResizeSession {
    pub client: u32,
    pub start_pointer: RootPoint,
    pub start_placement: PlacementSnapshot,
    pub edges: ResizeEdges,
    pub grab_window: u32,
}

/// In-progress title-bar control press. `armed` tracks enter/leave.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ControlPressSession {
    pub client: u32,
    pub control: FrameControl,
    pub grab_window: u32,
    pub armed: bool,
}

impl ControlPressSession {
    #[must_use]
    pub const fn new(client: u32, control: FrameControl, grab_window: u32) -> Self {
        Self {
            client,
            control,
            grab_window,
            armed: true,
        }
    }

    pub fn enter(&mut self) {
        self.armed = true;
    }

    pub fn leave(&mut self) {
        self.armed = false;
    }
}

/// Interaction session state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum InteractionSession {
    #[default]
    Idle,
    ControlPress(ControlPressSession),
    Move(MoveSession),
    Resize(ResizeSession),
}

impl InteractionSession {
    #[must_use]
    pub const fn is_idle(self) -> bool {
        matches!(self, Self::Idle)
    }

    pub fn begin_move(&mut self, session: MoveSession) {
        *self = Self::Move(session);
    }

    pub fn begin_resize(&mut self, session: ResizeSession) {
        *self = Self::Resize(session);
    }

    pub fn begin_control(&mut self, session: ControlPressSession) {
        *self = Self::ControlPress(session);
    }

    /// Pointer motion delta from the session start. Returns `None` when idle.
    /// Pure: reports the delta only, applies no geometry.
    #[cfg(test)]
    #[must_use]
    pub fn update_motion(self, current: RootPoint) -> Option<RootPoint> {
        let start = match self {
            Self::Idle => return None,
            Self::Move(m) => m.start_pointer,
            Self::Resize(r) => r.start_pointer,
            Self::ControlPress(_) => return None,
        };
        Some(RootPoint::new(
            current.x.saturating_sub(start.x),
            current.y.saturating_sub(start.y),
        ))
    }

    /// Pointer re-entered the pressed control: arm it. No-op unless control.
    pub fn control_enter(&mut self) {
        if let Self::ControlPress(c) = self {
            c.enter();
        }
    }

    /// Pointer left the pressed control: disarm it. No-op unless control.
    pub fn control_leave(&mut self) {
        if let Self::ControlPress(c) = self {
            c.leave();
        }
    }

    /// Release: returns the final point when a session was active and
    /// returns to `Idle`. Returns `None` when already idle.
    #[cfg(test)]
    pub fn finish(&mut self, point: RootPoint) -> Option<RootPoint> {
        let active = !self.is_idle();
        *self = Self::Idle;
        active.then_some(point)
    }

    /// Abort the session, discarding state.
    pub fn cancel(&mut self) {
        *self = Self::Idle;
    }
}

/// Position the floating restore rect for a maximized-window drag so the
/// pointer keeps its relative anchor (no jump).
///
/// `frac_num / frac_den` is the clamped horizontal anchor fraction and
/// `vert_off` the pointer distance below the frame top, clamped into
/// `0..=titlebar_h`. Width/height come from `restore` unchanged.
#[must_use]
pub fn maximized_restore_rect(
    restore: RootRect,
    pointer: RootPoint,
    frac_num: u32,
    frac_den: u32,
    vert_off: i32,
    titlebar_h: i32,
) -> RootRect {
    let frac = MoveAnchor::new(frac_num, frac_den, 0).fraction();
    let titlebar_h = titlebar_h.max(0);
    let vert = vert_off.clamp(0, titlebar_h);
    let dx = (frac * restore.w as f32).round() as i32;
    RootRect::new(
        pointer.x.saturating_sub(dx),
        pointer.y.saturating_sub(vert),
        restore.w,
        restore.h,
    )
}

#[cfg(test)]
mod tests {
    use super::super::model::{PlacementMode, PlacementSnapshot};
    use super::*;

    fn snap() -> PlacementSnapshot {
        PlacementSnapshot {
            mode: PlacementMode::Floating,
            rect: RootRect::new(500, 200, 400, 300),
        }
    }

    fn move_session() -> MoveSession {
        MoveSession {
            client: 7,
            start_pointer: RootPoint::new(520, 230),
            start_placement: snap(),
            anchor: MoveAnchor::new(20, 400, 30),
            grab_window: 99,
        }
    }

    #[test]
    fn move_delta_is_absolute_from_start() {
        let mut s = InteractionSession::Idle;
        s.begin_move(move_session());
        assert_eq!(
            s.update_motion(RootPoint::new(520, 230)),
            Some(RootPoint::new(0, 0))
        );
        assert_eq!(
            s.update_motion(RootPoint::new(560, 250)),
            Some(RootPoint::new(40, 20))
        );
        // Repeated motion stays relative to start, not incremental.
        assert_eq!(
            s.update_motion(RootPoint::new(530, 235)),
            Some(RootPoint::new(10, 5))
        );
    }

    #[test]
    fn resize_edge_lock_preserved() {
        let edges = ResizeEdges::top_left();
        let mut s = InteractionSession::Idle;
        s.begin_resize(ResizeSession {
            client: 7,
            start_pointer: RootPoint::new(500, 200),
            start_placement: snap(),
            edges,
            grab_window: 99,
        });
        assert!(matches!(
            s,
            InteractionSession::Resize(r) if r.edges == ResizeEdges::top_left()
        ));
        // Motion reports delta; locked edges unchanged.
        assert_eq!(
            s.update_motion(RootPoint::new(490, 190)),
            Some(RootPoint::new(-10, -10))
        );
        assert!(matches!(
            s,
            InteractionSession::Resize(r) if r.edges == ResizeEdges::top_left()
        ));
    }

    #[test]
    fn control_arm_disarm() {
        let mut s = InteractionSession::Idle;
        s.begin_control(ControlPressSession::new(7, FrameControl::Close, 99));
        assert!(matches!(
            s,
            InteractionSession::ControlPress(c) if c.armed
        ));
        s.control_leave();
        assert!(matches!(
            s,
            InteractionSession::ControlPress(c) if !c.armed
        ));
        s.control_enter();
        assert!(matches!(
            s,
            InteractionSession::ControlPress(c) if c.armed
        ));
        // Motion is N/A for control presses.
        assert_eq!(s.update_motion(RootPoint::new(0, 0)), None);
    }

    #[test]
    fn final_release_applies_last_point() {
        let mut s = InteractionSession::Idle;
        s.begin_move(move_session());
        let last = RootPoint::new(600, 400);
        assert_eq!(s.finish(last), Some(last));
        assert_eq!(s, InteractionSession::Idle);
        // Finishing while idle yields nothing.
        assert_eq!(s.finish(last), None);
    }

    #[test]
    fn cancel_returns_to_idle() {
        let mut s = InteractionSession::Idle;
        s.begin_resize(ResizeSession {
            client: 7,
            start_pointer: RootPoint::new(500, 200),
            start_placement: snap(),
            edges: ResizeEdges::bottom_right(),
            grab_window: 99,
        });
        s.cancel();
        assert_eq!(s, InteractionSession::Idle);
        assert_eq!(s.update_motion(RootPoint::new(510, 210)), None);
    }

    #[test]
    fn max_restore_anchor_no_jump() {
        let restore = RootRect::new(0, 0, 400, 300);
        let pointer = RootPoint::new(1000, 10);
        // Pointer grabbed at 1/4 across the maximized width.
        let placed = maximized_restore_rect(restore, pointer, 1, 4, 10, 24);
        assert_eq!(placed.w, 400);
        assert_eq!(placed.h, 300);
        assert_eq!(placed.x, 1000 - 100);
        assert_eq!(placed.y, 0);
        // Anchor fraction preserved: pointer sits at frac across placed rect.
        let frac = MoveAnchor::new(1, 4, 10).fraction();
        let got = (pointer.x - placed.x) as f32 / placed.w as f32;
        assert!((got - frac).abs() < 1e-6);
        assert_eq!(pointer.y - placed.y, 10);
        // Clamps: over-range fraction and vertical offset.
        let clamped = maximized_restore_rect(restore, pointer, 9, 4, 99, 24);
        assert_eq!(clamped.x, 1000 - 400);
        assert_eq!(clamped.y, pointer.y - 24);
        // Zero denominator degrades to left edge anchor.
        let zero = maximized_restore_rect(restore, pointer, 1, 0, 5, 24);
        assert_eq!(zero.x, pointer.x);
    }
}
