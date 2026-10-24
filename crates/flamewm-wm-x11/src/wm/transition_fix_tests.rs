    use super::*;
    use crate::frame::coords::RootRect;

    fn maximized(resume: Option<PlacementSnapshot>, restore: RootRect) -> PlacementState {
        PlacementState {
            mode: PlacementMode::Maximized,
            current: RootRect::new(0, 0, 1920, 1080),
            floating_restore: restore,
            resume,
        }
    }

    #[test]
    fn unmaximize_target_returns_floating_rect_for_maximized_client() {
        let floating = RootRect::new(500, 200, 400, 300);
        let snap = PlacementSnapshot {
            mode: PlacementMode::Floating,
            rect: floating,
        };
        assert_eq!(
            unmaximize_target(&maximized(Some(snap), floating)),
            Some(floating)
        );
        assert_eq!(
            unmaximize_target(&maximized(None, floating)),
            Some(floating)
        );
    }

    #[test]
    fn unmaximize_target_is_none_when_not_maximized_or_not_floating_restore() {
        let floating = RootRect::new(500, 200, 400, 300);
        let mut floating_state = maximized(None, floating);
        floating_state.mode = PlacementMode::Floating;
        assert_eq!(unmaximize_target(&floating_state), None);
        let fullscreen_snap = PlacementSnapshot {
            mode: PlacementMode::Fullscreen,
            rect: floating,
        };
        assert_eq!(
            unmaximize_target(&maximized(Some(fullscreen_snap), floating)),
            None
        );
    }

    #[test]
    fn expected_unmap_is_consumed_exactly_once() {
        let mut ignore = 1u8;
        assert!(consume_expected_unmap(&mut ignore));
        assert!(
            !consume_expected_unmap(&mut ignore),
            "no count left for a real unmap"
        );
    }

    /// Models the frame's UnmapNotify stream. Minimize hides the frame and counts one expected
    /// unmap; restore maps the frame (no UnmapNotify). A later client-initiated unmap must
    /// reach the WM. Before the fix, restore's extra count swallowed that unmap.
    #[test]
    fn restore_does_not_leave_an_ignore_count_that_swallows_a_later_unmap() {
        // minimize: WM unmaps the frame and expects exactly one UnmapNotify.
        let mut ignore_unmap = 1u8;
        assert!(
            consume_expected_unmap(&mut ignore_unmap),
            "minimize unmap is ours"
        );
        // restore: WM maps the frame; mapping produces no UnmapNotify, so no count is added.
        assert_eq!(ignore_unmap, 0);
        // later: the client withdraws its own window; this unmap must not be swallowed.
        assert!(
            !consume_expected_unmap(&mut ignore_unmap),
            "client unmap is handled"
        );
    }
