use super::*;

const SOURCE: WarpRect = WarpRect {
    x: 100.0,
    y: 50.0,
    width: 400.0,
    height: 300.0,
};

#[test]
fn bounds_cover_source_and_target_with_integer_edges() {
    let target = WarpRect {
        x: 20.5,
        y: 600.2,
        width: 60.0,
        height: 40.0,
    };
    let (x, y, w, h) = union_bounds(SOURCE, target);
    assert_eq!((x, y), (20, 50));
    assert_eq!((w, h), (480, 591));
    assert!(f64::from(x) <= target.x && f64::from(y) <= SOURCE.y);
    assert!(f64::from(x + w as i32) >= SOURCE.x + SOURCE.width);
}

#[test]
fn begin_paths_fail_closed_without_display() {
    let capture = CapturedImage {
        width: 400,
        height: 300,
        argb: vec![0; 400 * 300 * 4],
    };
    assert!(WarpOverlay::begin(std::ptr::null_mut(), capture, SOURCE, SOURCE).is_err());
    let capture = CapturedImage {
        width: 40,
        height: 30,
        argb: vec![0; 40 * 30 * 4],
    };
    assert!(WarpOverlay::begin_fade(std::ptr::null_mut(), capture, 0, 0).is_err());
}

#[test]
fn backdrop_surface_is_chosen_only_without_argb() {
    assert_eq!(warp_surface_for(true, true), WarpSurface::Argb);
    // Without a compositing manager the ARGB window would show black, so the opaque
    // root-snapshot path must take over even when a 32-bit visual exists.
    assert_eq!(warp_surface_for(true, false), WarpSurface::OpaqueBackdrop);
    assert_eq!(warp_surface_for(false, true), WarpSurface::OpaqueBackdrop);
    assert_eq!(warp_surface_for(false, false), WarpSurface::OpaqueBackdrop);
}

#[test]
fn argb_probe_is_false_for_null_display() {
    assert!(!argb_visual_available(std::ptr::null_mut()));
    assert!(!warp_argb_usable(std::ptr::null_mut()));
    assert!(!compositing_manager_running(std::ptr::null_mut()));
}

#[test]
fn fade_runs_from_small_transparent_to_full_opaque() {
    assert_eq!(fade_frame(0.0), (FADE_START_SCALE, 0.0));
    assert_eq!(fade_frame(1.0), (1.0, 1.0));
    let (mid_scale, mid_alpha) = fade_frame(0.5);
    assert!(mid_scale > FADE_START_SCALE && mid_scale < 1.0);
    assert!(mid_alpha > 0.5, "ease-out leads the linear midpoint");
    let mut last = (0.0, 0.0);
    for step in 0..=10 {
        let frame = fade_frame(f64::from(step) / 10.0);
        assert!(
            frame.0 >= last.0 && frame.1 >= last.1,
            "monotone at step {step}"
        );
        last = frame;
    }
    assert_eq!(fade_frame(-3.0), fade_frame(0.0));
    assert_eq!(fade_frame(9.0), fade_frame(1.0));
}

#[test]
fn crossfade_alpha_has_exact_endpoints_per_direction() {
    assert_eq!(fade_alpha(0.0, TransitionDirection::In), 0.0);
    assert_eq!(fade_alpha(1.0, TransitionDirection::In), 1.0);
    assert_eq!(fade_alpha(0.0, TransitionDirection::Out), 1.0);
    assert_eq!(fade_alpha(1.0, TransitionDirection::Out), 0.0);
    assert_eq!(fade_alpha(-3.0, TransitionDirection::In), 0.0);
    assert_eq!(fade_alpha(9.0, TransitionDirection::In), 1.0);
    assert_eq!(fade_alpha(9.0, TransitionDirection::Out), 0.0);
}

#[test]
fn crossfade_alpha_is_eased_and_monotone_in_its_direction() {
    // Ease-out leads the linear midpoint: In rises fast, Out falls fast.
    assert_eq!(fade_alpha(0.5, TransitionDirection::In), 0.75);
    assert_eq!(fade_alpha(0.5, TransitionDirection::Out), 0.25);
    let (mut last_in, mut last_out) = (-1.0, 2.0);
    for step in 0..=20 {
        let t = f64::from(step) / 20.0;
        let (a_in, a_out) = (
            fade_alpha(t, TransitionDirection::In),
            fade_alpha(t, TransitionDirection::Out),
        );
        assert!(
            a_in >= last_in && a_out <= last_out,
            "monotone at step {step}"
        );
        assert!((0.0..=1.0).contains(&a_in) && (0.0..=1.0).contains(&a_out));
        assert!(
            (a_in + a_out - 1.0).abs() < 1e-12,
            "Out is the mirror of In"
        );
        (last_in, last_out) = (a_in, a_out);
    }
}

#[test]
fn rect_move_never_fades_below_minimum_alpha() {
    assert_eq!(
        crossfade_alpha(0.0, TransitionDirection::Out, true),
        MOVE_MIN_ALPHA
    );
    assert_eq!(crossfade_alpha(1.0, TransitionDirection::In, true), 1.0);
    // Without a rect move the plain fade applies.
    assert_eq!(crossfade_alpha(1.0, TransitionDirection::Out, false), 0.0);
}

#[test]
fn lerp_rect_hits_both_rects_at_the_ends() {
    let a = WarpRect {
        x: 10.0,
        y: 20.0,
        width: 100.0,
        height: 50.0,
    };
    let b = WarpRect {
        x: 0.0,
        y: 0.0,
        width: 400.0,
        height: 300.0,
    };
    assert_eq!(lerp_rect(a, b, 0.0), a);
    assert_eq!(lerp_rect(a, b, 1.0), b);
    assert_eq!(lerp_rect(a, a, 0.5), a);
}

#[test]
fn styled_begin_fails_closed_without_display_for_each_style() {
    for style in [
        TransitionStyle::MagicLamp,
        TransitionStyle::Fade,
        TransitionStyle::Stretch,
    ] {
        for direction in [TransitionDirection::In, TransitionDirection::Out] {
            let capture = CapturedImage {
                width: 400,
                height: 300,
                argb: vec![0; 400 * 300 * 4],
            };
            let result = WarpOverlay::begin_styled(
                std::ptr::null_mut(),
                capture,
                SOURCE,
                SOURCE,
                PanelEdge::Bottom,
                style,
                direction,
            );
            assert!(result.is_err(), "{style:?} {direction:?} must fail closed");
        }
    }
}

#[test]
fn stretch_frame_hits_source_and_target_exactly() {
    let a = WarpRect {
        x: 0.0,
        y: 489.0,
        width: 1278.0,
        height: 300.0,
    };
    let b = WarpRect {
        x: 0.0,
        y: 0.0,
        width: 1278.0,
        height: 1024.0,
    };
    assert_eq!(stretch_frame(a, b, 0.0), a);
    assert_eq!(stretch_frame(a, b, 1.0), b);
    assert_eq!(stretch_frame(a, b, 9.0), b);
    assert_eq!(stretch_frame(b, a, 0.0), b);
    assert_eq!(stretch_frame(b, a, 1.0), a);
}

#[test]
fn stretch_frame_is_monotone_between_the_rects() {
    let a = WarpRect {
        x: 200.0,
        y: 300.0,
        width: 100.0,
        height: 80.0,
    };
    let b = WarpRect {
        x: 0.0,
        y: 0.0,
        width: 1278.0,
        height: 1024.0,
    };
    let mut last = a;
    for step in 1..=20 {
        let r = stretch_frame(a, b, f64::from(step) / 20.0);
        assert!(
            r.x <= last.x + 1e-9 && r.y <= last.y + 1e-9,
            "origin moves toward target at {step}"
        );
        assert!(
            r.width >= last.width - 1e-9 && r.height >= last.height - 1e-9,
            "size grows at {step}"
        );
        last = r;
    }
}

#[test]
fn stretch_frame_has_no_nan_for_degenerate_rects() {
    let zero = WarpRect {
        x: 5.0,
        y: 5.0,
        width: 0.0,
        height: 0.0,
    };
    let wide = WarpRect {
        x: 0.0,
        y: 0.0,
        width: 800.0,
        height: 600.0,
    };
    for (a, b) in [(zero, wide), (wide, zero), (zero, zero)] {
        for step in 0..=10 {
            let r = stretch_frame(a, b, f64::from(step) / 10.0);
            assert!(
                r.x.is_finite()
                    && r.y.is_finite()
                    && r.width.is_finite()
                    && r.height.is_finite()
            );
        }
    }
}
