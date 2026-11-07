//! Pure frame geometry planner: move / resize / configure (F08-F10).
//!
//! No X calls, no x11rb, no live-frame mutation. All planners take a
//! [`GeometryRequest`] value (snapshot + delta) and return a
//! [`GeometryPlan`]. Deltas are always relative to `start.rect` - never
//! incremental - so callers can replay from the drag/press snapshot.

mod bounds;
mod configure;
mod moves;
#[cfg(test)]
mod tests;

pub use bounds::*;
pub use configure::*;
pub use moves::*;

use super::coords::{ClientLocalRect, ClientRootRect, RootPoint, RootRect};
use super::model::{PlacementMode, PlacementSnapshot, ResizeEdges};
use crate::size_hints::{ClientSizeHints, WinGravity, snap_client_size};

/// Why a geometry plan was requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GeometryReason {
    InitialMap,
    InteractiveMove,
    InteractiveResize,
    ClientConfigure,
}

/// Frame chrome extents in pixels.
///
/// Old-good model (see `manage`/`configure_frame` in the pre-refactor wm):
/// the X frame interior width equals the client width plus the outline band,
/// the interior height equals titlebar + band + client height, the reparented
/// client sits at local `(inset, titlebar)`, and the X border is an outer frame
/// property only — it is never added to the local offset or size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameExtents {
    pub titlebar_h: i32,
    pub border: i32,
    /// Outline band painted into the frame's own outermost pixels. The client is placed this
    /// far inside the frame on the left, right and bottom edges, so its window cannot cover the
    /// ring no matter which shape the application installs afterwards. Zero when no ring is drawn
    /// or when a compositing-manager overlay composites the ring over the client's own pixels.
    pub inset: i32,
}

impl FrameExtents {
    #[must_use]
    pub const fn new(titlebar_h: i32, border: i32) -> Self {
        Self {
            titlebar_h,
            border,
            inset: 0,
        }
    }

    /// Same extents with an outline band of `inset` px reserved around the client.
    #[must_use]
    pub const fn with_inset(self, inset: i32) -> Self {
        Self { inset, ..self }
    }

    /// Extra frame pixels over the client size: `(dw, dh)`.
    ///
    /// The border never inflates the client area; the titlebar and the outline band do, so the
    /// application still gets exactly the size it asked for.
    #[must_use]
    pub const fn delta(self) -> (i32, i32) {
        (self.inset * 2, self.titlebar_h + self.inset)
    }

    /// Client origin inside the frame: reparented at `(inset, titlebar)`.
    #[must_use]
    pub const fn client_offset(self) -> (i32, i32) {
        (self.inset, self.titlebar_h)
    }
}

/// Pure input to every planner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeometryRequest<'a> {
    pub reason: GeometryReason,
    pub start: PlacementSnapshot,
    pub pointer_delta: (i32, i32),
    pub edges: ResizeEdges,
    pub work_area: RootRect,
    /// Output geometries; drags keep 1/4 of the window visible on these, not on the work area.
    /// Empty means no output info: `work_area` is used as the bound.
    pub outputs: &'a [RootRect],
    pub hints: ClientSizeHints,
    pub frame_extents: FrameExtents,
}

/// Pure planner output: frame rect plus derived client geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeometryPlan {
    pub frame: RootRect,
    /// Client origin offset inside the frame (0, titlebar).
    pub client_offset: (i32, i32),
    pub client_size: (u32, u32),
    /// True when the plan equals `start.rect` (no-op).
    pub noop: bool,
    pub keep_mode: PlacementMode,
}

impl GeometryPlan {
    fn of(reason_start: &GeometryRequest, frame: RootRect) -> Self {
        let (dw, dh) = reason_start.frame_extents.delta();
        let offset = reason_start.frame_extents.client_offset();
        let cw = frame.w.saturating_sub(dw).max(1) as u32;
        let ch = frame.h.saturating_sub(dh).max(1) as u32;
        let noop = frame == reason_start.start.rect;
        Self {
            frame,
            client_offset: offset,
            client_size: (cw, ch),
            noop,
            keep_mode: reason_start.start.mode,
        }
    }
}
