//! Frame controller: pure INPUT -> SESSION -> GEOMETRY -> effect plan reducer.
//!
//! No X calls, no x11rb, no live-frame mutation. Composes the `input`
//! registry view (source-XID-is-truth), the `session` state machine, and the
//! `geometry` planners into one changeset per event plus a native effect
//! list applied by the live owner.

use std::collections::HashMap;
use std::rc::Rc;

use flamewm_render_core::CursorKind;

use super::coords::{ClientRootRect, RootPoint, RootRect};
#[cfg(test)]
use super::geometry::plan_client_configure;
use super::geometry::{
    FrameExtents, GeometryReason, GeometryRequest, frame_to_client_local, frame_to_client_root,
    plan_move, plan_resize,
};
use super::input::target_for_xid;
use super::model::{
    FrameControl, FrameRegion, PlacementMode, PlacementSnapshot, PlacementState, ResizeEdges,
    SnapTarget,
};
use super::resources::cursor_for_region;
use super::session::{
    ControlPressSession, InteractionSession, MoveAnchor, MoveSession, ResizeSession,
    maximized_restore_rect,
};
use crate::size_hints::ClientSizeHints;

/// Native effect intents executed by the live owner. Descriptions only.
///
/// Domain split: `ConfigureClientLocal` carries client-local geometry
/// (origin (0, titlebar) + client size) for the reparented window;
/// `ConfigureFrameRoot` carries the frame outer rect in root coords
/// (resize/general paths only);
/// `MoveFrameRoot` carries a position-only frame origin in root coords
/// for move sessions (w/h unrepresentable by construction);
/// `NotifyClientRoot` carries the synthetic client-root rect.
mod planning;
mod reducer;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_effects;

use planning::*;
pub use reducer::{reduce, reduce_owned};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameEffect {
    Grab {
        window: u32,
        cursor: CursorKind,
    },
    Ungrab,
    ConfigureFrameRoot {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    },
    MoveFrameRoot {
        x: i32,
        y: i32,
    },
    ConfigureClientLocal {
        x: i32,
        y: i32,
        w: u32,
        h: u32,
    },
    LayoutInput,
    PaintChrome,
    ApplyShape,
    NotifyClientRoot {
        x: i32,
        y: i32,
        w: u32,
        h: u32,
    },
    SnapPreview {
        target: Option<SnapTarget>,
    },
    /// XP-style move/resize outline while `contents_while_dragging` is off. `None` hides it.
    DragOutline {
        rect: Option<RootRect>,
    },
    Noop,
}

/// Pure controller state: session + placement + registry view + geometry ctx.
#[derive(Debug, Clone)]
pub struct ControllerState {
    pub session: InteractionSession,
    pub placement: PlacementState,
    /// Shared with the geometry cache; cloning a controller only bumps the refcount.
    pub registry: Rc<HashMap<u32, FrameRegion>>,
    pub client: u32,
    pub grab: u32,
    pub work_area: RootRect,
    /// Output geometries bounding drags; empty falls back to `work_area`.
    pub outputs: Rc<Vec<RootRect>>,
    pub screen_rect: RootRect,
    pub hints: ClientSizeHints,
    pub extents: FrameExtents,
    /// Last title click (release without drag activation); a second one
    /// inside the double-click window toggles maximize.
    pub last_title_click: Option<TitleClick>,
    /// `false`: motion only moves an outline; the real frame/client change on release.
    pub contents_while_dragging: bool,
    pub window_margin: i32,
    /// Pointer and snap target of the last move motion applied; a repeat is skipped.
    pub last_motion: Option<(RootPoint, flamewm_window_core::SnapTarget)>,
}

/// Non-drag title release awaiting a possible second click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TitleClick {
    pub client: u32,
    pub time_ms: u32,
    pub pointer: RootPoint,
}

const TITLE_DOUBLE_CLICK_TIMEOUT_MS: u32 = 500;
const TITLE_DOUBLE_CLICK_SLOP_PX: i64 = 4;

impl ControllerState {
    #[must_use]
    pub fn new(
        client: u32,
        grab: u32,
        placement: PlacementState,
        work_area: RootRect,
        hints: ClientSizeHints,
        extents: FrameExtents,
    ) -> Self {
        Self {
            session: InteractionSession::Idle,
            placement,
            registry: Rc::new(HashMap::new()),
            client,
            grab,
            work_area,
            outputs: Rc::new(Vec::new()),
            screen_rect: work_area,
            hints,
            extents,
            last_title_click: None,
            contents_while_dragging: true,
            window_margin: 0,
            last_motion: None,
        }
    }

    #[must_use]
    pub fn with_window_margin(mut self, margin: i32) -> Self {
        self.window_margin = margin.max(0);
        self
    }

    fn layout_rect(&self, tile: RootRect) -> RootRect {
        margined_rect(tile, self.work_area, self.window_margin)
    }

    #[must_use]
    pub fn with_contents_while_dragging(mut self, enabled: bool) -> Self {
        self.contents_while_dragging = enabled;
        self
    }

    /// An XP outline move/resize is in progress: the committed frame stays put until release.
    #[must_use]
    pub fn outline_drag(&self) -> bool {
        !self.contents_while_dragging
            && match self.session {
                InteractionSession::Move(m) => m.anchor.is_activated(),
                InteractionSession::Resize(_) => true,
                InteractionSession::ControlPress(_) | InteractionSession::Idle => false,
            }
    }

    #[must_use]
    pub fn with_screen_rect(mut self, screen_rect: RootRect) -> Self {
        self.screen_rect = screen_rect;
        self
    }

    #[must_use]
    pub fn with_outputs(mut self, outputs: Rc<Vec<RootRect>>) -> Self {
        self.outputs = outputs;
        self
    }

    #[must_use]
    pub fn with_registry(mut self, registry: Rc<HashMap<u32, FrameRegion>>) -> Self {
        self.registry = registry;
        self
    }
}

/// One input event per reducer step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameEvent {
    Press {
        source: u32,
        pointer: RootPoint,
    },
    Motion {
        pointer: RootPoint,
    },
    /// `time_ms` is the X server event time (wrapping) for double-click timing.
    Release {
        pointer: RootPoint,
        time_ms: u32,
    },
    Enter,
    Leave,
    /// Client intent only: a `ConfigureRequest` from the client window.
    /// `root_origin`/`size` are client-window geometry (client-root rect),
    /// never the frame outer rect and never a `ConfigureNotify` observation.
    /// The live owner must never construct this from notify events (M01).
    #[cfg(test)]
    ClientConfigureRequest {
        root_origin: RootPoint,
        size: Option<(u32, u32)>,
    },
    Snap {
        target: SnapTarget,
    },
    ToggleMax,
    ToggleFullscreen,
    Cancel,
}
