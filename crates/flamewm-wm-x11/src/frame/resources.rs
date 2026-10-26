//! Frame resource registry: value/lifecycle owner for per-client frame XIDs.
//!
//! Pure value objects. No X calls, no x11rb. The live `wm` owner applies the
//! described effects (create/destroy windows, select masks, define cursors);
//! this module only declares which XIDs exist, which masks/cursors each child
//! needs, and owns the client<->xid mappings with orphan-free removal.

use std::collections::HashMap;

use super::model::{FrameControl, FrameRegion, ResizeEdges};
use flamewm_render_core::CursorKind;

/// X11 protocol event-mask bits (effect description only; applied by `wm`).
/// Values mirror the X protocol so the owner can select them verbatim.
pub const MASK_BUTTON_PRESS: u32 = 0x4;
pub const MASK_BUTTON_RELEASE: u32 = 0x8;
pub const MASK_ENTER_WINDOW: u32 = 0x10;
pub const MASK_LEAVE_WINDOW: u32 = 0x20;
pub const MASK_POINTER_MOTION: u32 = 0x40;
pub const MASK_EXPOSURE: u32 = 0x8000;

/// Combined native mask for every frame input child. Includes `LEAVE_WINDOW`
/// so hover/highlight state clears when the pointer exits a child.
pub const INPUT_CHILD_EVENT_MASK: u32 = MASK_BUTTON_PRESS
    | MASK_BUTTON_RELEASE
    | MASK_POINTER_MOTION
    | MASK_ENTER_WINDOW
    | MASK_LEAVE_WINDOW
    | MASK_EXPOSURE;

/// Resize children in corner-first order (matches `layout::resize_pairs`).
pub const RESIZE_ORDER: [ResizeEdges; 8] = [
    ResizeEdges::top_left(),
    ResizeEdges::top_right(),
    ResizeEdges::bottom_left(),
    ResizeEdges::bottom_right(),
    ResizeEdges::top(),
    ResizeEdges::bottom(),
    ResizeEdges::left(),
    ResizeEdges::right(),
];

/// Control children left-to-right: minimize, maximize/restore, close.
pub const CONTROL_ORDER: [FrameControl; 3] = [
    FrameControl::Minimize,
    FrameControl::MaximizeRestore,
    FrameControl::Close,
];

/// Semantic cursor for a frame region (applied via the render-x11
/// `define_cursor(frame_xid, kind)` API by the live owner).
#[must_use]
pub const fn cursor_for_region(region: FrameRegion) -> CursorKind {
    match region {
        FrameRegion::Resize(edges) => cursor_for_edges(edges),
        FrameRegion::TitleDrag => CursorKind::Move,
        FrameRegion::Control(_) | FrameRegion::Client => CursorKind::Default,
    }
}

/// Cursor for a resize edge set: Top/Bottom -> vertical, Left/Right ->
/// horizontal, TL/BR -> NW-SE, TR/BL -> NE-SW.
#[must_use]
pub const fn cursor_for_edges(edges: ResizeEdges) -> CursorKind {
    if edges.is_corner() {
        if (edges.left && edges.top) || (edges.right && edges.bottom) {
            CursorKind::ResizeNorthWestSouthEast
        } else {
            CursorKind::ResizeNorthEastSouthWest
        }
    } else if edges.top || edges.bottom {
        CursorKind::ResizeVertical
    } else if edges.left || edges.right {
        CursorKind::ResizeHorizontal
    } else {
        CursorKind::Default
    }
}

/// Where a native event on a frame input child routes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameEventTarget {
    pub client: u32,
    pub region: FrameRegion,
}

/// Owned XIDs for one managed client: frame parent + 12 input children
/// (8 resize + 1 title-drag + 3 controls). The `resize` array follows
/// [`RESIZE_ORDER`]; `controls` follows [`CONTROL_ORDER`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameResources {
    pub client: u32,
    pub frame: u32,
    pub title_drag: u32,
    pub controls: [u32; 3],
    pub resize: [u32; 8],
}

impl FrameResources {
    #[must_use]
    pub const fn new(
        client: u32,
        frame: u32,
        title_drag: u32,
        controls: [u32; 3],
        resize: [u32; 8],
    ) -> Self {
        Self {
            client,
            frame,
            title_drag,
            controls,
            resize,
        }
    }

    /// All 12 input-child XIDs (excludes the frame parent).
    #[must_use]
    pub fn children(self) -> [u32; 12] {
        let mut out = [0u32; 12];
        out[0..8].copy_from_slice(&self.resize);
        out[8] = self.title_drag;
        out[9..12].copy_from_slice(&self.controls);
        out
    }

    /// Region for an input-child XID within this resource set.
    #[must_use]
    pub fn region_of(self, xid: u32) -> Option<FrameRegion> {
        if xid == self.title_drag {
            return Some(FrameRegion::TitleDrag);
        }
        if let Some(i) = self.controls.iter().position(|&c| c == xid) {
            return Some(FrameRegion::Control(CONTROL_ORDER[i]));
        }
        self.resize
            .iter()
            .position(|&r| r == xid)
            .map(|j| FrameRegion::Resize(RESIZE_ORDER[j]))
    }
}

/// Value/lifecycle owner: client XID -> resources, child XID -> event target.
/// `capture` is the single WM-lifetime root-child interaction-capture window,
/// owned here but not per-client (it survives client removal).
#[derive(Debug, Default)]
pub struct FrameRegistry {
    by_client: HashMap<u32, FrameResources>,
    by_xid: HashMap<u32, FrameEventTarget>,
    capture: Option<u32>,
}

impl FrameRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            by_client: HashMap::new(),
            by_xid: HashMap::new(),
            capture: None,
        }
    }

    /// Register a client's resource set. Effect for the live owner: create
    /// the frame + 12 input children, select [`INPUT_CHILD_EVENT_MASK`] on
    /// each child, define [`cursor_for_region`] per child. Re-registering a
    /// client first purges its old xid mappings; returns displaced child
    /// XIDs (effect: destroy them) so no orphan mapping survives.
    pub fn register(&mut self, resources: FrameResources) -> Vec<u32> {
        let mut displaced = Vec::new();
        if let Some(old) = self.by_client.insert(resources.client, resources) {
            for xid in old.children() {
                self.by_xid.remove(&xid);
                displaced.push(xid);
            }
        }
        for xid in resources.children() {
            if let Some(region) = resources.region_of(xid) {
                self.by_xid.insert(
                    xid,
                    FrameEventTarget {
                        client: resources.client,
                        region,
                    },
                );
            }
        }
        displaced
    }

    #[must_use]
    pub fn lookup_by_client(&self, client: u32) -> Option<FrameResources> {
        self.by_client.get(&client).copied()
    }

    #[must_use]
    pub fn lookup_by_xid(&self, xid: u32) -> Option<FrameEventTarget> {
        self.by_xid.get(&xid).copied()
    }

    /// Remove a client. Effect for the live owner: destroy the returned
    /// child XIDs (plus the frame parent from the looked-up resources).
    /// Returns the removed child XIDs; leaves zero orphan xid mappings.
    pub fn remove_client(&mut self, client: u32) -> Vec<u32> {
        let Some(resources) = self.by_client.remove(&client) else {
            return Vec::new();
        };
        let removed = resources.children().to_vec();
        for xid in &removed {
            self.by_xid.remove(xid);
        }
        removed
    }

    /// Set the WM-lifetime interaction-capture window (effect: create one
    /// root child on first use, reuse afterwards).
    pub fn set_capture(&mut self, xid: u32) {
        self.capture = Some(xid);
    }

    #[cfg(test)]
    /// Clear the interaction-capture window (effect: destroy it).
    pub fn clear_capture(&mut self) {
        self.capture = None;
    }

    #[must_use]
    pub fn capture(&self) -> Option<u32> {
        self.capture
    }

    #[cfg(test)]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_client.is_empty()
    }

    #[cfg(test)]
    #[must_use]
    pub fn xid_count(&self) -> usize {
        self.by_xid.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(client: u32, base: u32) -> FrameResources {
        FrameResources::new(
            client,
            base,
            base + 8,
            [base + 9, base + 10, base + 11],
            [
                base,
                base + 1,
                base + 2,
                base + 3,
                base + 4,
                base + 5,
                base + 6,
                base + 7,
            ],
        )
    }

    #[test]
    fn register_lookup_remove_roundtrip() {
        let mut reg = FrameRegistry::new();
        let res = sample(100, 1000);
        assert!(reg.register(res).is_empty());
        assert_eq!(reg.lookup_by_client(100), Some(res));
        let target = reg.lookup_by_xid(1008).expect("title child");
        assert_eq!(target.client, 100);
        assert_eq!(target.region, FrameRegion::TitleDrag);
        let removed = reg.remove_client(100);
        assert_eq!(removed.len(), 12);
        assert_eq!(reg.lookup_by_client(100), None);
        assert!(reg.is_empty());
    }

    #[test]
    fn remove_leaves_no_orphan_xids() {
        let mut reg = FrameRegistry::new();
        reg.register(sample(1, 100));
        reg.register(sample(2, 200));
        assert_eq!(reg.xid_count(), 24);
        let removed = reg.remove_client(1);
        assert_eq!(removed.len(), 12);
        assert_eq!(reg.xid_count(), 12);
        for xid in removed {
            assert_eq!(reg.lookup_by_xid(xid), None);
        }
        // Survivor still fully mapped.
        assert!(reg.lookup_by_client(2).is_some());
        assert!(reg.lookup_by_xid(208).is_some());
        // Unknown client removes nothing.
        assert!(reg.remove_client(999).is_empty());
        assert_eq!(reg.xid_count(), 12);
    }

    #[test]
    fn all_twelve_targets_map_to_one_client() {
        let mut reg = FrameRegistry::new();
        let res = sample(42, 500);
        reg.register(res);
        let mut count = 0;
        for xid in res.children() {
            let target = reg.lookup_by_xid(xid).expect("child maps");
            assert_eq!(target.client, 42);
            count += 1;
        }
        assert_eq!(count, 12);
        // Regions: 8 resize + 1 title + 3 controls.
        let mut resize = 0;
        let mut title = 0;
        let mut controls = 0;
        for xid in res.children() {
            match reg.lookup_by_xid(xid).expect("child maps").region {
                FrameRegion::Resize(_) => resize += 1,
                FrameRegion::TitleDrag => title += 1,
                FrameRegion::Control(_) => controls += 1,
                FrameRegion::Client => panic!("unexpected region"),
            }
        }
        assert_eq!((resize, title, controls), (8, 1, 3));
    }

    #[test]
    fn cursor_bindings_match_contract() {
        use CursorKind as C;
        assert_eq!(
            cursor_for_region(FrameRegion::Resize(ResizeEdges::top())),
            C::ResizeVertical
        );
        assert_eq!(
            cursor_for_region(FrameRegion::Resize(ResizeEdges::bottom())),
            C::ResizeVertical
        );
        assert_eq!(
            cursor_for_region(FrameRegion::Resize(ResizeEdges::left())),
            C::ResizeHorizontal
        );
        assert_eq!(
            cursor_for_region(FrameRegion::Resize(ResizeEdges::right())),
            C::ResizeHorizontal
        );
        assert_eq!(
            cursor_for_region(FrameRegion::Resize(ResizeEdges::top_left())),
            C::ResizeNorthWestSouthEast
        );
        assert_eq!(
            cursor_for_region(FrameRegion::Resize(ResizeEdges::bottom_right())),
            C::ResizeNorthWestSouthEast
        );
        assert_eq!(
            cursor_for_region(FrameRegion::Resize(ResizeEdges::top_right())),
            C::ResizeNorthEastSouthWest
        );
        assert_eq!(
            cursor_for_region(FrameRegion::Resize(ResizeEdges::bottom_left())),
            C::ResizeNorthEastSouthWest
        );
        assert_eq!(cursor_for_region(FrameRegion::TitleDrag), C::Move);
        assert_eq!(
            cursor_for_region(FrameRegion::Control(FrameControl::Close)),
            C::Default
        );
    }

    #[test]
    fn child_mask_includes_leave_window() {
        assert_ne!(INPUT_CHILD_EVENT_MASK & MASK_LEAVE_WINDOW, 0);
        assert_ne!(INPUT_CHILD_EVENT_MASK & MASK_ENTER_WINDOW, 0);
        assert_ne!(INPUT_CHILD_EVENT_MASK & MASK_POINTER_MOTION, 0);
    }

    #[test]
    fn capture_is_wm_lifetime_not_per_client() {
        let mut reg = FrameRegistry::new();
        reg.set_capture(7);
        assert_eq!(reg.capture(), Some(7));
        reg.register(sample(1, 100));
        reg.remove_client(1);
        // Capture survives client removal.
        assert_eq!(reg.capture(), Some(7));
        assert!(reg.is_empty());
        reg.clear_capture();
        assert_eq!(reg.capture(), None);
    }
}
