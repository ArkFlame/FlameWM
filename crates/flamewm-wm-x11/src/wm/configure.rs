//! Configure ownership: client-request policy vs notify observation.
//!
//! Pure policy only. No X calls, no x11rb. The live `wm` owner applies the
//! returned decisions against the server.
//!
//! Separation (frozen J06 stubs, kept verbatim):
//! - `handle_configure_notify` is observation-only: classify, compare, never
//!   commit geometry. Observation cannot construct geometry effects.
//! - `handle_configure_request` owns the accept/refuse policy: exactly one
//!   frame commit on accept; a synthetic current-geometry notify on refuse.
//! - Stack/sibling intent is orthogonal: carried alongside, never flips the
//!   geometry decision.

use crate::frame::coords::{RootPoint, RootRect};
use crate::frame::geometry::{FrameExtents, frame_to_client_root};
use crate::frame::model::PlacementMode;
use crate::frame::session::InteractionSession;

// ---- FROZEN INTERFACE (mirrors J06 `wm_configure_tests` stubs) ----

/// Observation class for a ConfigureNotify event window id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyClass {
    SyntheticEchoDrop,
    FrameObserved,
    ClientObserved,
    InputChildObserved,
    StaleUnknown,
}

/// Pure notify classifier: observation ids never imply geometry intent.
#[must_use]
pub fn classify_notify(
    window: u32,
    frame: u32,
    client: u32,
    is_input_child: bool,
    synthetic_echo: bool,
    known: bool,
) -> NotifyClass {
    if synthetic_echo {
        return NotifyClass::SyntheticEchoDrop;
    }
    if !known {
        return NotifyClass::StaleUnknown;
    }
    if window == frame {
        return NotifyClass::FrameObserved;
    }
    if window == client {
        return NotifyClass::ClientObserved;
    }
    if is_input_child {
        return NotifyClass::InputChildObserved;
    }
    NotifyClass::StaleUnknown
}

/// Request policy decision. Accept means exactly one frame commit;
/// refusals must emit a synthetic current-geometry notify instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestDecision {
    AcceptOnce,
    RefuseInteractive,
    RefuseMode,
}

impl RequestDecision {
    #[cfg(test)]
    #[must_use]
    pub const fn needs_synthetic_notify(self) -> bool {
        !matches!(self, Self::AcceptOnce)
    }
    #[cfg(test)]
    #[must_use]
    pub const fn geometry_effects(self) -> usize {
        match self {
            Self::AcceptOnce => 1,
            Self::RefuseInteractive | Self::RefuseMode => 0,
        }
    }
}

/// Pure request policy: active gesture wins over mode; WM-owned modes
/// refuse client geometry; only Idle+Floating accepts.
#[must_use]
pub fn decide_request(session: &InteractionSession, mode: PlacementMode) -> RequestDecision {
    if !session.is_idle() {
        return RequestDecision::RefuseInteractive;
    }
    if !matches!(mode, PlacementMode::Floating) {
        return RequestDecision::RefuseMode;
    }
    RequestDecision::AcceptOnce
}

// ---- Observation classification (stale/current/mismatch) ----

/// Outcome of comparing one notify observation against expectation.
/// Observation-only: carries no geometry effects by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observation {
    Current,
    Mismatch,
}

/// Classify a frame observation: root-coords outer rect equality.
#[must_use]
pub fn observe_frame(expected: RootRect, observed: RootRect) -> Observation {
    if observed == expected {
        Observation::Current
    } else {
        Observation::Mismatch
    }
}

/// Expected client-local geometry for an outer frame rect.
#[must_use]
pub fn expected_client_geometry(frame: RootRect, extents: FrameExtents) -> (i32, i32, i32, i32) {
    let (ox, oy) = extents.client_offset();
    let expected_w = frame.w.max(1);
    let expected_h = frame.h.saturating_sub(extents.delta().1).max(1);
    (ox, oy, expected_w, expected_h)
}

/// Classify a client observation: local origin + interior size equality.
#[must_use]
pub fn observe_client(
    frame: RootRect,
    extents: FrameExtents,
    observed: (i32, i32, i32, i32),
) -> Observation {
    if observed == expected_client_geometry(frame, extents) {
        Observation::Current
    } else {
        Observation::Mismatch
    }
}

// ---- Synthetic notify contract ----

/// Synthetic ConfigureNotify payload: current client-root geometry.
/// Refusal paths (and accepted commits) acknowledge exactly this; the
/// frame is never moved by a notify.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyntheticNotify {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// Derive the synthetic notify from the authoritative outer frame rect.
/// Single geometry source: `frame_to_client_root` only.
#[must_use]
pub fn synthetic_notify_for(
    outer_x: i32,
    outer_y: i32,
    outer_w: i32,
    outer_h: i32,
    extents: FrameExtents,
) -> SyntheticNotify {
    let root = frame_to_client_root(
        RootRect::new(outer_x, outer_y, outer_w.max(1), outer_h.max(1)),
        extents,
    );
    let (w, h) = root.size_u32();
    SyntheticNotify {
        x: root.x,
        y: root.y,
        w,
        h,
    }
}

// ---- Stack/sibling policy ----

/// Stack/sibling intent rides alongside the geometry decision and never
/// flips it. Returns the carried flag unchanged so callers restack on
/// both accept and refuse paths.
#[must_use]
pub const fn carry_stack_intent(
    decision: RequestDecision,
    has_stack: bool,
) -> (RequestDecision, bool) {
    (decision, has_stack)
}

/// Client-root origin for a ConfigureRequest mask, verbatim ICCCM 4.1.5:
/// request coords are already root coords; missing axes keep the previous
/// origin. No frame-offset subtraction applies.
#[must_use]
pub fn request_origin(
    has_x: bool,
    has_y: bool,
    req_x: i32,
    req_y: i32,
    prev: RootPoint,
) -> RootPoint {
    RootPoint::new(
        if has_x { req_x } else { prev.x },
        if has_y { req_y } else { prev.y },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::coords::{RootPoint, RootRect};
    use crate::frame::geometry::FrameExtents;
    use crate::frame::model::{ResizeEdges, SnapTarget};
    use crate::frame::session::{
        ControlPressSession, InteractionSession, MoveAnchor, MoveSession, ResizeSession,
    };

    fn extents() -> FrameExtents {
        FrameExtents::new(24, 1)
    }

    fn idle() -> InteractionSession {
        InteractionSession::Idle
    }

    // T04: 1000 expected-frame notifies are observation-only (zero effects).
    #[test]
    fn t04_frame_expected_notify_is_observation_only() {
        let expected = RootRect::new(100, 100, 400, 300);
        for i in 0..1000 {
            let class = classify_notify(11, 11, 12, false, false, true);
            assert_eq!(class, NotifyClass::FrameObserved, "iter {i}");
            assert_eq!(
                observe_frame(expected, RootRect::new(100, 100, 400, 300)),
                Observation::Current,
                "iter {i}"
            );
        }
        assert_eq!(expected, RootRect::new(100, 100, 400, 300));
    }

    // T04: 1000 expected-client notifies are observation-only (zero effects).
    #[test]
    fn t04_client_expected_notify_is_observation_only() {
        let frame = RootRect::new(100, 100, 400, 300);
        let expected = expected_client_geometry(frame, extents());
        for i in 0..1000 {
            let class = classify_notify(12, 11, 12, false, false, true);
            assert_eq!(class, NotifyClass::ClientObserved, "iter {i}");
            assert_eq!(
                observe_client(frame, extents(), expected),
                Observation::Current,
                "iter {i}"
            );
        }
    }

    // T04: 1000 stale notifies drop without lookup or mutation.
    #[test]
    fn t04_stale_notify_is_dropped() {
        for i in 0..1000 {
            let class = classify_notify(9000 + (i % 64) as u32, 11, 12, false, false, false);
            assert_eq!(class, NotifyClass::StaleUnknown, "iter {i}");
        }
    }

    // T04: 1000 input-child notifies never touch placement.
    #[test]
    fn t04_input_child_notify_is_observation_only() {
        for i in 0..1000 {
            let child = 100 + (i % 12) as u32;
            let class = classify_notify(child, 11, 12, true, false, true);
            assert_eq!(class, NotifyClass::InputChildObserved, "iter {i}");
        }
    }

    // T05: Idle+Floating accepts exactly once.
    #[test]
    fn t05_idle_floating_accepts_once() {
        let decision = decide_request(&idle(), PlacementMode::Floating);
        assert_eq!(decision, RequestDecision::AcceptOnce);
        assert_eq!(decision.geometry_effects(), 1);
        assert!(!decision.needs_synthetic_notify());
        assert_eq!(carry_stack_intent(decision, true), (decision, true));
        assert_eq!(carry_stack_intent(decision, false), (decision, false));
        let frame = RootRect::new(100, 100, 400, 300);
        assert_eq!(observe_frame(frame, frame), Observation::Current);
    }

    // T05: active gesture refuses; synthetic notify owed.
    #[test]
    fn t05_interactive_sessions_refuse_with_synthetic() {
        let mut session = InteractionSession::Idle;
        session.begin_move(MoveSession {
            client: 7,
            start_pointer: RootPoint::new(120, 130),
            start_placement: crate::frame::model::PlacementSnapshot {
                mode: PlacementMode::Floating,
                rect: RootRect::new(100, 100, 400, 300),
            },
            anchor: MoveAnchor::new(20, 400, 30),
            grab_window: 99,
        });
        let mut resize = InteractionSession::Idle;
        resize.begin_resize(ResizeSession {
            client: 7,
            start_pointer: RootPoint::new(100, 100),
            start_placement: crate::frame::model::PlacementSnapshot {
                mode: PlacementMode::Floating,
                rect: RootRect::new(100, 100, 400, 300),
            },
            edges: ResizeEdges::bottom_right(),
            grab_window: 99,
        });
        let mut control = InteractionSession::Idle;
        control.begin_control(ControlPressSession::new(
            7,
            crate::frame::model::FrameControl::Close,
            99,
        ));

        for (name, session) in [("move", session), ("resize", resize), ("control", control)] {
            let decision = decide_request(&session, PlacementMode::Floating);
            assert_eq!(decision, RequestDecision::RefuseInteractive, "{name}");
            assert_eq!(decision.geometry_effects(), 0, "{name}");
            assert!(decision.needs_synthetic_notify(), "{name}");
            assert_eq!(carry_stack_intent(decision, true), (decision, true));
            assert_eq!(carry_stack_intent(decision, false), (decision, false));
        }
    }

    // T05: WM-owned modes refuse even when idle; stack intent orthogonal.
    #[test]
    fn t05_owned_modes_refuse_stack_independent() {
        let modes = [
            PlacementMode::Maximized,
            PlacementMode::Fullscreen,
            PlacementMode::Snapped(SnapTarget::Left),
            PlacementMode::Snapped(SnapTarget::Right),
        ];
        for mode in modes {
            let decision = decide_request(&idle(), mode);
            assert_eq!(decision, RequestDecision::RefuseMode, "mode {mode:?}");
            assert!(decision.needs_synthetic_notify(), "mode {mode:?}");
            assert_eq!(decision.geometry_effects(), 0, "mode {mode:?}");
            assert_eq!(carry_stack_intent(decision, true), (decision, true));
            assert_eq!(carry_stack_intent(decision, false), (decision, false));
        }
    }

    // Synthetic contract: derives from the single geometry source.
    #[test]
    fn synthetic_notify_matches_client_root() {
        let ext = extents();
        let notify = synthetic_notify_for(100, 100, 400, 300, ext);
        let root = frame_to_client_root(RootRect::new(100, 100, 400, 300), ext);
        assert_eq!(notify.x, root.x);
        assert_eq!(notify.y, root.y);
        let (w, h) = root.size_u32();
        assert_eq!((notify.w, notify.h), (w, h));
    }
}
