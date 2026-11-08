use super::*;

#[test]
fn cursor_placement_respects_output_origin_and_panel_struts() {
    // Each work area comes from the selected physical output, never the root union.
    let cases = [
        (
            RootPoint::new(150, 170),
            RootRect::new(0, 36, 1920, 1044),
            RootRect::new(174, 194, 700, 531),
        ),
        (
            RootPoint::new(2150, 50),
            RootRect::new(1920, 0, 1280, 984),
            RootRect::new(2174, 74, 700, 531),
        ),
        (
            RootPoint::new(-1810, 100),
            RootRect::new(-1920, 0, 1920, 1030),
            RootRect::new(-1786, 124, 700, 531),
        ),
        (
            RootPoint::new(1700, 100),
            RootRect::new(56, 0, 2504, 1440),
            RootRect::new(1724, 124, 700, 531),
        ),
        // Bottom/right edge: fit entirely within the same hovered output.
        (
            RootPoint::new(3150, 950),
            RootRect::new(1920, 0, 1280, 984),
            RootRect::new(2500, 453, 700, 531),
        ),
        // Left panel and right panel work areas.
        (
            RootPoint::new(20, 70),
            RootRect::new(64, 0, 1856, 1080),
            RootRect::new(64, 94, 700, 531),
        ),
        (
            RootPoint::new(1880, 40),
            RootRect::new(0, 0, 1860, 1080),
            RootRect::new(1160, 64, 700, 531),
        ),
    ];
    for (pointer, work_area, expected) in cases {
        let request = GeometryRequest {
            reason: GeometryReason::InitialMap,
            ..req(ResizeEdges::none(), (0, 0), free())
        };
        assert_eq!(
            plan_initial_map_near_pointer(&request, pointer, work_area, (700, 500)).frame,
            expected,
            "pointer {pointer:?}, work area {work_area:?}",
        );
    }
}

#[test]
fn oversize_cursor_window_preserves_reachable_titlebar() {
    let request = GeometryRequest {
        reason: GeometryReason::InitialMap,
        ..req(ResizeEdges::none(), (0, 0), free())
    };
    let area = RootRect::new(-1920, 48, 900, 580);
    let plan =
        plan_initial_map_near_pointer(&request, RootPoint::new(-1100, 620), area, (1400, 900));
    assert_eq!(plan.frame, RootRect::new(-1920, 48, 1400, 931));
    assert_eq!(plan.client_size, (1400, 900));
}

#[test]
fn initial_cursor_policy_preserves_all_special_client_origins() {
    let origin = RootPoint::new(0, 0);
    assert!(can_place_near_pointer(false, true, false, free(), origin));
    assert!(!can_place_near_pointer(true, true, false, free(), origin));
    assert!(!can_place_near_pointer(false, false, false, free(), origin));
    assert!(!can_place_near_pointer(false, true, true, free(), origin));
    assert!(!can_place_near_pointer(
        false,
        true,
        false,
        free(),
        RootPoint::new(-1920, 0)
    ));
    assert!(!can_place_near_pointer(
        false,
        true,
        false,
        free(),
        RootPoint::new(200, 130)
    ));
    assert!(!can_place_near_pointer(
        false,
        true,
        false,
        ClientSizeHints {
            position_specified: true,
            ..free()
        },
        origin,
    ));
}

#[test]
fn pointer_placement_keeps_size_hints_and_adoption_unchanged() {
    let hints = min_max((350, 250), (600, 400));
    let request = GeometryRequest {
        reason: GeometryReason::InitialMap,
        ..req(ResizeEdges::none(), (0, 0), hints)
    };
    let plan = plan_initial_map_near_pointer(&request, RootPoint::new(100, 100), WA, (900, 600));
    assert_eq!(plan.client_size, (600, 400));
    assert_eq!(plan.frame, RootRect::new(124, 124, 600, 431));
    assert_eq!(
        plan_adopt(RootPoint::new(120, 160), (600, 400), EXT),
        RootRect::new(119, 128, 600, 431)
    );
}

#[test]
fn resize_right_fixes_left_edge() {
    let r = req(ResizeEdges::right(), (50, 0), free());
    let p = plan_resize(&r);
    assert_eq!(p.frame.x, 100);
    assert_eq!(p.frame.w, 450);
    assert_eq!(p.frame.h, 300);
    assert!(!p.noop);
}

#[test]
fn resize_left_fixes_right_edge() {
    let r = req(ResizeEdges::left(), (50, 0), free());
    let p = plan_resize(&r);
    // Fixed right = 500; constrained width 350 -> x = 150.
    assert_eq!(p.frame.x + p.frame.w, 500);
    assert_eq!(p.frame.w, 350);
    assert_eq!(p.frame.y, 100);
}

#[test]
fn resize_top_fixes_bottom_edge() {
    let r = req(ResizeEdges::top(), (0, 40), free());
    let p = plan_resize(&r);
    assert_eq!(p.frame.y + p.frame.h, 400);
    assert_eq!(p.frame.h, 260);
    assert_eq!(p.frame.x, 100);
}

#[test]
fn resize_bottom_fixes_top_edge() {
    let r = req(ResizeEdges::bottom(), (0, 40), free());
    let p = plan_resize(&r);
    assert_eq!(p.frame.y, 100);
    assert_eq!(p.frame.h, 340);
}

#[test]
fn resize_top_left_fixes_opposite_corner() {
    let r = req(ResizeEdges::top_left(), (20, 30), free());
    let p = plan_resize(&r);
    assert_eq!(p.frame.x + p.frame.w, 500);
    assert_eq!(p.frame.y + p.frame.h, 400);
    assert_eq!((p.frame.w, p.frame.h), (380, 270));
}

#[test]
fn resize_top_right_fixes_opposite_corner() {
    let r = req(ResizeEdges::top_right(), (20, 30), free());
    let p = plan_resize(&r);
    assert_eq!(p.frame.x, 100);
    assert_eq!(p.frame.y + p.frame.h, 400);
    assert_eq!((p.frame.w, p.frame.h), (420, 270));
}

#[test]
fn resize_bottom_left_fixes_opposite_corner() {
    let r = req(ResizeEdges::bottom_left(), (20, 30), free());
    let p = plan_resize(&r);
    assert_eq!(p.frame.x + p.frame.w, 500);
    assert_eq!(p.frame.y, 100);
    assert_eq!((p.frame.w, p.frame.h), (380, 330));
}

#[test]
fn resize_bottom_right_fixes_opposite_corner() {
    let r = req(ResizeEdges::bottom_right(), (20, 30), free());
    let p = plan_resize(&r);
    assert_eq!(p.frame.x, 100);
    assert_eq!(p.frame.y, 100);
    assert_eq!((p.frame.w, p.frame.h), (420, 330));
}

#[test]
fn move_is_start_plus_delta_and_never_incremental() {
    let mut r = req(ResizeEdges::none(), (60, 25), free());
    r.reason = GeometryReason::InteractiveMove;
    let p = plan_move(&r);
    assert_eq!(p.frame, RootRect::new(160, 125, 400, 300));
    // Same delta applied to the plan output must equal applying twice
    // the delta to the start (no drift), not delta-on-delta.
    let mut r2 = r;
    r2.start.rect = p.frame;
    let p2 = plan_move(&r2);
    assert_eq!(p2.frame, RootRect::new(220, 150, 400, 300));
    assert_eq!(p2.frame.x - START.x, 120);
}

#[test]
fn move_stops_at_three_quarters_outside_each_side() {
    // START 400x300 on WA 1920x1080: at most 3/4 may leave, so 100x75 stays visible.
    // Old work-area clamp would pin these at x 1520 / 0 and y 0 / 780.
    assert_eq!(move_by((5000, 0)), RootRect::new(1820, 100, 400, 300));
    assert_eq!(move_by((-5000, 0)), RootRect::new(-300, 100, 400, 300));
    assert_eq!(move_by((0, -5000)), RootRect::new(100, -225, 400, 300));
    assert_eq!(move_by((0, 5000)), RootRect::new(100, 1005, 400, 300));
}

#[test]
fn short_window_titlebar_stays_above_panel_reserve() {
    // 1600x900 screen, 44 px panel: work area bottom at 856. A 150 px tall window dragged
    // down used to park its titlebar (y 856..887) fully under the panel.
    let outs = [RootRect::new(0, 0, 1600, 900)];
    let work = RootRect::new(0, 0, 1600, 856);
    let mut r = req(ResizeEdges::none(), (0, 700), free());
    r.reason = GeometryReason::InteractiveMove;
    r.start.rect = RootRect::new(719, 285, 300, 150);
    r.work_area = work;
    r.outputs = &outs;
    assert_eq!(plan_move(&r).frame, RootRect::new(719, 825, 300, 150));
    // A titlebar already above the panel is untouched.
    r.pointer_delta = (0, 100);
    assert_eq!(plan_move(&r).frame, RootRect::new(719, 385, 300, 150));
}

#[test]
fn top_panel_reserve_keeps_titlebar_below_it() {
    // 1920x1080 output, 30 px top panel. A window dragged up must keep its titlebar at y 30.
    let outs = [RootRect::new(0, 0, 1920, 1080)];
    let mut r = req(ResizeEdges::none(), (0, -900), free());
    r.reason = GeometryReason::InteractiveMove;
    r.start.rect = RootRect::new(200, 300, 400, 300);
    r.work_area = RootRect::new(0, 30, 1920, 1050);
    r.outputs = &outs;
    assert_eq!(plan_move(&r).frame, RootRect::new(200, 30, 400, 300));
    // Without a top reserve the drag may pass the top edge; keep_on_outputs keeps a quarter.
    r.work_area = RootRect::new(0, 0, 1920, 1080);
    assert_eq!(plan_move(&r).frame, RootRect::new(200, -225, 400, 300));
}

#[test]
fn side_panel_reserves_keep_a_quarter_of_the_titlebar_in_work_area() {
    // 1920x1080 output with a 64 px panel on the left, then on the right.
    let outs = [RootRect::new(0, 0, 1920, 1080)];
    let mut r = req(ResizeEdges::none(), (-1000, 0), free());
    r.reason = GeometryReason::InteractiveMove;
    r.start.rect = RootRect::new(300, 200, 400, 300);
    r.outputs = &outs;
    r.work_area = RootRect::new(64, 0, 1856, 1080);
    // visible strip >= quarter of 400 (100 px): x = 64 + 100 - 400 = -236.
    assert_eq!(plan_move(&r).frame, RootRect::new(-236, 200, 400, 300));
    r.pointer_delta = (2000, 0);
    r.work_area = RootRect::new(0, 0, 1856, 1080);
    // visible strip >= 100 px inside the work area: x <= 1856 - 100 = 1756.
    assert_eq!(plan_move(&r).frame, RootRect::new(1756, 200, 400, 300));
}

#[test]
fn secondary_monitor_work_area_is_not_a_reserve_against_its_neighbour() {
    // Two 1920x1080 outputs side by side; the window lives on the second one.
    let outs = [
        RootRect::new(0, 0, 1920, 1080),
        RootRect::new(1920, 0, 1920, 1080),
    ];
    let mut r = req(ResizeEdges::none(), (-500, 0), free());
    r.reason = GeometryReason::InteractiveMove;
    r.start.rect = RootRect::new(2000, 200, 400, 300);
    r.outputs = &outs;
    r.work_area = RootRect::new(1920, 0, 1920, 1080);
    // Dragged onto the first monitor: the second output's work area must not clamp it.
    assert_eq!(plan_move(&r).frame, RootRect::new(1500, 200, 400, 300));
}

#[test]
fn move_over_taskbar_band_is_allowed() {
    // A 24px taskbar at the bottom is not excluded from the drag bound.
    assert_eq!(move_by((0, 800)), RootRect::new(100, 900, 400, 300));
}

#[test]
fn oversized_window_keeps_quarter_visible_and_never_wraps() {
    let mut r = req(ResizeEdges::none(), (-5000, -5000), free());
    r.reason = GeometryReason::InteractiveMove;
    r.start.rect = RootRect::new(0, 0, 3000, 2000);
    r.outputs = OUTS;
    // min visible = 750 x 500 of the 3000 x 2000 window.
    assert_eq!(plan_move(&r).frame, RootRect::new(-2250, -1500, 3000, 2000));
}

#[test]
fn negative_origin_output_bounds_move_without_unsigned_wrap() {
    let left = RootRect::new(-1920, 0, 1920, 1080);
    let mut r = req(ResizeEdges::none(), (-5000, 0), free());
    r.reason = GeometryReason::InteractiveMove;
    let outs = [left, WA];
    r.outputs = &outs;
    assert_eq!(plan_move(&r).frame, RootRect::new(-2220, 100, 400, 300));
}

#[test]
fn restored_rect_slides_fully_inside_work_area() {
    // 900px screen with a 40px taskbar: work area y 0..860.
    let wa = RootRect::new(0, 0, 1600, 860);
    // The reported case: a 540px restore at y=789 must end inside the work area.
    assert_eq!(
        fit_restore_rect(RootRect::new(100, 789, 800, 540), wa),
        RootRect::new(100, 320, 800, 540)
    );
    // Off the top/left edges slides back to the origin.
    assert_eq!(
        fit_restore_rect(RootRect::new(-50, -20, 300, 200), wa),
        RootRect::new(0, 0, 300, 200)
    );
    // Larger than the work area: size is capped, then placed at the origin.
    assert_eq!(
        fit_restore_rect(RootRect::new(1200, 700, 2000, 1000), wa),
        RootRect::new(0, 0, 1600, 860)
    );
    // Already inside: untouched.
    let inside = RootRect::new(200, 150, 400, 300);
    assert_eq!(fit_restore_rect(inside, wa), inside);
}

#[test]
fn window_parked_in_gap_of_output_union_moves_to_nearest_output() {
    let a = RootRect::new(0, 0, 800, 600);
    let b = RootRect::new(1000, 0, 800, 600);
    // 100 px window fully inside the 200 px gap: no output keeps 1/4 visible.
    // Nearest is A (touching at x = 800): moved to x = 775 so 25 px stay on A.
    assert_eq!(
        keep_on_outputs(RootRect::new(800, 10, 100, 100), &[a, b], WA),
        RootRect::new(775, 10, 100, 100)
    );
    // A window straddling both outputs with 1/4 on one of them is left alone.
    assert_eq!(
        keep_on_outputs(RootRect::new(600, 10, 400, 300), &[a, b], WA),
        RootRect::new(600, 10, 400, 300)
    );
}

#[test]
fn resize_top_left_past_work_area_stops_at_work_area_edge() {
    let mut r = req(ResizeEdges::top_left(), (-5000, -5000), free());
    r.outputs = OUTS;
    let p = plan_resize(&r);
    // Fixed bottom-right corner is (500, 400); left and top stop at the work area's 0,0 edge.
    assert_eq!(p.frame, RootRect::new(0, 0, 500, 400));
    assert_eq!(p.frame.x + p.frame.w, 500);
    assert_eq!(p.frame.y + p.frame.h, 400);
}

#[test]
fn resize_stops_at_work_area_edge_not_output_edge() {
    // 56 px taskbar reserve: the work area ends at y=1024 while the output ends at 1080.
    let mut r = req(ResizeEdges::bottom_right(), (5000, 5000), free());
    r.work_area = RootRect::new(0, 0, 1920, 1024);
    let outputs = [RootRect::new(0, 0, 1920, 1080)];
    r.outputs = &outputs;
    let p = plan_resize(&r);
    // Fixed top-left corner is (100, 100); the frame fills the work area to its right/bottom edges.
    assert_eq!(p.frame, RootRect::new(100, 100, 1820, 924));
    assert!(p.frame.y + p.frame.h <= 1024);
}

#[test]
fn resize_fixed_edge_outside_work_area_is_not_capped() {
    // Fixed left edge already right of the work area: no span to cap against, so the
    // raw resize result stands (the output-bounds rule still applies).
    let mut r = req(ResizeEdges::right(), (100, 0), free());
    r.start.rect = RootRect::new(2000, 100, 400, 300);
    r.work_area = RootRect::new(0, 0, 1920, 1024);
    let outputs = [RootRect::new(0, 0, 1920, 1080)];
    r.outputs = &outputs;
    let p = plan_resize(&r);
    assert_eq!(p.frame.x, 2000);
    assert!(p.frame.w > 0);
}

#[test]
fn resize_bottom_overhanging_work_area_does_not_shrink_on_zero_motion() {
    // Frame already extends 20 px below the work area: the work-area cap must stop growth,
    // never pull the frame back to the cap on the first motion (251 -> 223 regression).
    let mut r = req(ResizeEdges::bottom(), (0, 0), free());
    r.start.rect = RootRect::new(100, 800, 400, 300);
    r.outputs = OUTS;
    let p = plan_resize(&r);
    assert_eq!(p.frame, RootRect::new(100, 800, 400, 300));
}

#[test]
fn resize_bottom_overhanging_work_area_does_not_shrink_on_outward_motion() {
    let mut r = req(ResizeEdges::bottom(), (0, 15), free());
    r.start.rect = RootRect::new(100, 800, 400, 300);
    r.outputs = OUTS;
    let p = plan_resize(&r);
    assert_eq!(p.frame.h, 300);
    assert_eq!(p.frame.y, 800);
}

#[test]
fn resize_bottom_overhanging_work_area_still_shrinks_inward() {
    let mut r = req(ResizeEdges::bottom(), (0, -40), free());
    r.start.rect = RootRect::new(100, 800, 400, 300);
    r.outputs = OUTS;
    let p = plan_resize(&r);
    assert_eq!(p.frame.h, 260);
}

#[test]
fn resize_moving_edge_stops_at_work_area_edge() {
    let mut r = req(ResizeEdges::right(), (5000, 0), free());
    r.start.rect = RootRect::new(1500, 100, 400, 300);
    r.outputs = OUTS;
    let p = plan_resize(&r);
    // Left edge fixed at 1500; the right edge stops at the 1920 px work-area edge.
    assert_eq!(p.frame, RootRect::new(1500, 100, 420, 300));
}

#[test]
fn resize_min_floor_reconstructs_dragged_edge() {
    // Frame delta = (0, 31); min client 200x150 -> min frame 200x181.
    let r = req(
        ResizeEdges::left(),
        (300, 0),
        min_max((200, 150), (2000, 2000)),
    );
    let p = plan_resize(&r);
    assert_eq!(p.frame.w, 200);
    assert_eq!(p.frame.x + p.frame.w, 500);
}

#[test]
fn resize_max_ceiling_reconstructs_dragged_edge() {
    let r = req(
        ResizeEdges::bottom_right(),
        (2000, 2000),
        min_max((100, 100), (500, 400)),
    );
    let p = plan_resize(&r);
    // Max client 500x400 -> frame 500x431, anchored at fixed top-left.
    assert_eq!((p.frame.w, p.frame.h), (500, 431));
    assert_eq!((p.frame.x, p.frame.y), (100, 100));
}

#[test]
fn resize_increment_snaps_in_8px_steps() {
    let hints = ClientSizeHints {
        base: Some((100, 100)),
        inc: Some((8, 8)),
        ..ClientSizeHints::default()
    };
    // Client width 400+50=450 raw -> base 100 + 43*8 = 444 -> frame 444.
    let r = req(ResizeEdges::right(), (50, 0), hints);
    let p = plan_resize(&r);
    assert_eq!(p.client_size.0 % 8, 100 % 8);
    assert_eq!(p.frame.w, 444);
}

#[test]
fn resize_aspect_shrinks_width() {
    let hints = ClientSizeHints {
        max_aspect: Some((2, 1)),
        ..ClientSizeHints::default()
    };
    // Drag far wider than 2:1; width must shrink to 2*h.
    let r = req(ResizeEdges::right(), (1500, 0), hints);
    let p = plan_resize(&r);
    let (cw, ch) = p.client_size;
    assert!(u64::from(cw) <= 2u64 * u64::from(ch));
    assert_eq!(p.frame.x, 100);
}

#[test]
fn resize_fixed_size_window_is_noop() {
    let hints = ClientSizeHints {
        min: Some((400, 269)),
        max: Some((400, 269)),
        resizable: false,
        ..ClientSizeHints::default()
    };
    let r = req(ResizeEdges::bottom_right(), (80, 60), hints);
    let p = plan_resize(&r);
    assert_eq!(p.frame, START);
    assert!(p.noop);
}

#[test]
fn gravity_static_keeps_origin() {
    let o = RootPoint::new(200, 150);
    let n = gravity_adjust_origin(o, (400, 300), (200, 100), WinGravity::Static);
    assert_eq!(n, o);
}

#[test]
fn gravity_northwest_keeps_origin() {
    let o = RootPoint::new(200, 150);
    let n = gravity_adjust_origin(o, (400, 300), (200, 100), WinGravity::NorthWest);
    assert_eq!(n, o);
}

#[test]
fn gravity_southeast_moves_origin_by_full_delta() {
    let o = RootPoint::new(200, 150);
    let n = gravity_adjust_origin(o, (400, 300), (200, 100), WinGravity::SouthEast);
    // SE corner fixed: 200+400=600 -> x = 600-200 = 400; 150+300=450 -> y = 350.
    assert_eq!(n, RootPoint::new(400, 350));
}

#[test]
fn gravity_north_centers_horizontally() {
    let o = RootPoint::new(200, 150);
    let n = gravity_adjust_origin(o, (400, 300), (200, 100), WinGravity::North);
    // Top-center fixed: old cx = 400 -> x = 400-100 = 300; top anchored.
    assert_eq!(n, RootPoint::new(300, 150));
}

#[test]
fn gravity_south_anchors_bottom() {
    let o = RootPoint::new(200, 150);
    let n = gravity_adjust_origin(o, (400, 300), (200, 300), WinGravity::South);
    // Bottom-center fixed: old cx = 400 -> x = 300; bottom 450 unchanged.
    assert_eq!(n, RootPoint::new(300, 150));
    let n2 = gravity_adjust_origin(o, (400, 300), (400, 100), WinGravity::South);
    // Bottom fixed: 150+300=450 -> y = 450-100 = 350.
    assert_eq!(n2, RootPoint::new(200, 350));
}

#[test]
fn gravity_center_and_east_west() {
    let o = RootPoint::new(200, 150);
    assert_eq!(
        gravity_adjust_origin(o, (400, 300), (200, 100), WinGravity::Center),
        RootPoint::new(300, 250)
    );
    assert_eq!(
        gravity_adjust_origin(o, (400, 300), (200, 300), WinGravity::West),
        RootPoint::new(200, 150)
    );
    assert_eq!(
        gravity_adjust_origin(o, (400, 300), (200, 300), WinGravity::East),
        RootPoint::new(400, 150)
    );
}

#[test]
fn gravity_parses_from_size_hints() {
    let mut values = vec![0u32; 18];
    values[0] |= HINT_P_WIN_GRAVITY;
    values[17] = 9; // SouthEast
    let hints = ClientSizeHints::parse(&values);
    assert_eq!(WinGravity::of(&hints), WinGravity::SouthEast);
    assert_eq!(
        WinGravity::of(&ClientSizeHints::default()),
        WinGravity::NorthWest
    );
}

#[test]
fn move_frame_keeps_client_local() {
    let mut r = req(ResizeEdges::none(), (300, 100), free());
    r.reason = GeometryReason::InteractiveMove;
    let p = plan_move(&r);
    assert_eq!(p.frame, RootRect::new(400, 200, 400, 300));
    assert_eq!(
        frame_to_client_local(p.frame, EXT),
        ClientLocalRect::new(0, 31, 400, 269)
    );
}
