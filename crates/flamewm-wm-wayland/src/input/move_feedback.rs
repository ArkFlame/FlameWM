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

/// Rounded delta a move or resize grab applies from its origin to the current pointer sample.
/// Geometry only ever sees this integer step, so a high-rate burst collapses to its last sample
/// without changing the result.
pub(super) fn grab_delta(origin: (f64, f64), latest: (f64, f64)) -> (i32, i32) {
    (
        (latest.0 - origin.0).round() as i32,
        (latest.1 - origin.1).round() as i32,
    )
}

/// Whether a grab release owes a repaint. Hiding the ring changes the scene even when no geometry
/// committed; an outline-mode commit has never been drawn live and must be shown at release.
pub(super) const fn release_needs_paint(ring_hidden: bool, geometry_committed: bool) -> bool {
    ring_hidden || geometry_committed
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

    #[test]
    fn grab_delta_uses_the_latest_rounded_sample() {
        assert_eq!(grab_delta((100.0, 50.0), (112.4, 47.6)), (12, -2));
        assert_eq!(grab_delta((100.0, 50.0), (112.5, 47.5)), (13, -3));
        // Sub-pixel samples inside one frame round to the same geometry step.
        assert_eq!(
            grab_delta((100.0, 50.0), (110.1, 51.9)),
            grab_delta((100.0, 50.0), (110.4, 51.6))
        );
        assert_eq!(grab_delta((100.0, 50.0), (100.0, 50.0)), (0, 0));
    }

    #[test]
    fn release_repaints_for_hidden_ring_or_committed_geometry() {
        assert!(release_needs_paint(true, false));
        assert!(release_needs_paint(false, true));
        assert!(release_needs_paint(true, true));
        assert!(!release_needs_paint(false, false));
    }
}
