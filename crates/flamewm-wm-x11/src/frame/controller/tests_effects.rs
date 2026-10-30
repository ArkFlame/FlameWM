use super::super::geometry::FrameExtents;
use super::tests::*;
use super::*;
use flamewm_api::{Rect as CoreRect, Size as CoreSize};
use flamewm_window_core::{SnapTarget as CoreSnapTarget, snap_geometry};

#[test]
fn effect_order_manage_chrome_paint_retarget_first_flush_last() {
    use super::super::chrome::{effects_for, plan_scene};
    use super::super::resources::{CONTROL_ORDER, FrameResources, RESIZE_ORDER};
    let res = FrameResources::new(7, 70, 71, [72, 73, 74], [80, 81, 82, 83, 84, 85, 86, 87]);
    let children = res.children();
    assert_eq!(children.len(), 12);
    assert_eq!(children[8], 71);
    assert_eq!(&children[9..12], &[72, 73, 74]);
    assert_eq!(RESIZE_ORDER.len(), 8);
    assert_eq!(CONTROL_ORDER.len(), 3);
    let scene = plan_scene(
        400,
        300,
        "app",
        true,
        None,
        None,
        false,
        false,
        true,
        &flamewm_skin::recipes::decoration::ChromeTokens::default(),
    );
    let fx = effects_for(70, &scene);
    let kind = |e: &super::super::chrome::ChromeEffect| match e {
        super::super::chrome::ChromeEffect::Retarget { .. } => 0,
        super::super::chrome::ChromeEffect::FillTitlebar { .. } => 1,
        super::super::chrome::ChromeEffect::DrawTitle { .. } => 2,
        super::super::chrome::ChromeEffect::BlitIcon => 3,
        super::super::chrome::ChromeEffect::BlitControls { .. } => 4,
        super::super::chrome::ChromeEffect::ApplyShape { .. } => 5,
        super::super::chrome::ChromeEffect::Flush => 6,
    };
    let order: Vec<i32> = fx.iter().map(kind).collect();
    assert_eq!(*order.first().expect("nonempty"), 0);
    assert_eq!(*order.last().expect("nonempty"), 6);
    let mut sorted = order.clone();
    sorted.sort_unstable();
    assert_eq!(
        order, sorted,
        "chrome effects must be monotonically ordered"
    );
}

#[test]
fn effect_order_move_motion_has_no_layout_or_paint() {
    let s1 = pressed(&floating_state(), 1, 150, 120);
    let (_, e2) = reduce(&s1, motion(200, 170));
    assert_eq!(e2.len(), 3);
    assert!(matches!(
        e2[0],
        FrameEffect::MoveFrameRoot { x: 150, y: 150 }
    ));
    assert!(!has_any_frame_root(&e2));
    // Client local origin is (0,titlebar); synthetic root notify at frame offset.
    assert!(matches!(
        e2[1],
        FrameEffect::NotifyClientRoot {
            x: 150,
            y: 181,
            w: 400,
            h: 269
        }
    ));
    assert_eq!(count_local(&e2), 0);
    assert!(!e2.contains(&FrameEffect::LayoutInput));
    assert!(matches!(e2[2], FrameEffect::PaintChrome));
    assert!(!e2.contains(&FrameEffect::ApplyShape));
    assert!(!e2.contains(&FrameEffect::Ungrab));
}

#[test]
fn effect_order_resize_release_has_frame_client_layout_chrome() {
    let s1 = pressed(&floating_state(), 2, 500, 400);
    let (s2, e_motion) = reduce(&s1, motion(550, 460));
    // Resize motion carries frame + local + root notify.
    assert_eq!(count_local(&e_motion), 1);
    assert!(
        e_motion
            .iter()
            .any(|e| matches!(e, FrameEffect::ConfigureClientLocal { x: 0, y: 31, .. }))
    );
    let (_, e3) = reduce(&s2, release(550, 460));
    assert!(e3.contains(&FrameEffect::LayoutInput));
    assert!(e3.contains(&FrameEffect::PaintChrome));
    assert!(e3.contains(&FrameEffect::ApplyShape));
    assert!(
        e3.iter()
            .any(|e| matches!(e, FrameEffect::ConfigureFrameRoot { .. }))
    );
    assert!(
        e3.iter()
            .any(|e| matches!(e, FrameEffect::ConfigureClientLocal { .. }))
    );
    let ungrab = e3
        .iter()
        .position(|e| *e == FrameEffect::Ungrab)
        .expect("ungrab");
    let layout = e3
        .iter()
        .position(|e| *e == FrameEffect::LayoutInput)
        .expect("layout");
    let paint = e3
        .iter()
        .position(|e| *e == FrameEffect::PaintChrome)
        .expect("paint");
    let frame = e3
        .iter()
        .position(|e| matches!(e, FrameEffect::ConfigureFrameRoot { .. }))
        .expect("frame");
    assert_eq!(ungrab, e3.len() - 1, "geometry precedes Ungrab on release");
    assert!(frame < layout && layout < paint && paint < ungrab);
}

#[test]
fn effect_order_release_geometry_precedes_ungrab() {
    let s1 = pressed(&floating_state(), 1, 150, 120);
    let (s2, _) = reduce(&s1, motion(200, 170));
    let (s3, e3) = reduce(&s2, release(200, 170));
    assert_eq!(s3.placement.current, RootRect::new(150, 150, 400, 300));
    assert_eq!(*e3.last().expect("nonempty"), FrameEffect::Ungrab);
    let tail_move = e3
        .iter()
        .find_map(|e| match *e {
            FrameEffect::MoveFrameRoot { x, y } => Some((x, y)),
            _ => None,
        })
        .expect("final move geometry");
    assert_eq!(tail_move, (150, 150));
    assert!(!has_any_frame_root(&e3));
    let frame_pos = e3
        .iter()
        .position(|e| matches!(e, FrameEffect::MoveFrameRoot { .. }))
        .expect("frame");
    let ungrab_pos = e3
        .iter()
        .position(|e| *e == FrameEffect::Ungrab)
        .expect("ungrab");
    assert!(frame_pos < ungrab_pos);
}

#[test]
fn move_thousand_samples_zero_client_local() {
    let s1 = pressed(&floating_state(), 1, 150, 120);
    let mut total_local = 0;
    let mut total_configure_root = 0;
    let mut total_layout = 0;
    let mut total_paint = 0;
    let mut total_shape = 0;
    let mut total_move_root = 0;
    let mut total_notify = 0;
    let mut total_noop = 0;
    for i in 0..1000 {
        let dx = (i % 100) - 50;
        let dy = ((i * 7) % 100) - 50;
        let (_, e) = reduce(&s1, motion(150 + dx, 120 + dy));
        // Move motion emits either Noop (zero/non-plan delta) or exactly
        // [MoveFrameRoot, NotifyClientRoot, PaintChrome]; nothing else is representable.
        if e == vec![FrameEffect::Noop] {
            total_noop += 1;
            continue;
        }
        assert_eq!(
            e.len(),
            3,
            "move motion must emit move+notify+paint (sample {i})"
        );
        assert!(
            matches!(e[2], FrameEffect::PaintChrome),
            "move[2] must be PaintChrome (sample {i})"
        );
        assert!(
            matches!(e[0], FrameEffect::MoveFrameRoot { .. }),
            "move[0] must be MoveFrameRoot (sample {i})"
        );
        assert!(
            matches!(e[1], FrameEffect::NotifyClientRoot { .. }),
            "move[1] must be NotifyClientRoot (sample {i})"
        );
        for fx in &e {
            match fx {
                FrameEffect::ConfigureClientLocal { .. } => total_local += 1,
                FrameEffect::ConfigureFrameRoot { .. } => total_configure_root += 1,
                FrameEffect::LayoutInput => total_layout += 1,
                FrameEffect::PaintChrome => total_paint += 1,
                FrameEffect::ApplyShape => total_shape += 1,
                FrameEffect::MoveFrameRoot { .. } => total_move_root += 1,
                FrameEffect::NotifyClientRoot { .. } => total_notify += 1,
                _ => panic!("unexpected move effect {fx:?} (sample {i})"),
            }
        }
    }
    assert_eq!(total_local, 0);
    assert_eq!(total_configure_root, 0);
    assert_eq!(total_layout, 0);
    assert_eq!(total_paint, total_move_root);
    assert_eq!(total_shape, 0);
    assert_eq!(total_move_root, total_notify);
    assert!(total_move_root > 0, "expected non-noop move samples");
    assert!(total_noop > 0, "expected at least one noop sample");
}

#[test]
fn post_activation_move_repaints_chrome_after_frame_move() {
    let s1 = pressed(&floating_state(), 1, 150, 120);
    let (s2, _) = reduce(&s1, motion(200, 170));
    // Post-activation step whose frame position changes: titlebar must
    // repaint live, ordered after the frame move.
    let (s3, e3) = reduce(&s2, motion(230, 190));
    assert_eq!(s3.placement.current, RootRect::new(180, 170, 400, 300));
    let move_pos = e3
        .iter()
        .position(|e| matches!(e, FrameEffect::MoveFrameRoot { .. }))
        .expect("move geometry");
    let paint_pos = e3
        .iter()
        .position(|e| *e == FrameEffect::PaintChrome)
        .expect("live move must repaint titlebar");
    assert!(
        move_pos < paint_pos,
        "PaintChrome must follow MoveFrameRoot"
    );
    // Repeated pointer position leaves the frame unchanged: no repaint.
    let (_, e4) = reduce(&s3, motion(230, 190));
    assert!(!e4.contains(&FrameEffect::PaintChrome));
}

#[test]
fn resize_motion_paints_after_native_resize_not_at_the_old_size() {
    let s1 = pressed(&floating_state(), 2, 500, 400);
    let (_, effects) = reduce(&s1, motion(550, 460));
    let at =
        |predicate: fn(&FrameEffect) -> bool| effects.iter().position(predicate).expect("effect");
    let frame = at(|e| matches!(e, FrameEffect::ConfigureFrameRoot { .. }));
    let client = at(|e| matches!(e, FrameEffect::ConfigureClientLocal { .. }));
    let paint = at(|e| matches!(e, FrameEffect::PaintChrome));
    let shape = at(|e| matches!(e, FrameEffect::ApplyShape));
    assert!(frame < client && client < paint && paint < shape);
}

#[test]
fn resize_still_uses_full_configure_frame_root() {
    let s1 = pressed(&floating_state(), 2, 500, 400);
    let (_, e) = reduce(&s1, motion(550, 460));
    assert!(
        e.iter()
            .any(|fx| matches!(fx, FrameEffect::ConfigureFrameRoot { .. })),
        "resize motion must keep full ConfigureFrameRoot"
    );
    assert_eq!(count_local(&e), 1);
}

#[test]
fn move_frame_at_origin_client_local_and_root() {
    // Start (100,100) + delta (300,100) => frame (400,200); client local
    // stays (0,titlebar), synthetic root = (400,231) size (400,269).
    let s1 = pressed(&floating_state(), 1, 150, 120);
    let (s2, e2) = reduce(&s1, motion(450, 220));
    assert_eq!(s2.placement.current, RootRect::new(400, 200, 400, 300));
    assert!(matches!(
        e2[1],
        FrameEffect::NotifyClientRoot {
            x: 400,
            y: 231,
            w: 400,
            h: 269
        }
    ));
}

#[test]
fn resize_right_and_bottom_right_local_origin_constant() {
    let s1 = pressed(&floating_state(), 2, 500, 400);
    let (_, e) = reduce(&s1, motion(550, 460));
    assert!(
        e.iter()
            .any(|fx| matches!(fx, FrameEffect::ConfigureClientLocal { x: 0, y: 31, .. }))
    );
}

#[test]
fn fullscreen_uses_screen_rect_and_restores_exact_snapshot() {
    let mut s0 = floating_state();
    s0.work_area = RootRect::new(0, 24, 1920, 1056);
    s0.screen_rect = RootRect::new(0, 0, 1920, 1080);
    let (s1, e1) = reduce(&s0, FrameEvent::ToggleFullscreen);
    assert_eq!(s1.placement.mode, PlacementMode::Fullscreen);
    assert_eq!(s1.placement.current, RootRect::new(0, 0, 1920, 1080));
    assert!(has_frame(&e1, 0, 0));
    let (s2, _) = reduce(&s0, FrameEvent::ToggleMax);
    assert_eq!(s2.placement.current, RootRect::new(0, 24, 1920, 1056));
    let (s3, _) = reduce(&s1, FrameEvent::ToggleFullscreen);
    assert_eq!(s3.placement.mode, PlacementMode::Floating);
    assert_eq!(s3.placement.current, START);
}

#[test]
fn eight_edges_hold_opposite_anchor() {
    use super::super::geometry::{GeometryReason, GeometryRequest, plan_resize};
    use super::super::model::PlacementSnapshot;
    let edges = [
        ResizeEdges::left(),
        ResizeEdges::right(),
        ResizeEdges::top(),
        ResizeEdges::bottom(),
        ResizeEdges::top_left(),
        ResizeEdges::top_right(),
        ResizeEdges::bottom_left(),
        ResizeEdges::bottom_right(),
    ];
    for e in edges {
        let r = GeometryRequest {
            reason: GeometryReason::InteractiveResize,
            start: PlacementSnapshot {
                mode: PlacementMode::Floating,
                rect: START,
            },
            pointer_delta: (40, 30),
            edges: e,
            work_area: WA,
            outputs: &[],
            hints: ClientSizeHints::default(),
            frame_extents: EXT,
        };
        let p = plan_resize(&r);
        if !e.left {
            assert_eq!(p.frame.x, START.x, "left anchor {e:?}");
        }
        if !e.right && !e.left {
            assert_eq!(p.frame.w, START.w, "width {e:?}");
        }
        if !e.top {
            assert_eq!(p.frame.y, START.y, "top anchor {e:?}");
        }
    }
}

#[test]
fn client_configure_request_frame_size_as_intent_grows_frame() {
    // M01 intent: ClientConfigureRequest carries client-window geometry
    // only. A frame-sized observation ((400,300) = outer incl. titlebar)
    // fed as client intent grows the frame by the titlebar: (100,100,
    // 400,300) -> (100,100,400,331). ConfigureNotify observations must
    // therefore never be constructed into this event by the live owner.
    let s0 = floating_state();
    let (s1, _) = reduce(
        &s0,
        FrameEvent::ClientConfigureRequest {
            root_origin: RootPoint::new(100, 131),
            size: Some((400, 300)),
        },
    );
    assert_eq!(s1.placement.current, RootRect::new(100, 100, 400, 331));
}

#[test]
fn border_does_not_change_local_geometry() {
    use super::super::geometry::frame_to_client_local;
    let thin = FrameExtents::new(31, 0);
    let thick = FrameExtents::new(31, 1);
    assert_eq!(
        frame_to_client_local(START, thin),
        frame_to_client_local(START, thick)
    );
}

// CONTRACT-REGRESSION T01: threshold keeps a press below activation inert.
// TRIGGER: title press followed by a three-pixel motion.
// OBSERVABLE RESULT: placement and motion effects remain unchanged.
// UNCOVERED GAP: current reducer activates every title press immediately.
// FAILURE MUTATION: remove the four-pixel activation threshold.
// REPRESENTATIVE CASE: ordinary floating title drag.
#[test]
fn t01_title_motion_below_four_pixels_is_pending() {
    let s0 = floating_state();
    let (s1, _) = reduce(&s0, press(1, 150, 120));
    let (s2, _effects) = reduce(&s1, motion(153, 120));
    assert!(matches!(s1.session, InteractionSession::Move(_)));
    assert_eq!(s1.placement, s0.placement);
    assert_eq!(s2.placement.mode, PlacementMode::Floating);
    assert_eq!(s2.placement.current, s0.placement.current);
    assert_eq!(
        (s2.placement.current.w, s2.placement.current.h),
        (s0.placement.current.w, s0.placement.current.h)
    );
    assert_eq!(s2.placement.floating_restore, s0.placement.floating_restore);
}

// CONTRACT-REGRESSION T02: activation occurs at the four-pixel boundary.
// TRIGGER: title press followed by exactly four pixels of motion.
// OBSERVABLE RESULT: max drag activates restore on the exact boundary.
// UNCOVERED GAP: threshold and press snapshot are not represented together.
// FAILURE MUTATION: change the boundary to three pixels or use incremental state.
// REPRESENTATIVE CASE: exact threshold maximized drag-away.
#[test]
fn t02_title_motion_at_four_pixels_activates_from_snapshot() {
    let s0 = maximized_state();
    let (s1, _) = reduce(&s0, press(1, 960, 20));
    let (s2, effects) = reduce(&s1, motion(964, 20));
    assert_eq!(s2.placement.mode, PlacementMode::Floating);
    assert_eq!(
        (s2.placement.current.w, s2.placement.current.h),
        (
            s0.placement.floating_restore.w,
            s0.placement.floating_restore.h
        )
    );
    assert!(has_full_configure(&effects));
}

// CONTRACT-REGRESSION T03: maximized press keeps authoritative placement.
// TRIGGER: title press while maximized.
// OBSERVABLE RESULT: current remains work-area/maximized; pending snapshot is Floating.
// UNCOVERED GAP: current press labels the restore snapshot Maximized.
// FAILURE MUTATION: store the current mode instead of Floating in MoveSession.
// REPRESENTATIVE CASE: maximize then drag.
#[test]
fn t03_maximized_press_has_floating_pending_snapshot() {
    let s0 = maximized_state();
    let (s1, effects) = reduce(&s0, press(1, 960, 20));
    assert_eq!(s1.placement, s0.placement);
    assert!(effects.contains(&FrameEffect::Grab {
        window: 71,
        cursor: flamewm_render_core::CursorKind::Move,
    }));
    match s1.session {
        InteractionSession::Move(session) => {
            assert_eq!(session.start_placement.mode, PlacementMode::Floating);
            assert_eq!(
                (
                    session.start_placement.rect.w,
                    session.start_placement.rect.h
                ),
                (
                    s0.placement.floating_restore.w,
                    s0.placement.floating_restore.h
                )
            );
            assert_ne!(session.start_placement.rect, s0.placement.current);
        }
        other => panic!("expected pending move session, got {other:?}"),
    }
}

// CONTRACT-REGRESSION T04: snapped press also records a Floating restore.
// TRIGGER: title press while snapped left.
// OBSERVABLE RESULT: snapped current rect stays authoritative until activation.
// UNCOVERED GAP: current snapped branch snapshots the snapped mode/rect.
// FAILURE MUTATION: treat snapped press as an ordinary move immediately.
// REPRESENTATIVE CASE: snapped title drag-away.
#[test]
fn t04_snapped_press_has_floating_pending_snapshot() {
    let s0 = snapped_state();
    let (s1, _) = reduce(&s0, press(1, 500, 20));
    assert_eq!(s1.placement, s0.placement);
    match s1.session {
        InteractionSession::Move(session) => {
            assert_eq!(session.start_placement.mode, PlacementMode::Floating);
            assert_eq!(
                (
                    session.start_placement.rect.w,
                    session.start_placement.rect.h
                ),
                (
                    s0.placement.floating_restore.w,
                    s0.placement.floating_restore.h
                )
            );
            assert_ne!(session.start_placement.rect, s0.placement.current);
        }
        other => panic!("expected pending move session, got {other:?}"),
    }
}

// CONTRACT-REGRESSION T05: first maximized drag-away is a full restore.
// TRIGGER: maximized title press, then motion beyond four pixels.
// OBSERVABLE RESULT: Floating/restore size plus full configure effects.
// UNCOVERED GAP: current motion keeps Maximized and emits position-only effects.
// FAILURE MUTATION: replace restore configure with MoveFrameRoot only.
// REPRESENTATIVE CASE: maximized window first activated motion.
#[test]
fn t05_maximized_first_drag_away_restores_with_full_configure() {
    let s0 = maximized_state();
    let (s1, _) = reduce(&s0, press(1, 960, 20));
    let (s2, effects) = reduce(&s1, motion(965, 25));
    assert_eq!(s2.placement.mode, PlacementMode::Floating);
    assert_eq!(
        (s2.placement.current.w, s2.placement.current.h),
        (
            s0.placement.floating_restore.w,
            s0.placement.floating_restore.h
        )
    );
    assert!(has_full_configure(&effects));
    assert!(effects.contains(&FrameEffect::LayoutInput));
    assert!(effects.contains(&FrameEffect::PaintChrome));
    assert!(effects.contains(&FrameEffect::ApplyShape));
    assert!(!effects.contains(&FrameEffect::Ungrab));
}

// CONTRACT-REGRESSION T06: snapped drag-away uses the same full restore path.
// TRIGGER: snapped title press, then motion beyond four pixels.
// OBSERVABLE RESULT: Floating restore dimensions and full configure effects.
// UNCOVERED GAP: current snapped motion resizes/moves while retaining Snapped.
// FAILURE MUTATION: omit the one-time snapped-to-floating transition.
// REPRESENTATIVE CASE: left-snapped window first activated motion.
#[test]
fn t06_snapped_first_drag_away_restores_with_full_configure() {
    let s0 = snapped_state();
    let (s1, _) = reduce(&s0, press(1, 500, 20));
    let (s2, effects) = reduce(&s1, motion(505, 25));
    assert_eq!(s2.placement.mode, PlacementMode::Floating);
    assert_eq!(
        (s2.placement.current.w, s2.placement.current.h),
        (
            s0.placement.floating_restore.w,
            s0.placement.floating_restore.h
        )
    );
    assert!(has_full_configure(&effects));
    assert!(effects.contains(&FrameEffect::LayoutInput));
    assert!(effects.contains(&FrameEffect::PaintChrome));
    assert!(effects.contains(&FrameEffect::ApplyShape));
}

// CONTRACT-REGRESSION T07: only the first activated motion restores size.
// TRIGGER: max drag-away followed by a second motion.
// OBSERVABLE RESULT: second motion is position-only and preserves Floating size.
// UNCOVERED GAP: current reducer has no first-activation distinction.
// FAILURE MUTATION: emit full configure effects on every move motion.
// REPRESENTATIVE CASE: repeated maximized drag motion.
#[test]
fn t07_second_motion_after_restore_is_position_only() {
    let s0 = maximized_state();
    let (s1, _) = reduce(&s0, press(1, 960, 20));
    let (s2, _) = reduce(&s1, motion(965, 25));
    let (s3, effects) = reduce(&s2, motion(985, 45));
    assert_eq!(s3.placement.mode, PlacementMode::Floating);
    assert_eq!(
        (s3.placement.current.w, s3.placement.current.h),
        (
            s0.placement.floating_restore.w,
            s0.placement.floating_restore.h
        )
    );
    assert_position_only(&effects);
}

// CONTRACT-REGRESSION T08: WM-owned fullscreen/maximized resize is refused.
// TRIGGER: resize press and motion in each non-resizable WM-owned mode.
// OBSERVABLE RESULT: no resize session, geometry, or native effects.
// UNCOVERED GAP: current reducer starts ResizeSession in both modes.
// FAILURE MUTATION: allow plan_resize to mutate current while mode-owned.
// REPRESENTATIVE CASE: bottom-right resize against max/fullscreen.
#[test]
fn t08_maximized_and_fullscreen_resize_is_refused() {
    for mode in [PlacementMode::Maximized, PlacementMode::Fullscreen] {
        let mut s0 = floating_state();
        s0.placement.mode = mode;
        s0.placement.current = if mode == PlacementMode::Maximized {
            WA
        } else {
            SCREEN
        };
        let (s1, press_effects) = reduce(&s0, press(2, 1900, 1050));
        assert!(s1.session.is_idle(), "mode {mode:?}");
        assert_eq!(s1.placement, s0.placement, "mode {mode:?}");
        assert_eq!(press_effects, vec![FrameEffect::Noop], "mode {mode:?}");
        let (s2, motion_effects) = reduce(&s1, motion(1950, 1100));
        assert_eq!(s2.placement, s0.placement, "mode {mode:?}");
        assert_eq!(motion_effects, vec![FrameEffect::Noop], "mode {mode:?}");
    }
}

// CONTRACT-REGRESSION T09: snapped resize exits snapped mode once.
// TRIGGER: snapped resize press, first resize motion, then another motion.
// OBSERVABLE RESULT: one Floating transition; later resize stays Floating.
// UNCOVERED GAP: current resize retains Snapped while changing its rect.
// FAILURE MUTATION: apply resize without clearing the snapped mode.
// REPRESENTATIVE CASE: left-snapped bottom-right resize.
#[test]
fn t09_snapped_resize_exits_once_and_stays_floating() {
    let s1 = pressed(&snapped_state(), 2, 960, 1080);
    let (s2, first_effects) = reduce(&s1, motion(965, 1085));
    assert_eq!(s2.placement.mode, PlacementMode::Floating);
    assert!(has_full_configure(&first_effects));
    let (s3, second_effects) = reduce(&s2, motion(985, 1105));
    assert_eq!(s3.placement.mode, PlacementMode::Floating);
    assert!(has_full_configure(&second_effects));
}

// CONTRACT-REGRESSION T11: mode/rect invariants survive pending max motion.
// TRIGGER: maximized title press followed by sub-threshold motion.
// OBSERVABLE RESULT: Maximized still owns the work-area rect.
// UNCOVERED GAP: current placement is rewritten from the restore snapshot.
// FAILURE MUTATION: mutate current before activation is complete.
// REPRESENTATIVE CASE: three-pixel max drag jitter.
#[test]
fn t11_maximized_pending_motion_preserves_mode_rect_invariant() {
    let s1 = pressed(&maximized_state(), 1, 960, 20);
    let (s2, _effects) = reduce(&s1, motion(963, 20));
    assert_eq!(s2.placement.mode, PlacementMode::Maximized);
    assert!(!matches!(s2.placement.mode, PlacementMode::Snapped(_)));
}

// CONTRACT-REGRESSION T12: mode/rect invariants survive pending snap motion.
// TRIGGER: snapped title press followed by sub-threshold motion.
// OBSERVABLE RESULT: Snapped still owns its snap rect.
// UNCOVERED GAP: current move planner can change current under Snapped.
// FAILURE MUTATION: let pending motion update authoritative placement.
// REPRESENTATIVE CASE: three-pixel left-snap drag jitter.
#[test]
fn t12_snapped_pending_motion_preserves_mode_rect_invariant() {
    let s0 = snapped_state();
    let (s1, _) = reduce(&s0, press(1, 500, 20));
    let (s2, _effects) = reduce(&s1, motion(503, 20));
    assert_eq!(s2.placement.mode, PlacementMode::Snapped(SnapTarget::Left));
    assert_eq!(
        (s2.placement.current.w, s2.placement.current.h),
        (s0.placement.current.w, s0.placement.current.h)
    );
    assert_eq!(s2.placement.floating_restore, s0.placement.floating_restore);
    assert_eq!(s2.placement.current, s0.placement.current);
}

// CONTRACT-REGRESSION T13: first restore is full configure, not a move alias.
// TRIGGER: max drag-away at the activation boundary.
// OBSERVABLE RESULT: frame root, client local, and client root all configure.
// UNCOVERED GAP: current max path emits only MoveFrameRoot plus notify.
// FAILURE MUTATION: collapse restore into position-only effects.
// REPRESENTATIVE CASE: exactly four-pixel max activation.
#[test]
fn t13_max_restore_activation_has_all_geometry_domains() {
    let s1 = pressed(&maximized_state(), 1, 960, 20);
    let (s2, effects) = reduce(&s1, motion(964, 24));
    assert_eq!(s2.placement.mode, PlacementMode::Floating);
    assert!(has_full_configure(&effects));
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, FrameEffect::MoveFrameRoot { .. }))
    );
}

// CONTRACT-REGRESSION T14: activation and later motion have distinct plans.
// TRIGGER: max press, first drag-away, then a second drag motion.
// OBSERVABLE RESULT: full configure once, position-only thereafter.
// UNCOVERED GAP: current reducer uses one position-only plan for both.
// FAILURE MUTATION: reuse the first activation plan for subsequent motions.
// REPRESENTATIVE CASE: two-sample maximized drag.
#[test]
fn t14_restore_then_motion_has_one_full_then_position_only_plan() {
    let s1 = pressed(&maximized_state(), 1, 960, 20);
    let (s2, first_effects) = reduce(&s1, motion(965, 25));
    assert!(has_full_configure(&first_effects));
    let (_, second_effects) = reduce(&s2, motion(985, 45));
    assert_position_only(&second_effects);
}

const ALL_SNAP_TARGETS: [SnapTarget; 8] = [
    SnapTarget::Left,
    SnapTarget::Right,
    SnapTarget::Top,
    SnapTarget::Bottom,
    SnapTarget::TopLeft,
    SnapTarget::TopRight,
    SnapTarget::BottomLeft,
    SnapTarget::BottomRight,
];

const EVEN_WORK_AREAS: [RootRect; 3] = [
    RootRect::new(0, 0, 1920, 1080),
    RootRect::new(7, 9, 1366, 768),
    RootRect::new(100, 40, 1280, 1024),
];

const ODD_WORK_AREAS: [RootRect; 4] = [
    RootRect::new(0, 0, 1921, 1080),
    RootRect::new(3, 5, 1920, 1081),
    RootRect::new(100, 40, 801, 601),
    RootRect::new(11, 13, 1367, 769),
];

// Preview owner: window-core snap_geometry, reached through the same
// frame-to-core target mapping that wm.rs uses for release actions.
fn preview_snap_rect(work_area: RootRect, target: SnapTarget) -> RootRect {
    let core_target = match target {
        SnapTarget::Left => CoreSnapTarget::LeftHalf,
        SnapTarget::Right => CoreSnapTarget::RightHalf,
        SnapTarget::Top => CoreSnapTarget::TopHalf,
        SnapTarget::Bottom => CoreSnapTarget::BottomHalf,
        SnapTarget::TopLeft => CoreSnapTarget::TopLeftQuarter,
        SnapTarget::TopRight => CoreSnapTarget::TopRightQuarter,
        SnapTarget::BottomLeft => CoreSnapTarget::BottomLeftQuarter,
        SnapTarget::BottomRight => CoreSnapTarget::BottomRightQuarter,
    };
    let area = CoreRect::new(work_area.x, work_area.y, work_area.w, work_area.h);
    let size = CoreSize::new(work_area.w, work_area.h);
    let rect = snap_geometry(core_target, area, size);
    RootRect::new(rect.x, rect.y, rect.width, rect.height)
}

fn snap_commit_mismatches(work_areas: &[RootRect]) -> Vec<String> {
    let mut out = Vec::new();
    for &work_area in work_areas {
        for target in ALL_SNAP_TARGETS {
            let commit = snap_rect(work_area, target);
            let preview = preview_snap_rect(work_area, target);
            if commit != preview {
                out.push(format!(
                    "work_area={work_area:?} target={target:?} commit={commit:?} preview={preview:?}"
                ));
            }
        }
    }
    out
}

// GREEN INVARIANT: on even work areas the committed rect equals the preview rect.
#[test]
fn snap_commit_matches_preview_on_even_work_areas() {
    let mismatches = snap_commit_mismatches(&EVEN_WORK_AREAS);
    assert!(
        mismatches.is_empty(),
        "commit/preview divergence:\n{}",
        mismatches.join("\n")
    );
}

// GREEN INVARIANT: odd widths/heights. snap_rect places the right/bottom half at
// floor(size/2) extent starting at the ceil offset; snap_geometry mirrors that exactly.
#[test]
fn snap_commit_matches_preview_on_odd_work_areas() {
    let all: Vec<RootRect> = EVEN_WORK_AREAS
        .iter()
        .chain(ODD_WORK_AREAS.iter())
        .copied()
        .collect();
    let mismatches = snap_commit_mismatches(&all);
    assert!(
        mismatches.is_empty(),
        "commit/preview divergence:\n{}",
        mismatches.join("\n")
    );
}
