//! Lifecycle transaction: frame/client/input-child creation order.
//!
//! Pure value objects only. No X calls, no x11rb. The live `wm` owner (J19
//! wiring) applies the described effects in order: create frame, save-set +
//! reparent client, configure client interior, create + configure 12 input
//! children, register resources.
//!
//! Single geometry source: [`FrameExtents`] + [`frame_to_client_local`].
//! No second geometry state is stored here.

use crate::frame::coords::{ClientLocalRect, RootRect};
use crate::frame::geometry::{FrameExtents, frame_to_client_local};
use crate::frame::layout::{CONTROLS_WIDTH, RESIZE_T, layout_frame_children};
use crate::frame::resources::FrameResources;

/// Twelve input-child XIDs in `manage` order: 8 resize, title-drag, 3 controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LifecycleIds {
    pub client: u32,
    pub frame: u32,
    pub child_ids: [u32; 12],
}

impl LifecycleIds {
    #[must_use]
    pub const fn new(client: u32, frame: u32, child_ids: [u32; 12]) -> Self {
        Self {
            client,
            frame,
            child_ids,
        }
    }

    /// Resource value for `FrameRegistry::register`.
    #[must_use]
    pub const fn resources(self) -> FrameResources {
        FrameResources::new(
            self.client,
            self.frame,
            self.child_ids[8],
            [self.child_ids[9], self.child_ids[10], self.child_ids[11]],
            [
                self.child_ids[0],
                self.child_ids[1],
                self.child_ids[2],
                self.child_ids[3],
                self.child_ids[4],
                self.child_ids[5],
                self.child_ids[6],
                self.child_ids[7],
            ],
        )
    }
}

/// One input-child configure: frame-local rect for a child XID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChildPlacement {
    pub xid: u32,
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// Client interior contract: local origin `(0, titlebar)`, size `(frame.w,
/// frame.h - titlebar)`. Derived from [`frame_to_client_local`] only.
#[must_use]
pub fn client_interior(frame: RootRect, extents: FrameExtents) -> ClientLocalRect {
    frame_to_client_local(frame, extents)
}

/// Reparent offset of the client inside the frame: `(0, titlebar)`.
#[must_use]
pub const fn reparent_offset(extents: FrameExtents) -> (i32, i32) {
    (0, extents.titlebar_h)
}

/// Client configure payload `(x, y, w, h)` for the reparented client window.
#[must_use]
pub fn client_configure(frame: RootRect, extents: FrameExtents) -> (i32, i32, u32, u32) {
    let local = client_interior(frame, extents);
    (
        local.x,
        local.y,
        local.w.max(1) as u32,
        local.h.max(1) as u32,
    )
}

/// Twelve input-child placements in `children()` order (8 resize +
/// title-drag + 3 controls), ready for the J19 create/configure loop.
#[must_use]
pub fn input_child_placements(
    frame_w: i32,
    frame_h: i32,
    titlebar_h: i32,
    ids: LifecycleIds,
) -> [ChildPlacement; 12] {
    let layout = layout_frame_children(
        frame_w.max(1),
        frame_h.max(1),
        titlebar_h,
        CONTROLS_WIDTH,
        RESIZE_T,
    );
    let mut rects = layout.resize_rects().to_vec();
    rects.push(layout.title_drag);
    rects.extend(layout.controls);
    let xids = ids.children_ordered();
    let mut out = [ChildPlacement {
        xid: 0,
        x: 0,
        y: 0,
        w: 1,
        h: 1,
    }; 12];
    let mut i = 0;
    while i < 12 {
        let rect = rects[i];
        out[i] = ChildPlacement {
            xid: xids[i],
            x: rect.x,
            y: rect.y,
            w: rect.w.max(1) as u32,
            h: rect.h.max(1) as u32,
        };
        i += 1;
    }
    out
}

impl LifecycleIds {
    fn children_ordered(self) -> [u32; 12] {
        let res = self.resources();
        res.children()
    }
}
