use x11rb::protocol::xproto::Window;

use crate::chrome::IconImage;
use flamewm_integrations_linux::icons::IconKey;
use crate::classifier::WindowKind;
use crate::frame::coords::RootRect;
use crate::frame::model::{FrameControl, PlacementMode, PlacementState};
use crate::geometry::Rect;
use crate::size_hints::ClientSizeHints;

#[derive(Debug, Clone)]
pub struct ManagedClient {
    pub client: Window,
    pub frame: Window,
    /// Authoritative geometry + mode. Single source of truth for placement.
    pub placement: PlacementState,
    /// Parsed `WM_NORMAL_HINTS` for this client (client-pixel domain).
    pub hints: ClientSizeHints,
    /// Legacy `Rect` mirror of `placement.current`, projected on every
    /// placement write. `wm.rs` hot paths read this; `placement` stays
    /// authoritative.
    pub outer: Rect,
    pub workspace: usize,
    pub title: String,
    pub title_text_width: i32,
    /// Cached `_NET_WM_ICON` selection (largest valid image), alpha preserved.
    /// Falls back to the catalog `Icon=` raster (converted to `IconImage`)
    /// resolved from the cached `WM_CLASS` identity; `None` is an empty
    /// slot, never brand artwork.
    pub icon: Option<IconImage>,
    /// Debug reason when the icon slot falls back to `WM_CLASS` identity.
    pub icon_fallback: Option<String>,
    /// Monotonic lifetime/request fence for asynchronous catalog icons.
    pub icon_request_token: u64,
    /// Catalog request currently expected for this client, if any.
    pub catalog_icon_key: Option<IconKey>,
    /// Parsed `WM_CLASS` identity cached at manage time: instance (first
    /// NUL-separated token) and class (second token). Paint never performs
    /// an X round-trip for these; the `_NET_WM_ICON` property path refreshes
    /// them alongside the icon.
    pub wm_instance: String,
    pub wm_class: String,
    pub transient_for: Option<Window>,
    pub minimized: bool,
    pub sticky: bool,
    pub ignore_unmap: u8,
    pub kind: WindowKind,
    /// All-control hover target: `Some` when the pointer is inside the
    /// titlebar over a control button, else `None`. Motion feeds it;
    /// the frame redraws only on change.
    pub hover_control: Option<FrameControl>,
}

/// Convert a legacy frame `Rect` (u32 size) to a placement `RootRect`.
#[must_use]
pub fn rect_to_root(rect: Rect) -> RootRect {
    RootRect::from_legacy(rect)
}

/// Convert a placement `RootRect` back to a legacy frame `Rect`.
#[must_use]
pub fn root_to_rect(root: RootRect) -> Rect {
    root.to_legacy()
}

/// Floating placement with current == floating_restore and no resume.
#[must_use]
pub fn placement_floating(current: RootRect) -> PlacementState {
    PlacementState {
        mode: PlacementMode::Floating,
        current,
        floating_restore: current,
        resume: None,
    }
}

impl ManagedClient {
    pub fn visible_on(&self, workspace: usize) -> bool {
        !self.minimized && (self.sticky || self.workspace == workspace)
    }

    #[must_use]
    pub fn is_maximized(&self) -> bool {
        self.placement.mode == PlacementMode::Maximized
    }

    #[must_use]
    pub fn is_fullscreen(&self) -> bool {
        self.placement.mode == PlacementMode::Fullscreen
    }

    /// Pointer left the frame: drop the control hover highlight.
    /// Returns whether the painted state changed.
    pub fn clear_hover(&mut self) -> bool {
        self.hover_control.take().is_some()
    }

    /// Set the authoritative frame rect (floating-restore follows when
    /// floating; resume snapshot is left for explicit transitions).
    pub fn set_outer(&mut self, outer: Rect) {
        let root = rect_to_root(outer);
        self.placement.current = root;
        if self.placement.mode == PlacementMode::Floating {
            self.placement.floating_restore = root;
        }
        self.outer = outer;
    }

    /// Adopt an externally computed placement (e.g. geometry planner output).
    pub fn apply_placement(&mut self, placement: PlacementState) {
        self.placement = placement;
        self.outer = root_to_rect(self.placement.current);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_is_authoritative_over_legacy_mirrors() {
        let outer = Rect::new(100, 100, 400, 300);
        let root = rect_to_root(outer);
        assert_eq!(root_to_rect(root), outer);
        let floating = placement_floating(root);
        assert_eq!(floating.mode, PlacementMode::Floating);
        assert_eq!(floating.floating_restore, root);
        assert_eq!(floating.resume, None);
    }

    #[test]
    fn transition_maximize_and_restore_roundtrip() {
        let start = RootRect::new(100, 100, 400, 300);
        let max_rect = RootRect::new(0, 0, 1920, 1080);
        let floating = placement_floating(start);
        let mut client = ManagedClient {
            client: 1,
            frame: 2,
            placement: floating,
            hints: ClientSizeHints::default(),
            outer: root_to_rect(start),
            workspace: 0,
            title: String::new(),
            title_text_width: 0,
            icon: None,
            icon_fallback: None,
            icon_request_token: 0,
            catalog_icon_key: None,
            wm_instance: String::new(),
            wm_class: String::new(),
            transient_for: None,
            minimized: false,
            sticky: false,
            ignore_unmap: 0,
            kind: WindowKind::Normal,
            hover_control: None,
        };
        assert!(!client.is_maximized());
        assert!(!client.is_fullscreen());
        client.hover_control = Some(FrameControl::Close);
        assert!(client.clear_hover());
        assert_eq!(client.hover_control, None);
        assert!(!client.clear_hover());
        client.set_outer(root_to_rect(max_rect));
        assert_eq!(client.placement.current, max_rect);
        assert_eq!(client.outer, root_to_rect(max_rect));
    }
}
