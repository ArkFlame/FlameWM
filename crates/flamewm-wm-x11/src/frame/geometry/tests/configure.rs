use super::*;

#[test]
fn client_rect_frame_roundtrip() {
    let local = frame_to_client_local(START, EXT);
    assert_eq!(local, ClientLocalRect::new(0, 31, 400, 269));
    let root = frame_to_client_root(START, EXT);
    assert_eq!(root, ClientRootRect::new(100, 131, 400, 269));
    assert_eq!(root.size_u32(), (400, 269));
    assert_eq!(
        client_rect_to_frame(root.origin(), root.size_u32(), EXT),
        START
    );
}

#[test]
fn client_configure_applies_hints_and_gravity() {
    // min 100x100, max 500x400; request 2000x2000 at (300,300).
    let r = client_configure(min_max((100, 100), (500, 400)));
    let p = plan_client_configure(&r, RootPoint::new(300, 300), Some((2000, 2000)));
    // NW gravity: origin pinned to previous client origin (100,131).
    assert_eq!(p.client_size, (500, 400));
    assert_eq!(p.frame, RootRect::new(100, 100, 500, 431));
}

#[test]
fn client_configure_explicit_origin_wins_over_gravity_on_resize() {
    let r = client_configure(free());
    let gravity_only = plan_client_configure(&r, RootPoint::new(300, 300), Some((500, 400)));
    let explicit =
        plan_client_configure_explicit(&r, RootPoint::new(300, 300), Some((500, 400)), true);
    assert_eq!(explicit.client_size, (500, 400));
    assert_eq!(
        explicit.frame,
        client_rect_to_frame(RootPoint::new(300, 300), (500, 400), EXT)
    );
    assert_ne!(gravity_only.frame, explicit.frame);
}

#[test]
fn client_configure_position_only_moves_frame() {
    let r = client_configure(free());
    let p = plan_client_configure(&r, RootPoint::new(300, 300), None);
    assert_eq!(p.frame, RootRect::new(300, 269, 400, 300));
    assert_eq!(p.client_size, (400, 269));
}

#[test]
fn client_configure_far_offscreen_position_is_kept_visible() {
    // START 400x300 frame on WA 1920x1080, one output: 1/4 of each axis stays visible.
    pub(super) const ONE: &[RootRect] = &[WA];
    assert_eq!(
        configure_at(ONE, (-10000, 100), None),
        RootRect::new(-300, 100, 400, 300)
    );
    assert_eq!(
        configure_at(ONE, (10000, 100), None),
        RootRect::new(1820, 100, 400, 300)
    );
    assert_eq!(
        configure_at(ONE, (100, -10000), None),
        RootRect::new(100, -225, 400, 300)
    );
    assert_eq!(
        configure_at(ONE, (100, 10000), None),
        RootRect::new(100, 1005, 400, 300)
    );
}

#[test]
fn client_configure_clamps_on_second_output_with_negative_origin() {
    pub(super) const TWO: &[RootRect] = &[
        RootRect::new(-1920, 0, 1920, 1080),
        RootRect::new(0, 0, 1920, 1080),
    ];
    // Far left lands at the left edge of the negative-origin output, not the work area.
    assert_eq!(
        configure_at(TWO, (-10000, 100), None),
        RootRect::new(-2220, 100, 400, 300)
    );
    assert_eq!(
        configure_at(TWO, (10000, 100), None),
        RootRect::new(1820, 100, 400, 300)
    );
}

#[test]
fn client_configure_in_gap_moves_to_nearest_output() {
    pub(super) const GAP: &[RootRect] = &[
        RootRect::new(0, 0, 800, 600),
        RootRect::new(1000, 0, 800, 600),
    ];
    // 100x100 frame fully inside the 200px gap (x 800..1000): nearest output A moves it to x 775.
    let (ox, oy) = EXT.client_offset();
    let r = GeometryRequest {
        reason: GeometryReason::ClientConfigure,
        start: PlacementSnapshot {
            mode: PlacementMode::Floating,
            rect: RootRect::new(0, 0, 100, 100),
        },
        outputs: GAP,
        ..req(ResizeEdges::none(), (0, 0), free())
    };
    let p = plan_client_configure(&r, RootPoint::new(820 + ox, 10 + oy), None);
    assert_eq!(p.frame, RootRect::new(775, 10, 100, 100));
}

#[test]
fn client_configure_oversized_window_keeps_quarter_visible() {
    pub(super) const ONE: &[RootRect] = &[WA];
    let r = GeometryRequest {
        reason: GeometryReason::ClientConfigure,
        start: PlacementSnapshot {
            mode: PlacementMode::Floating,
            rect: RootRect::new(0, 0, 3000, 2000),
        },
        outputs: ONE,
        ..req(ResizeEdges::none(), (0, 0), free())
    };
    let p = plan_client_configure(&r, RootPoint::new(-10000, 10000), None);
    assert_eq!(p.frame, RootRect::new(-2250, 1080 - 500, 3000, 2000));
}

#[test]
fn client_configure_within_rule_is_unchanged() {
    pub(super) const ONE: &[RootRect] = &[WA];
    assert_eq!(
        configure_at(ONE, (300, 300), None),
        RootRect::new(300, 300, 400, 300)
    );
    assert_eq!(
        configure_at(ONE, (-300, 100), None),
        RootRect::new(-300, 100, 400, 300)
    );
}

#[test]
fn client_configure_snapped_is_not_clamped() {
    pub(super) const ONE: &[RootRect] = &[WA];
    let mut r = GeometryRequest {
        reason: GeometryReason::ClientConfigure,
        outputs: ONE,
        ..req(ResizeEdges::none(), (0, 0), free())
    };
    r.start.mode = PlacementMode::Snapped(SnapTarget::Left);
    let p = plan_client_configure(&r, RootPoint::new(-10000, 100), None);
    assert_eq!(p.frame.x, -10000 - EXT.client_offset().0);
}

#[test]
fn user_move_is_unchanged_by_configure_clamp() {
    // Same far-left target through the user drag path keeps the identical rect.
    assert_eq!(
        move_by((-10000, 0)),
        configure_at(&[WA], (-10000, 100), None)
    );
}

#[test]
fn client_configure_maximized_is_noncorrupting() {
    let mut r = GeometryRequest {
        reason: GeometryReason::ClientConfigure,
        ..req(ResizeEdges::none(), (0, 0), free())
    };
    r.start.mode = PlacementMode::Maximized;
    r.start.rect = RootRect::new(0, 0, 1920, 1080);
    let p = plan_client_configure(&r, RootPoint::new(5, 5), Some((10, 10)));
    assert_eq!(p.frame, RootRect::new(0, 0, 1920, 1080));
    assert!(p.noop);
    assert_eq!(p.keep_mode, PlacementMode::Maximized);
}

#[test]
fn client_configure_fullscreen_is_noncorrupting() {
    let mut r = GeometryRequest {
        reason: GeometryReason::ClientConfigure,
        ..req(ResizeEdges::none(), (0, 0), free())
    };
    r.start.mode = PlacementMode::Fullscreen;
    r.start.rect = RootRect::new(0, 0, 1920, 1080);
    let p = plan_client_configure(&r, RootPoint::new(5, 5), Some((10, 10)));
    assert_eq!(p.frame, RootRect::new(0, 0, 1920, 1080));
    assert!(p.noop);
}

#[test]
fn client_configure_clamps_requested_size_to_min_max_hints() {
    let client_size = |hints: ClientSizeHints, w: u32, h: u32| {
        let r = client_configure(hints);
        let p = plan_client_configure(&r, RootPoint::new(100, 100), Some((w, h)));
        frame_to_client_root(p.frame, EXT).size_u32()
    };
    let fixed = min_max((275, 116), (275, 116));
    assert_eq!(client_size(fixed, 100, 50), (275, 116));
    assert_eq!(client_size(fixed, 10, 10), (275, 116));
    assert_eq!(client_size(fixed, 400, 300), (275, 116));
    let ranged = min_max((200, 100), (300, 200));
    assert_eq!(client_size(ranged, 50, 50), (200, 100));
    assert_eq!(client_size(ranged, 500, 500), (300, 200));
    let incs = ClientSizeHints {
        base: Some((0, 0)),
        inc: Some((10, 20)),
        ..min_max((10, 20), (1000, 1000))
    };
    assert_eq!(client_size(incs, 105, 55), (100, 40));
}

#[test]
fn reason_variants_constructible() {
    for reason in [
        GeometryReason::InitialMap,
        GeometryReason::InteractiveMove,
        GeometryReason::InteractiveResize,
        GeometryReason::ClientConfigure,
    ] {
        let mut r = req(ResizeEdges::none(), (0, 0), free());
        r.reason = reason;
        let plan = GeometryPlan::of(&r, START);
        assert!(plan.noop);
        assert_eq!(plan.keep_mode, PlacementMode::Floating);
    }
}

#[test]
fn right_bottom_right_local_origin_constant() {
    for edges in [
        ResizeEdges::right(),
        ResizeEdges::bottom_right(),
        ResizeEdges::none(),
    ] {
        let r = req(edges, (37, 19), free());
        let p = plan_resize(&r);
        let local = frame_to_client_local(p.frame, EXT);
        assert_eq!((local.x, local.y), (0, 31));
    }
}

#[test]
fn border_is_outer_only_no_local_change() {
    let ext1 = FrameExtents::new(31, 0);
    let ext2 = FrameExtents::new(31, 5);
    assert_eq!(
        frame_to_client_local(START, ext1),
        frame_to_client_local(START, ext2)
    );
    assert_eq!(ext1.client_offset(), (0, 31));
    assert_eq!(ext2.client_offset(), (0, 31));
    assert_eq!(ext1.delta(), (0, 31));
    assert_eq!(ext2.delta(), (0, 31));
}

#[test]
fn client_offset_matches_old_good_model() {
    // Reparented client local x=0 y=titlebar; interior w=client w,
    // h=titlebar+client h.
    assert_eq!(EXT.client_offset(), (0, 31));
    assert_eq!(EXT.delta(), (0, 31));
    let local = frame_to_client_local(START, EXT);
    assert_eq!(local.w, START.w);
    assert_eq!(local.h, START.h - 31);
}

#[test]
fn synthetic_root_differs_from_local_at_nonzero_origin() {
    let frame = RootRect::new(400, 200, 400, 300);
    let local = frame_to_client_local(frame, EXT);
    let root = frame_to_client_root(frame, EXT);
    assert_eq!((local.x, local.y), (0, 31));
    assert_eq!(root.origin(), RootPoint::new(400, 231));
    assert_eq!(root.size_u32(), (400, 269));
}

#[test]
fn parse_all_hint_fields_used_by_geometry() {
    let mut values = vec![0u32; 18];
    values[0] |= HINT_P_MIN_SIZE
        | HINT_P_MAX_SIZE
        | HINT_P_RESIZE_INC
        | HINT_P_ASPECT
        | HINT_P_BASE_SIZE
        | HINT_P_WIN_GRAVITY;
    values[5] = 100;
    values[6] = 100;
    values[7] = 800;
    values[8] = 600;
    values[9] = 8;
    values[10] = 8;
    values[11] = 1;
    values[12] = 1;
    values[13] = 2;
    values[14] = 1;
    values[15] = 100;
    values[16] = 100;
    values[17] = 1;
    let hints = ClientSizeHints::parse(&values);
    assert_eq!(hints.min, Some((100, 100)));
    assert_eq!(hints.max, Some((800, 600)));
    assert_eq!(hints.inc, Some((8, 8)));
    assert_eq!(hints.base, Some((100, 100)));
    assert_eq!(hints.min_aspect, Some((1, 1)));
    assert_eq!(hints.max_aspect, Some((2, 1)));
    assert_eq!(hints.gravity, Some(1));
    let r = req(ResizeEdges::bottom_right(), (75, 75), hints);
    let p = plan_resize(&r);
    assert!(p.client_size.0 >= 100 && p.client_size.1 >= 100);
}
