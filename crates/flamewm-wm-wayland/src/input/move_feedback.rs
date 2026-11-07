//! What an interactive move shows: the live client rect (content mode only) and the ring drawn
//! around the landing rect. Pure, so the two `showContentWhileDragging` modes are unit-tested here.

use crate::model::Rect;

/// Per-motion feedback for a move grab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MoveFeedback {
    /// Client rect the window is moved to now. `None` keeps the window in place (outline mode).
    pub live: Option<Rect>,
    /// Frame rect outlined by the selection-style ring. `None` hides the ring.
    pub ring: Option<Rect>,
}

/// Content mode moves the window live and outlines only a snap zone. Outline mode keeps the window
/// in place and outlines the snap zone when one is targeted, otherwise the free landing frame.
pub(super) fn move_feedback(
    show_content: bool,
    snap_zone: Option<Rect>,
    floating_client: Rect,
    floating_frame: Rect,
) -> MoveFeedback {
    let ring = match snap_zone {
        Some(zone) => Some(zone),
        None if show_content => None,
        None => Some(floating_frame),
    };
    MoveFeedback {
        live: show_content.then_some(floating_client),
        ring,
    }
}

/// Whether a paint must rebuild the cached scene. The DRM path reuses the cached elements on
/// pointer-only paints, so any ring or moved-window change has to force a full rebuild.
pub(super) const fn needs_scene_rebuild(ring_changed: bool, rect_changed: bool) -> bool {
    ring_changed || rect_changed
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIENT: Rect = Rect {
        x: 100,
        y: 130,
        w: 640,
        h: 400,
    };
    const FRAME: Rect = Rect {
        x: 100,
        y: 100,
        w: 640,
        h: 430,
    };
    const ZONE: Rect = Rect {
        x: 0,
        y: 0,
        w: 672,
        h: 637,
    };

    #[test]
    fn outline_mode_keeps_window_and_outlines_free_frame() {
        let fb = move_feedback(false, None, CLIENT, FRAME);
        assert_eq!(fb.live, None, "surface stays until release");
        assert_eq!(fb.ring, Some(FRAME));
    }

    #[test]
    fn outline_mode_outlines_snap_zone_when_targeted() {
        let fb = move_feedback(false, Some(ZONE), CLIENT, FRAME);
        assert_eq!(fb.live, None);
        assert_eq!(fb.ring, Some(ZONE));
    }

    #[test]
    fn content_mode_moves_live_and_outlines_only_snap_zone() {
        let free = move_feedback(true, None, CLIENT, FRAME);
        assert_eq!(free.live, Some(CLIENT));
        assert_eq!(free.ring, None, "no free-move outline in content mode");

        let snapped = move_feedback(true, Some(ZONE), CLIENT, FRAME);
        assert_eq!(snapped.live, Some(CLIENT));
        assert_eq!(snapped.ring, Some(ZONE));
    }

    #[test]
    fn scene_rebuilds_on_any_ring_or_rect_change_only() {
        assert!(needs_scene_rebuild(true, false));
        assert!(needs_scene_rebuild(false, true));
        assert!(needs_scene_rebuild(true, true));
        assert!(!needs_scene_rebuild(false, false));
    }
}
