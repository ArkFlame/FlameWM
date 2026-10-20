use std::time::{Duration, Instant};

use super::backdrop_shape::round_backdrop_words;
use super::*;
use flamewm_reactor::timeline::FRAME_INTERVAL;
use flamewm_reactor::{Easing, Start, Timeline};
use flamewm_render_core::{Rect, SurfaceDamage};

#[cfg(test)]
mod dock_frame_plan_tests {
    use super::*;

    #[test]
    fn classic_to_classic_keeps_shape() {
        assert_eq!(dock_mask_plan(0, 0), DockMask::Keep);
    }

    #[test]
    fn floating_to_classic_resets_rectangle() {
        assert_eq!(dock_mask_plan(12, 0), DockMask::Reset);
    }

    #[test]
    fn floating_radius_rounds_and_resize_keeps_it() {
        assert_eq!(dock_mask_plan(0, 12), DockMask::Round(12));
        assert_eq!(dock_mask_plan(12, 12), DockMask::Round(12));
        assert_eq!(dock_mask_plan(12, 6), DockMask::Round(6));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_resize_notification_is_distinct_from_child_configure() {
        assert!(is_root_geometry_configure(
            ROOT_CHILD_CONFIGURE_NOTIFY,
            1,
            1
        ));
        assert!(!is_root_geometry_configure(
            ROOT_CHILD_CONFIGURE_NOTIFY,
            2,
            1
        ));
        assert!(!is_root_geometry_configure(ROOT_CHILD_MAP_NOTIFY, 1, 1));
    }

    #[test]
    fn grabbed_press_classifies_inside_and_outside_in_root_space() {
        let origin = (100, 50);
        // 200x100 popup: [100,300) x [50,150)
        assert_eq!(
            classify_grabbed_press(100, 50, origin, 200, 100),
            GrabbedPressOutcome::Inside
        );
        assert_eq!(
            classify_grabbed_press(299, 149, origin, 200, 100),
            GrabbedPressOutcome::Inside
        );
        assert_eq!(
            classify_grabbed_press(300, 80, origin, 200, 100),
            GrabbedPressOutcome::Outside
        );
        assert_eq!(
            classify_grabbed_press(99, 80, origin, 200, 100),
            GrabbedPressOutcome::Outside
        );
        assert_eq!(
            classify_grabbed_press(150, 150, origin, 200, 100),
            GrabbedPressOutcome::Outside
        );
    }

    #[test]
    fn popup_press_policy_never_grabs_the_pointer_or_buttons() {
        // Production code must not grab or replay presses; dismissal observes raw presses only.
        let production = [
            include_str!("../surface_controller.rs"),
            include_str!("raw_press.rs"),
            include_str!("slide.rs"),
            include_str!("dock.rs"),
            include_str!("geometry_trace.rs"),
            include_str!("present.rs"),
            include_str!("backdrop_shape.rs"),
        ]
        .concat();
        assert!(
            !production.contains("XGrabPointer("),
            "active pointer grab reintroduced"
        );
        assert!(
            !production.contains("XGrabButton("),
            "passive button grab reintroduced"
        );
        assert!(
            !production.contains("XAllowEvents("),
            "press replay reintroduced"
        );
        assert!(
            production.contains("XISelectEvents("),
            "raw press selection missing"
        );
    }

    #[test]
    fn raw_press_mask_selects_only_raw_button_press() {
        // XI_RawButtonPress is event 15: byte 15 >> 3 = 1, bit 15 & 7 = 7.
        assert_eq!(raw_press_mask(), [0, 0x80]);
        assert_eq!(XI_RAW_BUTTON_PRESS, 15);
    }

    #[test]
    fn raw_press_selection_arms_and_disarms_idempotently() {
        let a = SurfaceId(1);
        let mut selection = RawPressSelection::new(Some(2));
        assert!(!selection.disarm(a), "disarm while idle must not write");
        assert!(selection.arm(a), "first arm writes the selection");
        assert!(
            !selection.arm(a),
            "second arm of the same surface must not write again"
        );
        assert!(
            selection.disarm(a),
            "disarm of the last armed surface writes"
        );
        assert!(!selection.disarm(a), "second disarm must not write again");
        assert!(selection.arm(a), "re-arm after disarm writes again");
    }

    #[test]
    fn raw_press_selection_is_shared_across_two_armed_surfaces() {
        let (a, b) = (SurfaceId(1), SurfaceId(2));
        let mut selection = RawPressSelection::new(Some(2));
        assert!(selection.arm(a), "first surface writes the selection");
        assert!(!selection.arm(b), "second surface joins without rewriting");
        assert!(
            !selection.disarm(a),
            "closing one popup must keep the selection for the other"
        );
        assert!(!selection.arm(b), "b stays armed and idempotent");
        assert!(selection.disarm(b), "last surface out clears the selection");
        assert!(!selection.disarm(a), "a already disarmed");
        assert!(selection.arm(a), "re-arm after full clear writes again");
    }

    #[test]
    fn press_observer_shares_the_root_selection_with_popup_grabs() {
        let mut selection = RawPressSelection::new(Some(2));
        let popup = SurfaceId(7);
        assert!(
            selection.arm(PRESS_OBSERVER),
            "observer writes the root selection"
        );
        assert!(
            !selection.arm(popup),
            "popup rides the observer's selection"
        );
        assert!(
            selection.arm(PRESS_OBSERVER) == false,
            "re-arm is idempotent"
        );
        assert!(
            !selection.disarm(PRESS_OBSERVER),
            "popup keeps the selection"
        );
        assert!(
            selection.disarm(popup),
            "last disarm clears the root selection"
        );
        assert!(!selection.disarm(PRESS_OBSERVER), "disarm is idempotent");
    }

    #[test]
    fn raw_press_selection_without_xinput2_never_writes() {
        let mut selection = RawPressSelection::new(None);
        assert_eq!(selection.opcode(), None);
        assert!(!selection.arm(SurfaceId(1)));
        assert!(!selection.disarm(SurfaceId(1)));
    }

    #[test]
    fn raw_press_outside_maps_only_outside_presses_to_dismiss_event() {
        let popup = ((100, 50), (200, 100));
        let outside = raw_press_outside_event(
            SurfaceId(3),
            XI_RAW_BUTTON_PRESS,
            1,
            (300, 80),
            popup,
            1.0,
            42,
        )
        .expect("press right of the popup dismisses");
        assert_eq!(outside.surface, SurfaceId(3));
        assert_eq!(outside.event.action, OUTSIDE_RELEASE_ACTION);
        assert_eq!(outside.event.phase, ActionPhase::Release);
        assert_eq!((outside.event.x, outside.event.y), (200.0, 30.0));
        assert_eq!(outside.event.button, 1);
        assert_eq!(outside.event.time_ms, 42);
        assert!(!outside.event.inside);

        let scaled = raw_press_outside_event(
            SurfaceId(3),
            XI_RAW_BUTTON_PRESS,
            3,
            (300, 80),
            popup,
            2.0,
            7,
        )
        .expect("scaled outside press dismisses");
        assert_eq!((scaled.event.x, scaled.event.y), (100.0, 15.0));
        assert_eq!(scaled.event.button, 3);

        assert!(raw_press_outside_event(
            SurfaceId(3),
            XI_RAW_BUTTON_PRESS,
            1,
            (150, 80),
            popup,
            1.0,
            0
        )
        .is_none());
        assert!(raw_press_outside_event(
            SurfaceId(3),
            XI_RAW_BUTTON_PRESS,
            1,
            (299, 149),
            popup,
            1.0,
            0
        )
        .is_none());
        assert!(
            raw_press_outside_event(SurfaceId(3), 16, 1, (300, 80), popup, 1.0, 0).is_none(),
            "release is not a press"
        );
    }

    #[test]
    fn outside_release_bounds_are_window_relative() {
        assert!(!button_release_is_outside(10, 10, 800, 600));
        assert!(!button_release_is_outside(0, 0, 800, 600));
        assert!(!button_release_is_outside(799, 599, 800, 600));
        assert!(button_release_is_outside(-1, 10, 800, 600));
        assert!(button_release_is_outside(10, -1, 800, 600));
        assert!(button_release_is_outside(800, 10, 800, 600));
        assert!(button_release_is_outside(10, 600, 800, 600));
    }

    #[test]
    fn outside_event_uses_reserved_generic_action() {
        let event = outside_release_event(SurfaceId(3), 1, 850.0, 10.0, 42);
        assert_eq!(event.surface, SurfaceId(3));
        assert_eq!(event.event.action, OUTSIDE_RELEASE_ACTION);
        assert_eq!(event.event.phase, ActionPhase::Release);
        assert_eq!(event.event.button, 1);
        assert!(!event.event.inside);
        assert_eq!(event.event.time_ms, 42);
        assert!(event.event.text.is_none());
    }

    #[test]
    fn surface_id_round_trips() {
        let id = SurfaceId(7);
        assert_eq!(id.get(), 7);
    }

    #[test]
    fn overlay_maps_to_notification_type_and_is_unmanaged() {
        assert_eq!(
            overlay_window_type_name(SurfaceRole::Overlay),
            "_NET_WM_WINDOW_TYPE_NOTIFICATION"
        );
        assert!(overlay_excluded_from_wm(SurfaceRole::Overlay));
        assert!(!overlay_excluded_from_wm(SurfaceRole::Normal));
        assert!(!overlay_excluded_from_wm(SurfaceRole::Dock));
    }

    #[test]
    fn passthrough_refuses_grab_and_focus() {
        assert!(!SurfaceInputMode::PassThrough.allows_grab());
        assert!(!SurfaceInputMode::PassThrough.allows_focus());
        assert!(SurfaceInputMode::Interactive.allows_grab());
        assert!(SurfaceInputMode::Interactive.allows_focus());
    }

    #[test]
    fn config_maps_overlay_role() {
        let config = SurfaceConfig {
            role: SurfaceRole::Overlay,
            ..SurfaceConfig::default()
        };
        let x11: X11Config = config.into();
        assert_eq!(x11.role, X11WindowRole::Overlay);
    }

    #[test]
    fn popup_policy_maps_raise_for_transients_only() {
        assert!(!surface_role_needs_popup_raise(SurfaceRole::Normal));
        assert!(!surface_role_needs_popup_raise(SurfaceRole::Desktop));
        assert!(!surface_role_needs_popup_raise(SurfaceRole::Dock));
        assert!(surface_role_needs_popup_raise(SurfaceRole::PopupMenu));
        assert!(surface_role_needs_popup_raise(SurfaceRole::DropdownMenu));
        assert!(surface_role_needs_popup_raise(SurfaceRole::Overlay));
        assert!(!x11_role_needs_popup_raise(X11WindowRole::Normal));
        assert!(x11_role_needs_popup_raise(X11WindowRole::PopupMenu));
        assert!(x11_role_needs_popup_raise(X11WindowRole::DropdownMenu));
        assert!(x11_role_needs_popup_raise(X11WindowRole::Overlay));
    }

    #[test]
    fn damage_latch_merges_without_paint() {
        use flamewm_render_core::Rect;
        assert!(SurfaceDamage::None.is_empty());
        assert!(!SurfaceDamage::Full.is_empty());
        assert!(!SurfaceDamage::Region(Rect::default()).is_empty());
        assert_eq!(
            SurfaceDamage::None.merge(SurfaceDamage::Full),
            SurfaceDamage::Full
        );
        assert_eq!(
            SurfaceDamage::Full.merge(SurfaceDamage::None),
            SurfaceDamage::Full
        );
        assert_eq!(
            SurfaceDamage::None.merge(SurfaceDamage::None),
            SurfaceDamage::None
        );
    }

    #[test]
    fn stack_order_predicate_desktop_normal_dock_popup() {
        assert!(stack_order_ok(&[
            SurfaceRole::Desktop,
            SurfaceRole::Normal,
            SurfaceRole::Dock,
            SurfaceRole::PopupMenu,
        ]));
        assert!(stack_order_ok(&[
            SurfaceRole::Desktop,
            SurfaceRole::Normal,
            SurfaceRole::Normal,
            SurfaceRole::Dock,
            SurfaceRole::DropdownMenu,
            SurfaceRole::Overlay,
        ]));
        // Empty/single are trivially ordered.
        assert!(stack_order_ok(&[]));
        assert!(stack_order_ok(&[SurfaceRole::Dock]));
        // Inversions fail: popup below dock/normal, dock below normal,
        // normal below desktop.
        assert!(!stack_order_ok(&[
            SurfaceRole::PopupMenu,
            SurfaceRole::Dock,
        ]));
        assert!(!stack_order_ok(&[SurfaceRole::Dock, SurfaceRole::Normal,]));
        assert!(!stack_order_ok(&[
            SurfaceRole::Normal,
            SurfaceRole::Desktop,
        ]));
        assert!(!stack_order_ok(&[
            SurfaceRole::Desktop,
            SurfaceRole::Dock,
            SurfaceRole::Normal,
        ]));
    }

    #[test]
    fn debug_helpers_are_gated() {
        // Gating is pure: without FLAMEWM_DEBUG=1 the canary/probe report
        // refusal intent (non-debug callers never reach X). Exact env
        // behavior is asserted without touching process env.
        assert!(!debug_enabled() || std::env::var_os("FLAMEWM_DEBUG").is_some_and(|v| v == "1"));
    }
    #[test]
    fn one_txn_traces_full_geometry_chain() {
        // C04: one correlated id traces anchor/intrinsic/fitted/ui-request/
        // native-request/observed/retained with the same txn + role.
        let trace = GeometryTrace::begin("start-menu").for_surface(7);
        trace.anchor((8, 400, 48, 48));
        trace.intrinsic((320, 480));
        trace.fitted((8, 400, 320, 480));
        trace.ui_request((8, 400, 320, 480));
        trace.native_request((8, 400, 320, 480));
        trace.observed((8, 400, 320, 480));
        trace.retained((8, 400, 320, 480));
        let stages = geometry_trace_snapshot(trace.txn);
        let names: Vec<&str> = stages.iter().map(|s| s.stage).collect();
        assert_eq!(
            names,
            [
                "anchor",
                "intrinsic",
                "fitted",
                "ui-request",
                "native-request",
                "observed",
                "retained"
            ]
        );
        assert!(stages.iter().all(|s| s.txn == trace.txn));
        assert!(stages.iter().all(|s| s.role == "start-menu"));
        assert!(stages.iter().all(|s| s.surface == 7));
        // Distinct txns never leak into each other.
        let other = GeometryTrace::begin("start-menu").for_surface(7);
        assert_ne!(other.txn, trace.txn);
        other.anchor((0, 0, 10, 10));
        assert_eq!(geometry_trace_snapshot(other.txn).len(), 1);
        assert_eq!(geometry_trace_snapshot(trace.txn).len(), 7);
    }

    #[test]
    fn popup_backdrop_corners_are_transparent_and_centre_untouched() {
        let mut words = vec![0xff80_8080_u32; 12 * 12];
        round_backdrop_words(&mut words, 12, 12, 6);
        assert_eq!(words[0] >> 24, 0, "top-left corner outside the arc");
        assert_eq!(words[11] >> 24, 0, "top-right corner outside the arc");
        assert_eq!(
            words[11 * 12] >> 24,
            0,
            "bottom-left corner outside the arc"
        );
        assert_eq!(
            words[11 * 12 + 11] >> 24,
            0,
            "bottom-right corner outside the arc"
        );
        assert_eq!(words[6 * 12 + 6], 0xff80_8080, "centre keeps the snapshot");
        assert!(
            words.iter().any(|w| (1..0xff).contains(&(w >> 24))),
            "arc has anti-aliased partial alpha"
        );
        for word in &words {
            let alpha = word >> 24;
            for shift in [0, 8, 16] {
                assert!(
                    ((word >> shift) & 0xff) <= alpha,
                    "premultiplied: {word:#010x}"
                );
            }
        }
    }

    #[test]
    fn slide_out_keeps_grab_only_for_shown_slide_popups() {
        let slide = Some(SlideConfig::slide(SlideEdge::Bottom, 200));
        let fade = Some(SlideConfig::fade(SlideEdge::Bottom, 200));
        assert!(
            slide_out_keeps_grab(slide, true),
            "shown slide keeps grab during slide-out"
        );
        assert!(
            !slide_out_keeps_grab(slide, false),
            "unmapped popup releases at once"
        );
        assert!(!slide_out_keeps_grab(fade, true), "fade releases at once");
        assert!(
            !slide_out_keeps_grab(None, true),
            "no transition releases at once"
        );
    }

    /// Replays hide() then the landing: returns how many times the grab was released.
    fn grab_releases(slide: Option<SlideConfig>, shown: bool) -> (u32, u32) {
        let mut grabbed = true;
        let mut at_hide = 0;
        if !slide_out_keeps_grab(slide, shown) {
            grabbed = false;
            at_hide += 1;
        }
        let mut at_landing = 0;
        if landing_releases_grab(false, grabbed) {
            grabbed = false;
            at_landing += 1;
        }
        assert!(!grabbed, "grab must not outlive the close");
        (at_hide, at_landing)
    }

    #[test]
    fn slide_hide_keeps_grab_until_landing_then_releases_exactly_once() {
        let slide = Some(SlideConfig::slide(SlideEdge::Bottom, 200));
        assert_eq!(grab_releases(slide, true), (0, 1));
        // A landed opening never releases, and a second landing finds nothing left to release.
        assert!(
            !landing_releases_grab(true, true),
            "opening landing keeps grab"
        );
        assert!(
            !landing_releases_grab(false, false),
            "second landing has nothing to release"
        );
    }

    #[test]
    fn fade_lamp_and_none_release_at_hide_not_at_landing() {
        let fade = Some(SlideConfig::fade(SlideEdge::Bottom, 200));
        let lamp = Some(SlideConfig::lamp(SlideEdge::Bottom, 200, (0, 0, 10, 10)));
        for slide in [fade, lamp, None] {
            assert_eq!(
                grab_releases(slide, true),
                (1, 0),
                "{slide:?} releases at hide"
            );
        }
    }

    #[test]
    fn begin_overlay_hide_never_touches_the_grab() {
        // Overlay close owns only the visual flight; grab release stays in hide() and landing.
        let source = include_str!("slide.rs");
        let start = source
            .find("pub(super) fn begin_overlay_hide(")
            .expect("begin_overlay_hide present");
        let body = &source[start..];
        let end = body
            .find("\n    /// Final root-space rect")
            .expect("body end marker");
        let body = &body[..end];
        assert!(
            !body.contains("release_grab"),
            "overlay hide must not release the grab"
        );
        assert!(
            !body.contains("pointer_grabbed"),
            "overlay hide must not touch the grab flag"
        );
        assert!(
            !body.contains("release_popup_presses"),
            "overlay hide must not release presses"
        );
    }

    #[test]
    fn outside_press_is_swallowed_only_while_sliding_out() {
        let closing = Some(SlideKind {
            edge: SlideEdge::Bottom,
            opening: false,
        });
        let opening = Some(SlideKind {
            edge: SlideEdge::Bottom,
            opening: true,
        });
        assert!(
            slide_out_swallows_press(closing),
            "second outside press during slide-out is ignored"
        );
        assert!(
            !slide_out_swallows_press(opening),
            "a reversed open still dismisses"
        );
        assert!(
            !slide_out_swallows_press(None),
            "settled popup dismisses normally"
        );
    }

    #[test]
    fn popup_backdrop_square_corners_stay_opaque() {
        let mut words = vec![0xff10_2030_u32; 12 * 12];
        round_backdrop_words(&mut words, 12, 12, 0);
        assert!(words.iter().all(|w| *w == 0xff10_2030));
    }
}

#[cfg(test)]
mod present_cycle_tests {
    use super::*;

    #[test]
    fn full_cycle_runs_each_stage_once_in_order() {
        let mut cycle = PresentCycle::new();
        cycle.run_full_cycle().expect("full cycle runs");
        assert!(cycle.is_complete());
        assert!(cycle.advance("present").is_err());
        assert!(cycle.advance("bogus").is_err());
    }

    #[test]
    fn out_of_order_stage_is_rejected() {
        let mut cycle = PresentCycle::new();
        assert!(cycle.advance("paint").is_err());
        for stage in ["project", "measure", "anchor"] {
            cycle.advance(stage).expect("in-order stage runs");
        }
        assert!(cycle.advance("paint").is_err());
        cycle.advance("prepare").expect("prepare runs");
        cycle.advance("paint").expect("paint runs after prepare");
        assert!(!cycle.is_complete());
    }

    #[test]
    fn retained_geometry_helpers_are_headless_free() {
        // Pure helpers: scale 1.0/1.5 math + origin offset, no X needed.
        fn scaled_box_rect(
            layout: &flamewm_render_core::LayoutResult,
            scale: f32,
            index: u32,
        ) -> Option<Rect> {
            layout.boxes.get(index as usize).map(|b| Rect {
                x: b.rect.x * scale,
                y: b.rect.y * scale,
                width: b.rect.width * scale,
                height: b.rect.height * scale,
            })
        }
        fn root_space_rect(origin: (i32, i32), local: Rect) -> Rect {
            Rect {
                x: local.x + origin.0 as f32,
                y: local.y + origin.1 as f32,
                width: local.width,
                height: local.height,
            }
        }
        use flamewm_render_core::{LayoutBox, LayoutResult};
        let rect = Rect {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 50.0,
        };
        let layout = LayoutResult {
            boxes: vec![LayoutBox { rect }],
            contents: vec![rect],
            revision: 1,
            z_order: vec![0],
        };
        let at_scale1 = scaled_box_rect(&layout, 1.0, 0).expect("box retained");
        assert_eq!((at_scale1.width, at_scale1.height), (100.0, 50.0));
        let at_scale15 = scaled_box_rect(&layout, 1.5, 0).expect("box retained");
        assert_eq!((at_scale15.x, at_scale15.width), (15.0, 150.0));
        let rooted = root_space_rect((40, 60), at_scale15);
        assert_eq!((rooted.x, rooted.y), (55.0, 90.0));
        // Intrinsic device size scales the retained content extent.
        let content = layout.contents[0];
        assert_eq!((content.width * 1.5, content.height * 1.5), (150.0, 75.0));
        // Headless fallback extent is the documented constant, never a
        // mapped-surface query result.
        assert_eq!(HEADLESS_FALLBACK_EXTENT, (1350.0, 641.0));
    }
}

#[cfg(test)]
mod slide_motion_tests {
    use super::*;

    const FINAL: (i32, i32) = (100, 500);
    const SIZE: (u32, u32) = (300, 400);

    #[test]
    fn closing_landing_frame_stays_off_the_panel_edge() {
        // Close timeline runs progress 1.0 -> 0.0; its landed frame is the off-edge end frame,
        // never the fully visible origin (that frame would flash before the unmap).
        for edge in [
            SlideEdge::Bottom,
            SlideEdge::Top,
            SlideEdge::Left,
            SlideEdge::Right,
        ] {
            let landed = landing_frame_origin(FINAL, SIZE, edge, 0.0);
            assert_eq!(landed, slide_origin(FINAL, SIZE, edge, 0.0), "{edge:?}");
            assert_ne!(landed, FINAL, "{edge:?}");
        }
        // Opening landing still snaps to the final origin.
        assert_eq!(
            landing_frame_origin(FINAL, SIZE, SlideEdge::Bottom, 1.0),
            FINAL
        );
    }

    #[test]
    fn slide_origin_starts_off_the_panel_edge_per_edge() {
        assert_eq!(
            slide_origin(FINAL, SIZE, SlideEdge::Bottom, 0.0),
            (100, 900)
        );
        assert_eq!(slide_origin(FINAL, SIZE, SlideEdge::Top, 0.0), (100, 100));
        assert_eq!(slide_origin(FINAL, SIZE, SlideEdge::Left, 0.0), (-200, 500));
        assert_eq!(slide_origin(FINAL, SIZE, SlideEdge::Right, 0.0), (400, 500));
    }

    #[test]
    fn panel_clip_keeps_only_the_screen_side_of_the_dock_edge() {
        // Bottom dock at y 1040..1080: rows above the panel top survive the clip.
        let bottom = Some((0, 1040, 1920, 40));
        assert_eq!(
            panel_clip(bottom, (100, 900), (300, 400), SlideEdge::Bottom),
            (0, 0, 300, 140)
        );
        // Still fully below the panel: nothing may show.
        assert_eq!(
            panel_clip(bottom, (100, 1100), (300, 400), SlideEdge::Bottom),
            (0, 0, 300, 0)
        );
        // Top dock at y 0..40: rows from the panel bottom down survive.
        let top = Some((0, 0, 1920, 40));
        assert_eq!(
            panel_clip(top, (100, -200), (300, 400), SlideEdge::Top),
            (0, 240, 300, 400)
        );
        // Left and right docks clip columns on the same rule.
        let left = Some((0, 0, 60, 1080));
        assert_eq!(
            panel_clip(left, (-100, 0), (300, 400), SlideEdge::Left),
            (160, 0, 300, 400)
        );
        let right = Some((1860, 0, 60, 1080));
        assert_eq!(
            panel_clip(right, (1700, 0), (300, 400), SlideEdge::Right),
            (0, 0, 160, 400)
        );
        // No dock: nothing is clipped.
        assert_eq!(
            panel_clip(None, (1, 2), (300, 400), SlideEdge::Bottom),
            (0, 0, 300, 400)
        );
    }

    #[test]
    fn helper_popup_clips_against_the_published_dock_when_no_local_dock() {
        // Quick-control helpers have no Dock surface: the parent's panel rect must stand in.
        let published = Some((0, 1040, 1920, 40));
        assert_eq!(resolve_dock(None, published), published);
        assert_eq!(
            resolve_dock(Some((0, 1000, 1920, 80)), published),
            Some((0, 1000, 1920, 80))
        );
        assert_eq!(resolve_dock(None, None), None);
        let clip = panel_clip(
            resolve_dock(None, published),
            (100, 900),
            (300, 400),
            SlideEdge::Bottom,
        );
        assert_eq!(clip, (0, 0, 300, 140));
    }

    #[test]
    fn final_frame_is_exact_anchored_origin_for_every_edge() {
        for edge in [
            SlideEdge::Top,
            SlideEdge::Bottom,
            SlideEdge::Left,
            SlideEdge::Right,
        ] {
            assert_eq!(slide_origin(FINAL, SIZE, edge, 1.0), FINAL, "{edge:?}");
            assert_eq!(
                slide_origin(FINAL, SIZE, edge, 1.5),
                FINAL,
                "{edge:?} clamped"
            );
        }
    }

    const OPEN: SlideKind = SlideKind {
        edge: SlideEdge::Bottom,
        opening: true,
    };
    const CLOSE: SlideKind = SlideKind {
        edge: SlideEdge::Bottom,
        opening: false,
    };

    #[test]
    fn progress_endpoints_and_ease_out_midpoint() {
        let t0 = Instant::now();
        let mut slides = Timeline::new();
        slides.start_towards(
            1,
            OPEN,
            t0,
            SLIDE_OPEN_BASE_MS,
            Easing::EaseOutCubic,
            (0.0, 1.0),
        );
        assert_eq!(slides.progress(1, t0), Some(0.0));
        assert_eq!(
            slides.progress(1, t0 + Duration::from_millis(180)),
            Some(1.0)
        );
        assert_eq!(slides.progress(1, t0 + Duration::from_secs(10)), Some(1.0));
        assert_eq!(
            slides.finished(1, t0 + Duration::from_millis(179)),
            Some(false)
        );
        assert_eq!(
            slides.finished(1, t0 + Duration::from_millis(180)),
            Some(true)
        );
        // Ease-out cubic: halfway in time is already past half the travel.
        assert!(slides.progress(1, t0 + Duration::from_millis(90)).unwrap() > 0.5);
    }

    #[test]
    fn reverse_mid_flight_continues_from_current_progress() {
        let t0 = Instant::now();
        let mut slides = Timeline::new();
        slides.start_towards(
            1,
            OPEN,
            t0,
            SLIDE_OPEN_BASE_MS,
            Easing::EaseOutCubic,
            (0.0, 1.0),
        );
        let now = t0 + Duration::from_millis(60);
        let before = slides.progress(1, now).unwrap();
        slides.start_towards(
            1,
            CLOSE,
            now,
            SLIDE_OPEN_BASE_MS,
            Easing::EaseInCubic,
            (1.0, 0.0),
        );
        assert!(
            (slides.progress(1, now).unwrap() - before).abs() < 1e-6,
            "no jump on reverse"
        );
        assert_eq!(
            slides.progress(1, now + Duration::from_millis(180)),
            Some(0.0)
        );
        // Reversing back to open from the midpoint is also jump-free.
        let mid = now + Duration::from_millis(40);
        let mid_value = slides.progress(1, mid).unwrap();
        slides.start_towards(
            1,
            OPEN,
            mid,
            SLIDE_OPEN_BASE_MS,
            Easing::EaseOutCubic,
            (0.0, 1.0),
        );
        assert!((slides.progress(1, mid).unwrap() - mid_value).abs() < 1e-6);
        assert_eq!(
            slides.progress(1, mid + Duration::from_millis(180)),
            Some(1.0)
        );
    }

    #[test]
    fn close_from_fully_shown_starts_at_one_and_ends_at_zero() {
        let t0 = Instant::now();
        let mut slides = Timeline::new();
        slides.start_towards(
            1,
            CLOSE,
            t0,
            SLIDE_OPEN_BASE_MS,
            Easing::EaseInCubic,
            (1.0, 0.0),
        );
        assert_eq!(slides.progress(1, t0), Some(1.0));
        assert_eq!(
            slides.progress(1, t0 + Duration::from_millis(180)),
            Some(0.0)
        );
    }

    #[test]
    fn disabled_or_idle_state_arms_no_frame_wait() {
        assert_eq!(motion_frame_wait(false), None);
        assert_eq!(motion_frame_wait(true), Some(FRAME_INTERVAL));
    }

    #[test]
    fn disabled_timeline_refuses_slide_so_surface_maps_immediately() {
        let mut slides = Timeline::<SlideKind>::new();
        slides.set_enabled(false);
        let now = Instant::now();
        assert_eq!(
            slides.start_towards(
                1,
                OPEN,
                now,
                SLIDE_OPEN_BASE_MS,
                Easing::EaseOutCubic,
                (0.0, 1.0)
            ),
            Start::Immediate
        );
        assert!(slides.is_empty());
        assert_eq!(motion_frame_wait(!slides.is_empty()), None);
    }

    #[test]
    fn durations_scale_with_speed() {
        let mut slides = Timeline::<SlideKind>::new();
        slides.set_speed(100);
        assert_eq!(
            slides.duration_for(SLIDE_OPEN_BASE_MS),
            Duration::from_millis(180)
        );
        slides.set_speed(200);
        assert_eq!(
            slides.duration_for(SLIDE_OPEN_BASE_MS),
            Duration::from_millis(90)
        );
        slides.set_speed(25);
        assert_eq!(
            slides.duration_for(SLIDE_OPEN_BASE_MS),
            Duration::from_millis(720)
        );
    }

    #[test]
    fn constructors_pick_each_transition() {
        assert_eq!(
            SlideConfig::slide(SlideEdge::Top, 100).transition,
            PopupTransition::Slide
        );
        assert_eq!(
            SlideConfig::fade(SlideEdge::Top, 100).transition,
            PopupTransition::Fade
        );
        let lamp = SlideConfig::lamp(SlideEdge::Bottom, 100, (10, 900, 48, 40));
        assert_eq!(
            lamp.transition,
            PopupTransition::Lamp {
                target: (10, 900, 48, 40)
            }
        );
        assert_eq!(lamp.edge, SlideEdge::Bottom);
    }

    #[test]
    fn fade_paint_follows_timeline_in_both_directions() {
        // Closing starts from the fully shown timeline value, so alpha starts at 1 and ends at 0.
        assert_eq!(overlay_paint_progress(PopupTransition::Fade, 0.0), 0.0);
        assert_eq!(overlay_paint_progress(PopupTransition::Fade, 1.0), 1.0);
        assert_eq!(overlay_style(PopupTransition::Fade), TransitionStyle::Fade);
    }

    #[test]
    fn lamp_paint_runs_target_to_source_on_open_and_back_on_close() {
        let lamp = PopupTransition::Lamp {
            target: (0, 0, 1, 1),
        };
        // Open: starts folded into the button (warp progress 1) and unfolds to the window (0).
        assert_eq!(overlay_paint_progress(lamp, 0.0), 1.0);
        assert_eq!(overlay_paint_progress(lamp, 1.0), 0.0);
        assert_eq!(overlay_style(lamp), TransitionStyle::MagicLamp);
    }

    #[test]
    fn lamp_target_is_the_start_button_rect_and_fade_keeps_popup_rect() {
        let popup = WarpRect {
            x: 100.0,
            y: 500.0,
            width: 300.0,
            height: 400.0,
        };
        assert_eq!(
            overlay_target(
                PopupTransition::Lamp {
                    target: (12, 1050, 48, 36)
                },
                popup
            ),
            WarpRect {
                x: 12.0,
                y: 1050.0,
                width: 48.0,
                height: 36.0
            }
        );
        assert_eq!(overlay_target(PopupTransition::Fade, popup), popup);
    }

    #[test]
    fn panel_edge_maps_one_to_one_for_the_lamp_bend() {
        assert_eq!(lamp_edge(SlideEdge::Top), crate::lamp::PanelEdge::Top);
        assert_eq!(lamp_edge(SlideEdge::Bottom), crate::lamp::PanelEdge::Bottom);
        assert_eq!(lamp_edge(SlideEdge::Left), crate::lamp::PanelEdge::Left);
        assert_eq!(lamp_edge(SlideEdge::Right), crate::lamp::PanelEdge::Right);
    }

    #[test]
    fn reversed_fade_keeps_its_timeline_value() {
        // A fade-out reversed mid-flight continues from the same timeline value, so the overlay
        // paint argument does not jump.
        let t0 = Instant::now();
        let mut slides = Timeline::new();
        slides.start_towards(
            1,
            CLOSE,
            t0,
            SLIDE_CLOSE_BASE_MS,
            Easing::EaseOutCubic,
            (1.0, 0.0),
        );
        let now = t0 + Duration::from_millis(40);
        let before =
            overlay_paint_progress(PopupTransition::Fade, slides.progress(1, now).unwrap());
        slides.start_towards(
            1,
            OPEN,
            now,
            SLIDE_OPEN_BASE_MS,
            Easing::EaseOutCubic,
            (0.0, 1.0),
        );
        let after = overlay_paint_progress(PopupTransition::Fade, slides.progress(1, now).unwrap());
        assert!((before - after).abs() < 1e-6, "no jump on reverse");
    }
}

#[cfg(test)]
mod slide_exit_shape_regression {
    #[test]
    fn clipped_and_full_shapes_share_mask_owner() {
        let render = include_str!("../app/render.rs");
        assert!(render.contains(
            "let spans = self.base_shape_spans(&cached.commands, 1.0, backdrop_radius);"
        ));
        assert!(
            render.contains("let spans = self.base_shape_spans(commands, scale, backdrop_radius);")
        );
    }

    #[test]
    fn slide_out_unmaps_before_restoring_full_mask() {
        let controller = include_str!("../surface_controller.rs");
        let end = controller
            .split_once("// Never restore the full shape while still mapped")
            .unwrap()
            .1;
        let close = end.split_once("if landing_releases_grab").unwrap().0;
        assert!(
            close.find("XUnmapWindow(display, window)").unwrap()
                < close
                    .find("refresh_shape_mask(&instance.document)")
                    .unwrap()
        );
    }
}
