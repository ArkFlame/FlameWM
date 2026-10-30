use super::super::geometry::FrameExtents;
use super::*;

pub(super) const WA: RootRect = RootRect::new(0, 0, 1920, 1080);
pub(super) const EXT: FrameExtents = FrameExtents::new(31, 1);
pub(super) const START: RootRect = RootRect::new(100, 100, 400, 300);

pub(super) const SCREEN: RootRect = RootRect::new(0, 0, 1920, 1080);

pub(super) fn press(source: u32, x: i32, y: i32) -> FrameEvent {
    FrameEvent::Press {
        source,
        pointer: RootPoint::new(x, y),
    }
}

pub(super) fn motion(x: i32, y: i32) -> FrameEvent {
    FrameEvent::Motion {
        pointer: RootPoint::new(x, y),
    }
}

pub(super) fn pressed(state: &ControllerState, source: u32, x: i32, y: i32) -> ControllerState {
    reduce(state, press(source, x, y)).0
}

pub(super) fn release(x: i32, y: i32) -> FrameEvent {
    release_at(x, y, 0)
}

pub(super) fn release_at(x: i32, y: i32, time_ms: u32) -> FrameEvent {
    FrameEvent::Release {
        pointer: RootPoint::new(x, y),
        time_ms,
    }
}

pub(super) fn floating_state() -> ControllerState {
    let placement = PlacementState {
        mode: PlacementMode::Floating,
        current: START,
        floating_restore: START,
        resume: None,
    };
    let mut registry = HashMap::new();
    registry.insert(1, FrameRegion::TitleDrag);
    registry.insert(2, FrameRegion::Resize(ResizeEdges::bottom_right()));
    registry.insert(3, FrameRegion::Control(FrameControl::MaximizeRestore));
    ControllerState::new(7, 71, placement, WA, ClientSizeHints::default(), EXT)
        .with_screen_rect(SCREEN)
        .with_registry(Rc::new(registry))
}

/// Floating fixture moved into `mode` at `current`, resumable back to `START`.
pub(super) fn resumed_state(mode: PlacementMode, current: RootRect) -> ControllerState {
    let mut state = floating_state();
    state.placement.mode = mode;
    state.placement.current = current;
    state.placement.resume = Some(PlacementSnapshot {
        mode: PlacementMode::Floating,
        rect: START,
    });
    state
}

pub(super) fn maximized_state() -> ControllerState {
    resumed_state(PlacementMode::Maximized, WA)
}

pub(super) fn snapped_state() -> ControllerState {
    resumed_state(
        PlacementMode::Snapped(SnapTarget::Left),
        RootRect::new(0, 0, 960, 1080),
    )
}

pub(super) fn has_full_configure(effects: &[FrameEffect]) -> bool {
    effects
        .iter()
        .any(|effect| matches!(effect, FrameEffect::ConfigureFrameRoot { .. }))
        && effects
            .iter()
            .any(|effect| matches!(effect, FrameEffect::ConfigureClientLocal { .. }))
        && effects
            .iter()
            .any(|effect| matches!(effect, FrameEffect::NotifyClientRoot { .. }))
}

pub(super) fn assert_position_only(effects: &[FrameEffect]) {
    assert_eq!(effects.len(), 3);
    assert!(matches!(effects[0], FrameEffect::MoveFrameRoot { .. }));
    assert!(matches!(effects[1], FrameEffect::NotifyClientRoot { .. }));
    assert!(matches!(effects[2], FrameEffect::PaintChrome));
    assert!(!has_any_frame_root(effects));
    assert!(!effects.contains(&FrameEffect::LayoutInput));
    assert!(!effects.contains(&FrameEffect::ApplyShape));
}

pub(super) fn has_frame(target: &[FrameEffect], x: i32, y: i32) -> bool {
    target.iter().any(|e| {
        matches!(
            e,
            FrameEffect::ConfigureFrameRoot { x: fx, y: fy, .. } if *fx == x && *fy == y
        )
    })
}

pub(super) fn has_move_frame(target: &[FrameEffect], x: i32, y: i32) -> bool {
    target.iter().any(|e| {
        matches!(
            e,
            FrameEffect::MoveFrameRoot { x: fx, y: fy } if *fx == x && *fy == y
        )
    })
}

pub(super) fn has_any_frame_root(target: &[FrameEffect]) -> bool {
    target
        .iter()
        .any(|e| matches!(e, FrameEffect::ConfigureFrameRoot { .. }))
}

pub(super) fn count_local(target: &[FrameEffect]) -> usize {
    target
        .iter()
        .filter(|e| matches!(e, FrameEffect::ConfigureClientLocal { .. }))
        .count()
}

#[test]
fn press_motion_release_move() {
    let s0 = floating_state();
    let (s1, e1) = reduce(&s0, press(1, 150, 120));
    assert!(matches!(s1.session, InteractionSession::Move(_)));
    assert!(e1.iter().any(|e| matches!(e, FrameEffect::Grab { .. })));
    let (s2, e2) = reduce(&s1, motion(200, 170));
    assert_eq!(s2.placement.current, RootRect::new(150, 150, 400, 300));
    assert!(has_move_frame(&e2, 150, 150));
    assert!(!has_any_frame_root(&e2));
    let (s3, e3) = reduce(&s2, release(200, 170));
    assert!(s3.session.is_idle());
    assert!(e3.contains(&FrameEffect::Ungrab));
    assert_eq!(s3.placement.current, RootRect::new(150, 150, 400, 300));
}

#[test]
fn press_motion_release_resize() {
    let s0 = floating_state();
    let (s1, e1) = reduce(&s0, press(2, 500, 400));
    assert!(matches!(s1.session, InteractionSession::Resize(_)));
    assert!(e1.iter().any(|e| matches!(e, FrameEffect::Grab { .. })));
    let (s2, _) = reduce(&s1, motion(550, 460));
    assert_eq!(s2.placement.current, RootRect::new(100, 100, 450, 360));
    let (s3, e3) = reduce(&s2, release(550, 460));
    assert!(s3.session.is_idle());
    assert!(e3.contains(&FrameEffect::Ungrab));
    assert!(e3.contains(&FrameEffect::ApplyShape));
    assert_eq!(s3.placement.current, RootRect::new(100, 100, 450, 360));
}

pub(super) fn outline_state() -> ControllerState {
    floating_state().with_contents_while_dragging(false)
}

/// A snapped window dragged out under the outline gets its floating size back: the frame and
/// the client are configured to the restore size, never left at the snapped size.
#[test]
fn outline_drag_out_of_snap_restores_the_floating_size() {
    let start = snapped_state().with_contents_while_dragging(false);
    let restore = start.placement.floating_restore;
    let (s1, _) = reduce(&start, press(1, 400, 10));
    let (s2, e2) = reduce(&s1, motion(700, 300));
    let (s3, e3) = reduce(&s2, release(700, 300));
    assert!(s3.session.is_idle());
    let restored = e2.iter().chain(&e3).any(|effect| {
        matches!(effect, FrameEffect::ConfigureFrameRoot { w, h, .. }
            if *w == restore.w && *h == restore.h)
    });
    assert!(restored, "no configure to {restore:?} in {e2:?} / {e3:?}");
    assert!(
        e2.iter()
            .any(|effect| matches!(effect, FrameEffect::DragOutline { rect: Some(_) }))
    );
    assert_eq!(s3.placement.mode, PlacementMode::Floating);
}

#[test]
fn outline_move_motion_never_touches_real_frame_or_client() {
    let (s1, _) = reduce(&outline_state(), press(1, 150, 120));
    let (s2, e2) = reduce(&s1, motion(200, 170));
    assert_eq!(
        e2,
        vec![FrameEffect::DragOutline {
            rect: Some(RootRect::new(150, 150, 400, 300)),
        }]
    );
    assert_eq!(s2.placement.current, RootRect::new(150, 150, 400, 300));
}

#[test]
fn outline_resize_motion_shows_outline_without_configure() {
    let (s1, _) = reduce(&outline_state(), press(2, 500, 400));
    let (_, e2) = reduce(&s1, motion(550, 460));
    assert_eq!(
        e2,
        vec![FrameEffect::DragOutline {
            rect: Some(RootRect::new(100, 100, 450, 360)),
        }]
    );
    assert!(!e2.iter().any(|e| matches!(
        e,
        FrameEffect::ConfigureFrameRoot { .. }
            | FrameEffect::MoveFrameRoot { .. }
            | FrameEffect::ConfigureClientLocal { .. }
            | FrameEffect::NotifyClientRoot { .. }
    )));
}

#[test]
fn outline_drag_predicate_tracks_only_outline_sessions() {
    let idle = outline_state();
    assert!(!idle.outline_drag());
    let (resizing, _) = reduce(&idle, press(2, 500, 400));
    assert!(resizing.outline_drag());
    let (contents, _) = reduce(&floating_state(), press(2, 500, 400));
    assert!(!contents.outline_drag());
    let (after_release, _) = reduce(&resizing, release(550, 460));
    assert!(!after_release.outline_drag());
}

#[test]
fn outline_move_release_hides_outline_then_applies_final_position() {
    let (s1, _) = reduce(&outline_state(), press(1, 150, 120));
    let (s2, _) = reduce(&s1, motion(200, 170));
    let (s3, e3) = reduce(&s2, release(200, 170));
    assert!(s3.session.is_idle());
    assert_eq!(e3.first(), Some(&FrameEffect::DragOutline { rect: None }));
    assert!(has_move_frame(&e3, 150, 150));
    assert_eq!(e3.last(), Some(&FrameEffect::Ungrab));
}

#[test]
fn outline_resize_release_applies_full_configure_and_hides_outline() {
    let (s1, _) = reduce(&outline_state(), press(2, 500, 400));
    let (s2, _) = reduce(&s1, motion(550, 460));
    let (_, e3) = reduce(&s2, release(550, 460));
    assert_eq!(e3.first(), Some(&FrameEffect::DragOutline { rect: None }));
    assert!(e3.contains(&FrameEffect::ConfigureFrameRoot {
        x: 100,
        y: 100,
        w: 450,
        h: 360,
    }));
    assert!(e3.contains(&FrameEffect::ApplyShape));
    assert_eq!(e3.last(), Some(&FrameEffect::Ungrab));
}

#[test]
fn outline_cancel_hides_outline() {
    let (s1, _) = reduce(&outline_state(), press(1, 150, 120));
    let (s2, _) = reduce(&s1, motion(200, 170));
    let (_, e3) = reduce(&s2, FrameEvent::Cancel);
    assert!(e3.contains(&FrameEffect::DragOutline { rect: None }));
}

#[test]
fn live_contents_mode_emits_no_outline() {
    let (s1, _) = reduce(&floating_state(), press(1, 150, 120));
    let (_, e2) = reduce(&s1, motion(200, 170));
    assert!(
        !e2.iter()
            .any(|e| matches!(e, FrameEffect::DragOutline { .. }))
    );
    let (_, e3) = reduce(&s1, release(200, 170));
    assert!(
        !e3.iter()
            .any(|e| matches!(e, FrameEffect::DragOutline { .. }))
    );
}

#[test]
fn control_click_toggles_max() {
    let s1 = pressed(&floating_state(), 3, 300, 110);
    assert!(matches!(s1.session, InteractionSession::ControlPress(_)));
    let (s2, e2) = reduce(&s1, release(300, 110));
    assert_eq!(s2.placement.mode, PlacementMode::Maximized);
    assert_eq!(s2.placement.current, WA);
    assert!(e2.contains(&FrameEffect::Ungrab));
    assert!(has_frame(&e2, 0, 0));
}

pub(super) fn title_click(
    state: &ControllerState,
    x: i32,
    y: i32,
    time_ms: u32,
) -> (ControllerState, Vec<FrameEffect>) {
    let (pressed, _) = reduce(state, press(1, x, y));
    reduce(&pressed, release_at(x, y, time_ms))
}

#[test]
fn title_double_click_within_window_toggles_max() {
    let s0 = floating_state();
    let (s1, e1) = title_click(&s0, 150, 120, 1000);
    assert_eq!(s1.placement.mode, PlacementMode::Floating);
    assert!(s1.session.is_idle());
    assert_eq!(e1, vec![FrameEffect::Ungrab]);
    assert!(s1.last_title_click.is_some());
    let (s2, e2) = title_click(&s1, 150, 120, 1300);
    assert_eq!(s2.placement.mode, PlacementMode::Maximized);
    assert_eq!(s2.placement.current, WA);
    assert!(s2.session.is_idle());
    assert!(s2.last_title_click.is_none());
    assert!(has_frame(&e2, 0, 0));
}

#[test]
fn title_double_click_on_maximized_restores() {
    let (s1, _) = title_click(&maximized_state(), 960, 20, 1000);
    assert_eq!(s1.placement.mode, PlacementMode::Maximized);
    let (s2, _) = title_click(&s1, 960, 20, 1100);
    assert_eq!(s2.placement.mode, PlacementMode::Floating);
    assert_eq!(s2.placement.current, START);
}

#[test]
fn title_second_click_after_timeout_does_not_toggle() {
    let s0 = floating_state();
    let (s1, _) = title_click(&s0, 150, 120, 1000);
    let (slow, _) = title_click(&s1, 150, 120, 1501);
    assert_eq!(slow.placement.mode, PlacementMode::Floating);
    assert!(slow.last_title_click.is_some());
    let (edge, _) = title_click(&s1, 150, 120, 1500);
    assert_eq!(edge.placement.mode, PlacementMode::Maximized);
}

#[test]
fn title_second_click_beyond_slop_does_not_toggle() {
    let s0 = floating_state();
    let (s1, _) = title_click(&s0, 150, 120, 1000);
    let (far, _) = title_click(&s1, 155, 120, 1100);
    assert_eq!(far.placement.mode, PlacementMode::Floating);
    let (diagonal, _) = title_click(&s1, 153, 123, 1100);
    assert_eq!(diagonal.placement.mode, PlacementMode::Floating);
    let (edge, _) = title_click(&s1, 154, 120, 1100);
    assert_eq!(edge.placement.mode, PlacementMode::Maximized);
}

#[test]
fn title_click_from_other_client_does_not_toggle() {
    let (mut s1, _) = title_click(&floating_state(), 150, 120, 1000);
    if let Some(record) = s1.last_title_click.as_mut() {
        record.client = 8;
    }
    let (s2, _) = title_click(&s1, 150, 120, 1100);
    assert_eq!(s2.placement.mode, PlacementMode::Floating);
    assert!(s2.last_title_click.is_some());
}

#[test]
fn title_drag_between_clicks_breaks_pair() {
    let s0 = floating_state();
    let (s1, _) = title_click(&s0, 150, 120, 1000);
    let (s2, _) = reduce(&s1, press(1, 150, 120));
    let (s3, _) = reduce(&s2, motion(200, 170));
    let (s4, _) = reduce(&s3, release_at(200, 170, 1200));
    assert_eq!(s4.placement.mode, PlacementMode::Floating);
    assert!(s4.last_title_click.is_none());
    let (s5, _) = title_click(&s4, 200, 170, 1300);
    assert_eq!(s5.placement.mode, PlacementMode::Floating);
}

#[test]
fn non_title_press_clears_pending_title_click() {
    let s0 = floating_state();
    let (s1, _) = title_click(&s0, 150, 120, 1000);
    let (s2, _) = reduce(&s1, press(2, 500, 400));
    let (s3, _) = reduce(&s2, release_at(500, 400, 1100));
    assert!(s3.last_title_click.is_none());
    let (s4, _) = title_click(&s3, 150, 120, 1200);
    assert_eq!(s4.placement.mode, PlacementMode::Floating);
}

#[test]
fn cancel_aborts_move() {
    let s1 = pressed(&floating_state(), 1, 150, 120);
    let (s2, e2) = reduce(&s1, FrameEvent::Cancel);
    assert!(s2.session.is_idle());
    assert_eq!(s2.placement.current, START);
    assert!(e2.contains(&FrameEffect::Ungrab));
    let (_, e3) = reduce(&s2, FrameEvent::Cancel);
    assert_eq!(e3, vec![FrameEffect::Noop]);
}

#[test]
fn snap_finalizes_left_half() {
    let s0 = floating_state();
    let (s1, e1) = reduce(
        &s0,
        FrameEvent::Snap {
            target: SnapTarget::Left,
        },
    );
    assert_eq!(s1.placement.mode, PlacementMode::Snapped(SnapTarget::Left));
    assert_eq!(s1.placement.current, RootRect::new(0, 0, 960, 1080));
    assert!(e1.contains(&FrameEffect::SnapPreview {
        target: Some(SnapTarget::Left)
    }));
    assert!(has_frame(&e1, 0, 0));
}

#[test]
fn window_margin_insets_snapped_and_maximized_frames() {
    let s0 = floating_state().with_window_margin(2);
    let (snapped, _) = reduce(
        &s0,
        FrameEvent::Snap {
            target: SnapTarget::Left,
        },
    );
    assert_eq!(snapped.placement.current, RootRect::new(2, 2, 957, 1076));
    let (maximized, _) = reduce(&s0, FrameEvent::ToggleMax);
    assert_eq!(maximized.placement.current, RootRect::new(2, 2, 1916, 1076));
}

#[test]
fn max_restore_returns_to_floating() {
    let s0 = floating_state();
    let (s1, _) = reduce(&s0, FrameEvent::ToggleMax);
    assert_eq!(s1.placement.mode, PlacementMode::Maximized);
    let (s2, e2) = reduce(&s1, FrameEvent::ToggleMax);
    assert_eq!(s2.placement.mode, PlacementMode::Floating);
    assert_eq!(s2.placement.current, START);
    assert!(has_frame(&e2, 100, 100));
}

#[test]
fn noop_geometry_yields_noop() {
    let s0 = floating_state();
    // Motion with zero delta while moving: plan equals start -> Noop.
    let (s1, _) = reduce(&s0, press(1, 150, 120));
    let (s2, e2) = reduce(&s1, motion(150, 120));
    assert_eq!(e2, vec![FrameEffect::Noop]);
    assert_eq!(s2.placement.current, START);
    // Bare configure with no movement on idle session -> Noop path.
    let (_, e3) = reduce(&s0, motion(0, 0));
    assert_eq!(e3, vec![FrameEffect::Noop]);
}

#[test]
fn move_motion_clamped_at_output_edge_emits_no_native_move() {
    let s1 = pressed(&floating_state(), 1, 150, 120);
    // First pointer step past the right edge moves the frame to the clamp.
    let (s2, e2) = reduce(&s1, motion(5150, 120));
    assert!(
        e2.iter()
            .any(|e| matches!(e, FrameEffect::MoveFrameRoot { .. }))
    );
    let clamped = s2.placement.current;
    // Further pointer travel keeps the same clamped frame: no move, no notify, no paint.
    let (s3, e3) = reduce(&s2, motion(6150, 120));
    assert_eq!(e3, vec![FrameEffect::Noop]);
    assert_eq!(s3.placement.current, clamped);
}

#[test]
fn resize_motion_leaving_frame_unchanged_emits_no_configure() {
    let s1 = pressed(&floating_state(), 2, 500, 400);
    let (s2, e2) = reduce(&s1, motion(550, 460));
    assert!(
        e2.iter()
            .any(|e| matches!(e, FrameEffect::ConfigureFrameRoot { .. }))
    );
    let (s3, e3) = reduce(&s2, motion(550, 460));
    assert_eq!(e3, vec![FrameEffect::Noop]);
    assert_eq!(s3.placement.current, RootRect::new(100, 100, 450, 360));
}

#[test]
fn release_after_clamped_motion_configures_client_at_final_size() {
    let s1 = pressed(&floating_state(), 2, 500, 400);
    // Grow, clamp at the output edge, then repeat the clamped point as an unchanged motion.
    let (s2, _) = reduce(&s1, motion(550, 460));
    let (s3, _) = reduce(&s2, motion(900, 900));
    let (s4, e4) = reduce(&s3, motion(900, 900));
    assert_eq!(e4, vec![FrameEffect::Noop]);
    let final_frame = s4.placement.current;
    let (_, release_effects) = reduce(&s4, release(900, 900));
    assert!(
        release_effects.iter().any(|e| matches!(
            e,
            FrameEffect::ConfigureFrameRoot { w, h, .. }
                if *w == final_frame.w && *h == final_frame.h
        )),
        "release must size the frame to the final geometry: {release_effects:?}"
    );
    assert!(
        release_effects
            .iter()
            .any(|e| matches!(e, FrameEffect::ConfigureClientLocal { .. })),
        "release must configure the client window: {release_effects:?}"
    );
    assert!(release_effects.contains(&FrameEffect::ApplyShape));
}
