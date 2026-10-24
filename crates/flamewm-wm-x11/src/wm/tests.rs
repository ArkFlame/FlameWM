#[test]
fn desktop_configure_is_forced_to_bottom_and_keeps_geometry() {
    let raise_request = ConfigureWindowAux::new()
        .x(0)
        .y(0)
        .width(1350)
        .height(641)
        .sibling(0x200000)
        .stack_mode(StackMode::ABOVE);
    let aux = desktop_stack_policy(raise_request, WindowKind::Desktop);
    assert_eq!(aux.stack_mode, Some(StackMode::BELOW));
    assert_eq!(aux.sibling, None);
    assert_eq!(
        (aux.x, aux.y, aux.width, aux.height),
        (Some(0), Some(0), Some(1350), Some(641))
    );
    // A desktop geometry-only request (no stack mask) still ends at the bottom.
    let geometry_only = ConfigureWindowAux::new().width(1350).height(600);
    assert_eq!(
        desktop_stack_policy(geometry_only, WindowKind::Desktop).stack_mode,
        Some(StackMode::BELOW)
    );
}

#[test]
fn non_desktop_configure_keeps_stack_intent() {
    let request = ConfigureWindowAux::new()
        .sibling(0x200000)
        .stack_mode(StackMode::ABOVE);
    for kind in [
        WindowKind::Normal,
        WindowKind::Utility,
        WindowKind::Dock,
        WindowKind::Popup,
    ] {
        let aux = desktop_stack_policy(request, kind);
        assert_eq!(aux.stack_mode, Some(StackMode::ABOVE), "{kind:?}");
        assert_eq!(aux.sibling, Some(0x200000), "{kind:?}");
    }
}

#[test]
fn destroyed_window_races_are_not_fatal_but_other_errors_are() {
    use x11rb::x11_utils::X11Error;
    let error = |kind: ErrorKind| {
        ReplyOrIdError::X11Error(X11Error {
            error_kind: kind,
            error_code: 3,
            sequence: 1,
            bad_value: 0,
            minor_opcode: 0,
            major_opcode: 20,
            extension_name: None,
            request_name: Some("GetProperty"),
        })
    };
    assert!(is_destroyed_window_race(&error(ErrorKind::Window)));
    assert!(is_destroyed_window_race(&error(ErrorKind::Drawable)));
    assert!(!is_destroyed_window_race(&error(ErrorKind::Access)));
    assert!(!is_destroyed_window_race(&ReplyOrIdError::IdsExhausted));
}
use super::*;
use crate::frame::model::ResizeEdges;
use crate::frame::session::{MoveAnchor, MoveSession};

#[test]
fn focus_reverts_to_pointer_root_so_root_shortcut_grabs_survive_focus_loss() {
    assert_eq!(FOCUS_REVERT_TO, InputFocus::POINTER_ROOT);
}

#[test]
fn wm_class_parse_characterization_trims_and_keeps_empty_tokens() {
    assert_eq!(
        parse_wm_class(b"firefox\0Firefox\0"),
        ("firefox".to_owned(), "Firefox".to_owned())
    );
    assert_eq!(
        parse_wm_class(b" app \0class\0"),
        ("app".to_owned(), "class".to_owned())
    );
    assert_eq!(
        parse_wm_class(b"firefox\0Firefox\0\0\0"),
        ("firefox".to_owned(), "Firefox".to_owned())
    );
    assert_eq!(
        parse_wm_class(b"firefox"),
        ("firefox".to_owned(), String::new())
    );
    assert_eq!(parse_wm_class(b""), (String::new(), String::new()));
    assert_eq!(
        parse_wm_class(b"\xffapp\0Cl\xe9ss\0"),
        ("\u{FFFD}app".to_owned(), "Cl\u{FFFD}ss".to_owned())
    );
}

fn test_icon_key(name: &str) -> flamewm_integrations_linux::icons::IconKey {
    use flamewm_integrations_linux::icons::{IconKey, IconLookupPurpose, IconRequest, IconSize};
    IconKey::new(
        IconLookupPurpose::Application,
        IconRequest::name(name),
        IconSize::new(24, 24),
        7,
        0,
    )
}

#[test]
fn catalog_result_requires_matching_client_lifetime_and_full_icon_key() {
    let key = test_icon_key("terminal");
    assert!(icon_completion_matches(42, Some(&key), 42, &key));
    assert!(!icon_completion_matches(43, Some(&key), 42, &key));
    assert!(!icon_completion_matches(
        42,
        Some(&test_icon_key("other")),
        42,
        &key
    ));
    let mut stale_theme = key.clone();
    stale_theme.theme_generation += 1;
    assert!(!icon_completion_matches(42, Some(&stale_theme), 42, &key));
    assert!(!icon_completion_matches(42, None, 42, &key));
}

#[test]
fn deferred_high_icon_work_is_bounded_to_one_current_job_per_client() {
    use flamewm_integrations_linux::icon_service::{IconJob, IconPriority};
    let mut deferred = HashMap::new();
    retain_deferred_icon_job(
        &mut deferred,
        5,
        10,
        IconJob {
            key: test_icon_key("old"),
            priority: IconPriority::High,
        },
    );
    retain_deferred_icon_job(
        &mut deferred,
        5,
        11,
        IconJob {
            key: test_icon_key("new"),
            priority: IconPriority::High,
        },
    );
    assert_eq!(deferred.len(), 1);
    assert_eq!(
        deferred
            .get(&5)
            .map(|(token, job)| (*token, job.key.clone())),
        Some((11, test_icon_key("new")))
    );
}

fn output(id: &str, geometry: Rect, work_area: Rect, primary: bool) -> OutputWorkArea {
    OutputWorkArea {
        output: flamewm_api::OutputId::new(id),
        geometry,
        work_area,
        primary,
    }
}

#[test]
fn local_output_selection_handles_adjacent_vertical_gap_and_spanning_windows() {
    let outputs = vec![
        output(
            "left",
            Rect::new(0, 0, 1000, 800),
            Rect::new(0, 0, 1000, 756),
            true,
        ),
        output(
            "right",
            Rect::new(1000, 0, 1200, 900),
            Rect::new(1000, 0, 1200, 845),
            false,
        ),
        output(
            "above",
            Rect::new(0, -700, 900, 700),
            Rect::new(0, -700, 900, 656),
            false,
        ),
    ];
    assert_eq!(
        select_output_for_rect(&outputs, Rect::new(1100, 40, 100, 100)),
        Some(1)
    );
    assert_eq!(
        select_output_for_rect(&outputs, Rect::new(50, -650, 200, 200)),
        Some(2)
    );
    assert_eq!(
        select_output_for_rect(&outputs, Rect::new(900, 100, 200, 100)),
        Some(1)
    );
    assert_eq!(
        select_output_for_rect(&outputs, Rect::new(2300, 100, 100, 100)),
        Some(0)
    );
}

#[test]
fn cross_output_window_selects_output_containing_centre_for_unequal_layouts() {
    // Horizontal: 1920x1080 left, 1280x720 right offset down by 180.
    let horizontal = vec![
        output(
            "left",
            Rect::new(0, 0, 1920, 1080),
            Rect::new(0, 0, 1920, 1050),
            true,
        ),
        output(
            "right",
            Rect::new(1920, 180, 1280, 720),
            Rect::new(1920, 180, 1280, 690),
            false,
        ),
    ];
    // 400x300 window straddling x=1920 with centre (2020, 330) on the right output.
    assert_eq!(
        select_output_for_rect(&horizontal, Rect::new(1820, 180, 400, 300)),
        Some(1)
    );
    // Centre (1900, 330) stays on the left output even though the window spills right.
    assert_eq!(
        select_output_for_rect(&horizontal, Rect::new(1700, 180, 400, 300)),
        Some(0)
    );

    // Negative-coordinate: 1280x720 left at x=-1280, 1920x1080 primary right.
    let negative = vec![
        output(
            "left",
            Rect::new(-1280, 0, 1280, 720),
            Rect::new(-1280, 0, 1280, 690),
            false,
        ),
        output(
            "right",
            Rect::new(0, 0, 1920, 1080),
            Rect::new(0, 0, 1920, 1050),
            true,
        ),
    ];
    assert_eq!(
        select_output_for_rect(&negative, Rect::new(-500, 100, 400, 300)),
        Some(0)
    );
    assert_eq!(
        select_output_for_rect(&negative, Rect::new(-100, 100, 400, 300)),
        Some(1)
    );
    // The selected output's work area (panel strut) is the one used for maximize/snap.
    assert_eq!(
        negative[select_output_for_rect(&negative, Rect::new(-500, 100, 400, 300)).unwrap()]
            .work_area,
        Rect::new(-1280, 0, 1280, 690)
    );
    assert_eq!(
        horizontal[select_output_for_rect(&horizontal, Rect::new(1820, 180, 400, 300)).unwrap()]
            .work_area,
        Rect::new(1920, 180, 1280, 690)
    );
    // Vertical: right output stacked above at negative y.
    let vertical = vec![
        output(
            "below",
            Rect::new(0, 0, 1920, 1080),
            Rect::new(0, 0, 1920, 1050),
            true,
        ),
        output(
            "above",
            Rect::new(0, -720, 1280, 720),
            Rect::new(0, -720, 1280, 690),
            false,
        ),
    ];
    assert_eq!(
        select_output_for_rect(&vertical, Rect::new(100, -600, 300, 200)),
        Some(1)
    );
    assert_eq!(
        select_output_for_rect(&vertical, Rect::new(100, -200, 300, 400)),
        Some(0)
    );
}

#[test]
fn output_reconciliation_drops_unplugged_and_accepts_replugged_geometry() {
    let mut outputs = vec![output(
        "panel",
        Rect::new(0, 0, 100, 100),
        Rect::new(0, 0, 100, 56),
        true,
    )];
    assert_eq!(
        select_output_for_rect(&outputs, Rect::new(20, 20, 10, 10)),
        Some(0)
    );
    outputs.clear();
    assert_eq!(
        select_output_for_rect(&outputs, Rect::new(20, 20, 10, 10)),
        None
    );
    outputs.push(output(
        "panel",
        Rect::new(0, 0, 100, 100),
        Rect::new(0, 0, 100, 45),
        true,
    ));
    assert_eq!(outputs[0].work_area.height, 45);
}

#[test]
fn next_wait_timeout_is_none_without_pending_deadline() {
    assert_eq!(next_wait_timeout(1_000, None), None);
}

#[test]
fn next_wait_timeout_returns_remaining_time_until_deadline() {
    assert_eq!(
        next_wait_timeout(1_000, Some(1_500)),
        Some(std::time::Duration::from_millis(500))
    );
}

#[test]
fn next_wait_timeout_clamps_past_deadline_to_zero() {
    assert_eq!(
        next_wait_timeout(2_000, Some(1_500)),
        Some(std::time::Duration::ZERO)
    );
    assert_eq!(
        next_wait_timeout(1_500, Some(1_500)),
        Some(std::time::Duration::ZERO)
    );
}

fn observed_release_action(
    session: InteractionSession,
    mode: PlacementMode,
    target: SnapTarget,
) -> ReleaseAction {
    let move_session = match session {
        InteractionSession::Move(session) => Some(session),
        _ => None,
    };
    release_action_for_move(move_session, Some(mode), target)
}

fn pending_move() -> InteractionSession {
    InteractionSession::Move(MoveSession {
        client: 7,
        start_pointer: RootPoint::new(520, 230),
        start_placement: PlacementSnapshot {
            mode: PlacementMode::Floating,
            rect: RootRect::new(500, 200, 400, 300),
        },
        anchor: MoveAnchor::new(20, 400, 30),
        grab_window: 99,
    })
}

#[test]
fn own_desktop_publish_echo_does_not_bump_workspace_revision() {
    // Echo of our own _NET_CURRENT_DESKTOP write must not advance the fence.
    assert!(!desktop_change_is_external(Some(2), 2));
    assert!(!desktop_change_is_external(Some(0), 0));
    // A foreign pager/client change must still advance the fence.
    assert!(desktop_change_is_external(Some(1), 2));
    // Unreadable property is treated as external (conservative fence advance).
    assert!(desktop_change_is_external(None, 2));
}

#[test]
fn own_workspace_metadata_publish_echo_does_not_bump_workspace_revision() {
    let names = desktop_names_payload(2);
    // Echo of our own publish (count + names) must not advance the fence.
    assert!(!workspace_metadata_is_external(Some(2), &names, 2));
    // A foreign count or name change must still advance the fence.
    assert!(workspace_metadata_is_external(Some(3), &names, 2));
    assert!(workspace_metadata_is_external(Some(2), b"Web\0Mail\0", 2));
    // Unreadable count is treated as external (conservative fence advance).
    assert!(workspace_metadata_is_external(None, &names, 2));
}

#[test]
fn desktop_names_match_agrees_with_payload_builder() {
    for count in 0..=12 {
        let names = desktop_names_payload(count);
        assert!(desktop_names_match(&names, count), "count={count}");
        assert!(!desktop_names_match(&names, count + 1), "count={count}");
    }
    assert!(!desktop_names_match(b"Workspace 1\0Workspace 3\0", 2));
    assert!(!desktop_names_match(b"Workspace 1\0Workspace 2", 2));
    assert!(!desktop_names_match(b"Workspace 1\0", 2));
    assert!(!desktop_names_match(b"Workspace 1\0Workspace 2\0x", 2));
    assert!(desktop_names_match(b"", 0));
}

#[test]
fn workspace_move_maps_only_on_visibility_flips() {
    assert_eq!(frame_visibility_delta(true, false), Some(false));
    assert_eq!(frame_visibility_delta(false, true), Some(true));
    assert_eq!(
        frame_visibility_delta(false, false),
        None,
        "minimized/hidden windows must not produce repeated unmaps"
    );
    assert_eq!(
        frame_visibility_delta(true, true),
        None,
        "sticky and same-workspace windows must not be remapped"
    );
}

#[test]
fn workspace_switch_marks_windows_only_on_visibility_or_focus_change() {
    assert!(!workspace_switch_touches_windows(false, false));
    assert!(workspace_switch_touches_windows(true, false));
    assert!(workspace_switch_touches_windows(false, true));
    assert!(workspace_switch_touches_windows(true, true));
}

#[test]
fn resize_from_left_preserves_right_edge() {
    let original = Rect::new(100, 100, 600, 400);
    let edges = ResizeEdges {
        left: true,
        ..ResizeEdges::default()
    };
    let resized = ResizeEdges::resized_rect(original, edges, 100, 0);
    assert_eq!(resized, Rect::new(200, 100, 500, 400));
}

#[test]
fn resize_enforces_minimum_size() {
    let original = Rect::new(100, 100, 600, 400);
    let edges = ResizeEdges {
        left: true,
        top: true,
        ..ResizeEdges::default()
    };
    let resized = ResizeEdges::resized_rect(original, edges, 1_000, 1_000);
    assert_eq!(resized.width, 160);
    assert_eq!(resized.height, 96);
}

#[test]
fn cursor_kind_covers_eight_directions() {
    use flamewm_render_core::CursorKind;
    let edges = |left, top, right, bottom| ResizeEdges {
        left,
        top,
        right,
        bottom,
        ..ResizeEdges::default()
    };
    let cases = [
        (
            edges(true, true, false, false),
            CursorKind::ResizeNorthWestSouthEast,
        ),
        (
            edges(false, false, true, true),
            CursorKind::ResizeNorthWestSouthEast,
        ),
        (
            edges(false, true, true, false),
            CursorKind::ResizeNorthEastSouthWest,
        ),
        (
            edges(true, false, false, true),
            CursorKind::ResizeNorthEastSouthWest,
        ),
        (
            edges(true, false, false, false),
            CursorKind::ResizeHorizontal,
        ),
        (
            edges(false, false, true, false),
            CursorKind::ResizeHorizontal,
        ),
        (edges(false, true, false, false), CursorKind::ResizeVertical),
        (edges(false, false, false, true), CursorKind::ResizeVertical),
    ];
    for (e, expected) in cases {
        assert_eq!(e.cursor_kind(), expected);
    }
    assert_eq!(ResizeEdges::default().cursor_kind(), CursorKind::Default);
}

#[test]
fn start_menu_toggle_flag_defaults_to_false() {
    assert!(!WmChangeSet::default().start_menu_toggle);
    let mut changes = WmChangeSet::default();
    changes.start_menu_toggle = true;
    let drained = changes.take();
    assert!(drained.start_menu_toggle);
    assert!(!changes.start_menu_toggle);
}

/// Replays one event batch: events in order, then the batch-boundary commit.
/// Returns how many times Start toggled in that batch.
fn run_batch(tap: &mut SuperTap, events: &[(&str, u8, u32)]) -> usize {
    let mut toggles = 0;
    for &(kind, keycode, time) in events {
        let outcome = match kind {
            "super-press" => tap.press_super(keycode, time),
            "super-release" => {
                tap.release_super(keycode, time);
                SuperTapOutcome::Idle
            }
            "other-press" => tap.press_other(),
            other => panic!("unknown test event kind {other}"),
        };
        toggles += usize::from(outcome == SuperTapOutcome::Toggle);
    }
    toggles + usize::from(tap.commit() == SuperTapOutcome::Toggle)
}

const SUPER_KEY: u8 = 133;
const RIGHT_KEY: u8 = 114;

#[test]
fn super_tap_toggles_exactly_once() {
    let mut tap = SuperTap::default();
    assert_eq!(run_batch(&mut tap, &[("super-press", SUPER_KEY, 100)]), 0);
    assert_eq!(run_batch(&mut tap, &[("super-release", SUPER_KEY, 150)]), 1);
    assert_eq!(tap.commit(), SuperTapOutcome::Idle);
}

#[test]
fn super_autorepeat_pairs_while_held_never_toggle() {
    let mut tap = SuperTap::default();
    assert_eq!(run_batch(&mut tap, &[("super-press", SUPER_KEY, 100)]), 0);
    // Each repeat is a KeyRelease and KeyPress sharing one timestamp.
    for stamp in [200, 225, 250] {
        let batch = [
            ("super-release", SUPER_KEY, stamp),
            ("super-press", SUPER_KEY, stamp),
        ];
        assert_eq!(run_batch(&mut tap, &batch), 0, "repeat at {stamp}");
    }
    // The final real release is the single toggle for the whole hold.
    assert_eq!(run_batch(&mut tap, &[("super-release", SUPER_KEY, 400)]), 1);
}

#[test]
fn super_press_with_no_release_yet_is_a_repeat_not_a_new_tap() {
    let mut tap = SuperTap::default();
    assert_eq!(
        run_batch(
            &mut tap,
            &[
                ("super-press", SUPER_KEY, 100),
                ("super-press", SUPER_KEY, 125),
                ("super-press", SUPER_KEY, 150),
            ],
        ),
        0
    );
    assert_eq!(run_batch(&mut tap, &[("super-release", SUPER_KEY, 175)]), 1);
}

#[test]
fn super_right_chord_never_toggles() {
    let mut tap = SuperTap::default();
    assert_eq!(
        run_batch(
            &mut tap,
            &[
                ("super-press", SUPER_KEY, 100),
                ("other-press", RIGHT_KEY, 120),
                ("super-release", SUPER_KEY, 150),
            ],
        ),
        0
    );
    // The Right key's own autorepeat must not start a tap either.
    assert_eq!(
        run_batch(
            &mut tap,
            &[
                ("other-press", RIGHT_KEY, 200),
                ("other-press", RIGHT_KEY, 225),
            ],
        ),
        0
    );
}

#[test]
fn ctrl_super_right_never_starts_a_tap() {
    // Ctrl is a plain other-key press; Super with Ctrl held is not bare.
    let mut tap = SuperTap::default();
    assert_eq!(
        run_batch(
            &mut tap,
            &[
                ("other-press", 37, 100),
                ("other-press", SUPER_KEY, 110),
                ("other-press", RIGHT_KEY, 120),
                ("other-press", RIGHT_KEY, 150),
            ],
        ),
        0
    );
    assert_eq!(run_batch(&mut tap, &[("super-release", SUPER_KEY, 200)]), 0);
}

#[test]
fn tap_followed_by_other_key_in_same_batch_still_toggles_once() {
    let mut tap = SuperTap::default();
    assert_eq!(
        run_batch(
            &mut tap,
            &[
                ("super-press", SUPER_KEY, 100),
                ("super-release", SUPER_KEY, 150),
                ("other-press", RIGHT_KEY, 160),
            ],
        ),
        1
    );
}

#[test]
fn focus_loss_while_held_cancels_the_tap() {
    let mut tap = SuperTap::default();
    assert_eq!(run_batch(&mut tap, &[("super-press", SUPER_KEY, 100)]), 0);
    tap.focus_lost();
    assert_eq!(run_batch(&mut tap, &[("super-release", SUPER_KEY, 150)]), 0);
}

#[test]
fn focus_loss_after_release_cancels_pending_commit() {
    let mut tap = SuperTap::default();
    assert_eq!(run_batch(&mut tap, &[("super-press", SUPER_KEY, 100)]), 0);
    tap.release_super(SUPER_KEY, 150);
    tap.focus_lost();
    assert_eq!(tap.commit(), SuperTapOutcome::Idle);
}

#[test]
fn two_separate_taps_toggle_twice() {
    let mut tap = SuperTap::default();
    assert_eq!(
        run_batch(
            &mut tap,
            &[
                ("super-press", SUPER_KEY, 100),
                ("super-release", SUPER_KEY, 150),
                ("super-press", SUPER_KEY, 300),
                ("super-release", SUPER_KEY, 350),
            ],
        ),
        2
    );
}

#[test]
fn bare_state_ignores_lock_bits_only() {
    assert!(is_bare_state(0));
    assert!(is_bare_state(0x0002)); // CapsLock
    assert!(is_bare_state(0x0010)); // NumLock
    assert!(!is_bare_state(0x0040)); // Super held as modifier
    assert!(!is_bare_state(0x0004)); // Ctrl
}

#[test]
fn change_set_drains_once_per_turn() {
    let mut changes = WmChangeSet::default();
    assert!(!changes.any());
    changes.windows = true;
    changes.work_area = true;
    let drained = changes.take();
    assert!(drained.windows && drained.work_area);
    assert!(!changes.any());
}

#[test]
fn frame_control_hit_targets_unchanged() {
    use crate::frame::input::frame_control_at;
    use crate::frame::model::FrameControl as EngineControl;
    let titlebar = chrome::TITLEBAR_HEIGHT;
    // Skin chrome: button_width=38, controls tile the right edge.
    // 400-wide frame: min [286,324), max [324,362), close [362,400).
    assert_eq!(
        frame_control_at(400, titlebar, 390),
        Some(EngineControl::Close)
    );
    assert_eq!(
        frame_control_at(400, titlebar, 362),
        Some(EngineControl::Close)
    );
    assert_eq!(
        frame_control_at(400, titlebar, 360),
        Some(EngineControl::MaximizeRestore)
    );
    assert_eq!(
        frame_control_at(400, titlebar, 328),
        Some(EngineControl::MaximizeRestore)
    );
    assert_eq!(
        frame_control_at(400, titlebar, 324),
        Some(EngineControl::MaximizeRestore)
    );
    assert_eq!(
        frame_control_at(400, titlebar, 286),
        Some(EngineControl::Minimize)
    );
    assert_eq!(frame_control_at(400, titlebar, 285), None);
    assert_eq!(frame_control_at(400, titlebar, 100), None);
}

#[test]
fn frame_event_source_prefers_live_registered_child() {
    let mut registry = FrameRegistry::new();
    registry.register(test_frame_resources());
    assert_eq!(resolve_frame_event_source(&registry, 99, 7), 7);
    assert_eq!(resolve_frame_event_source(&registry, 99, 0), 99);
    assert_eq!(resolve_frame_event_source(&registry, 99, 42), 99);
}

#[test]
fn pointer_grab_target_accepts_live_frame_or_input_child() {
    let resources = test_frame_resources();
    assert!(pointer_grab_target(2, resources));
    assert!(pointer_grab_target(7, resources));
    assert!(!pointer_grab_target(99, resources));
}

fn test_frame_resources() -> crate::frame::resources::FrameResources {
    crate::frame::resources::FrameResources::new(1, 2, 3, [4, 5, 6], [7, 8, 9, 10, 11, 12, 13, 14])
}

// CONTRACT-REGRESSION T10: release mapping is total and typed.
// TRIGGER: release policy receives each snap candidate, including None.
// OBSERVABLE RESULT: None is no action, Maximize stays Maximize, and the
// eight half/quarter candidates become their matching engine snaps.
// UNCOVERED GAP: release mapping collapses Maximize and None to Left.
// FAILURE MUTATION: retain a Maximize/None -> Left fallback arm.
// REPRESENTATIVE CASE: the complete release-candidate mapping table.
#[test]
fn t10_release_action_mapping_is_total() {
    use crate::frame::model::SnapTarget as EngineSnapTarget;

    let cases = [
        (SnapTarget::None, ReleaseAction::None),
        (SnapTarget::Maximize, ReleaseAction::Maximize),
        (
            SnapTarget::LeftHalf,
            ReleaseAction::Snap(EngineSnapTarget::Left),
        ),
        (
            SnapTarget::RightHalf,
            ReleaseAction::Snap(EngineSnapTarget::Right),
        ),
        (
            SnapTarget::TopHalf,
            ReleaseAction::Snap(EngineSnapTarget::Top),
        ),
        (
            SnapTarget::BottomHalf,
            ReleaseAction::Snap(EngineSnapTarget::Bottom),
        ),
        (
            SnapTarget::TopLeftQuarter,
            ReleaseAction::Snap(EngineSnapTarget::TopLeft),
        ),
        (
            SnapTarget::TopRightQuarter,
            ReleaseAction::Snap(EngineSnapTarget::TopRight),
        ),
        (
            SnapTarget::BottomLeftQuarter,
            ReleaseAction::Snap(EngineSnapTarget::BottomLeft),
        ),
        (
            SnapTarget::BottomRightQuarter,
            ReleaseAction::Snap(EngineSnapTarget::BottomRight),
        ),
    ];
    let mismatches = cases
        .into_iter()
        .filter_map(|(target, expected)| {
            let actual = release_action(target);
            (actual != expected).then_some((target, expected, actual))
        })
        .collect::<Vec<_>>();
    assert!(
        mismatches.is_empty(),
        "release mapping mismatches: {mismatches:?}"
    );
}

// CONTRACT-REGRESSION T11: an unactivated move cannot snap on release.
// TRIGGER: title press arms Move, then release reaches an edge before any
// activating motion crosses the threshold.
// OBSERVABLE RESULT: release eligibility is None; no Snap action exists.
// UNCOVERED GAP: release_from_move currently treats a pending Move as
// activated and evaluates the edge target anyway.
// FAILURE MUTATION: admit every InteractionSession::Move at release.
// REPRESENTATIVE CASE: floating title press/release with LeftHalf target.
#[test]
fn t11_unactivated_move_release_is_not_snap_eligible() {
    let session = pending_move();
    assert!(matches!(session, InteractionSession::Move(_)));
    assert_eq!(
        observed_release_action(session, PlacementMode::Floating, SnapTarget::LeftHalf),
        ReleaseAction::None
    );
}

#[test]
fn topology_dirty_is_consumed_exactly_once_per_change() {
    // A RandR notification must survive until the runtime consumes it, then clear.
    let mut dirty = TopologyDirty::default();
    assert!(!dirty.take());
    dirty.mark();
    dirty.mark();
    assert!(dirty.take());
    assert!(!dirty.take());
}

#[test]
fn removed_output_rehomes_floating_snapped_and_maximized_clients() {
    let global = Rect::new(0, 0, 1920, 1040);
    let surviving = vec![output(
        "A",
        Rect::new(0, 0, 1920, 1080),
        Rect::new(0, 0, 1920, 1040),
        true,
    )];
    // A window on the removed right output is pulled onto the surviving output, keeping 1/4
    // (100 px) visible; it may overflow the primary work area (taskbar band included).
    let floating = rehome_outer(
        PlacementMode::Floating,
        Rect::new(2500, 900, 400, 300),
        &surviving,
        global,
    );
    assert_eq!(floating, Rect::new(1820, 900, 400, 300));
    let snapped = rehome_outer(
        PlacementMode::Snapped(crate::frame::model::SnapTarget::Right),
        Rect::new(2900, 0, 960, 1040),
        &surviving,
        global,
    );
    assert_eq!(snapped, Rect::new(960, 0, 960, 1040));
    let maximized = rehome_outer(
        PlacementMode::Maximized,
        Rect::new(2000, 0, 1920, 1040),
        &surviving,
        global,
    );
    assert_eq!(maximized, Rect::new(0, 0, 1920, 1040));
    // Fullscreen is left to its own owner; reconcile must not move it.
    let fullscreen = Rect::new(2000, 0, 1920, 1080);
    assert_eq!(
        rehome_outer(PlacementMode::Fullscreen, fullscreen, &surviving, global),
        fullscreen
    );
    // A client already inside the surviving layout is untouched.
    let inside = Rect::new(100, 100, 300, 200);
    assert_eq!(
        rehome_outer(PlacementMode::Floating, inside, &surviving, global),
        inside
    );
}

#[test]
fn negative_origin_outputs_clamp_without_unsigned_wrap() {
    let global = Rect::new(0, 0, 1920, 1040);
    let outputs = vec![
        output(
            "L",
            Rect::new(-1920, 0, 1920, 1080),
            Rect::new(-1920, 0, 1920, 1040),
            false,
        ),
        output(
            "P",
            Rect::new(0, 0, 1920, 1080),
            Rect::new(0, 0, 1920, 1040),
            true,
        ),
        output(
            "T",
            Rect::new(0, -1080, 1920, 1080),
            Rect::new(0, -1080, 1920, 1040),
            false,
        ),
    ];
    // Overlaps the left output with y above its work area: floating windows may overflow the
    // work area, and this one keeps a quarter visible on the outputs, so it is left alone.
    assert_eq!(
        rehome_outer(
            PlacementMode::Floating,
            Rect::new(-1900, -50, 400, 300),
            &outputs,
            global
        ),
        Rect::new(-1900, -50, 400, 300)
    );
    // Lives entirely on the negative-y output: already inside its work area, unchanged.
    let above = Rect::new(100, -1000, 400, 300);
    assert_eq!(
        rehome_outer(PlacementMode::Floating, above, &outputs, global),
        above
    );
    // Fully outside every output: pulled to the nearest output so 1/4 (100 px) stays visible.
    assert_eq!(
        rehome_outer(
            PlacementMode::Floating,
            Rect::new(-2500, 100, 400, 300),
            &outputs,
            global
        ),
        Rect::new(-2220, 100, 400, 300)
    );
    // Snapped keeps work-area semantics: no output contains it, so the primary work area wins.
    assert_eq!(
        rehome_outer(
            PlacementMode::Snapped(crate::frame::model::SnapTarget::Left),
            Rect::new(-2500, 100, 400, 300),
            &outputs,
            global
        ),
        Rect::new(0, 100, 400, 300)
    );
}

#[test]
fn dock_raise_targets_frame_sibling_above_so_dock_stays_over_focused_normal() {
    let aux = dock_above_frame_aux(0x42);
    assert_eq!(aux.sibling, Some(0x42));
    assert_eq!(aux.stack_mode, Some(StackMode::ABOVE));
}

#[test]
fn grab_reaches_dismissed_counter_static_shape() {
    // Serial-owner static check: switch path must carry the split
    // metadata/current pair plus the renamed counters/spans without
    // touching query_tree or client-list publish.
    let src = WM_SOURCE;
    assert!(src.contains("fn publish_workspace_metadata("));
    assert!(src.contains("fn publish_current_workspace("));
    assert!(src.contains("\"wm.workspace.switch.total\""));
    assert!(src.contains("\"wm.workspace.switch.visibility\""));
    assert!(src.contains("\"wm.workspace.switch.ewmh_current\""));
    assert!(src.contains("\"wm.workspace.switch.shell_publish\""));
    assert!(src.contains("\"wm.workspace.query_tree.count\""));
    assert!(src.contains("NotifyClientRoot"));
    assert!(src.contains("ConfigureClientLocal"));
    assert!(src.contains("ConfigureFrameRoot"));
    assert!(src.contains("with_screen_rect"));
}

#[test]
fn workspace_left_right_stay_inside_the_current_row() {
    // Four workspaces read 1 2 / 3 4: the row edge is a no-op, never a row jump.
    assert_eq!(
        workspace_shortcut_target(0, 4, ACTION_WORKSPACE_RIGHT),
        Some(1)
    );
    assert_eq!(
        workspace_shortcut_target(1, 4, ACTION_WORKSPACE_RIGHT),
        None
    );
    assert_eq!(
        workspace_shortcut_target(1, 4, ACTION_WORKSPACE_LEFT),
        Some(0)
    );
    assert_eq!(workspace_shortcut_target(0, 4, ACTION_WORKSPACE_LEFT), None);
    assert_eq!(
        workspace_shortcut_target(2, 4, ACTION_WORKSPACE_RIGHT),
        Some(3)
    );
    assert_eq!(workspace_shortcut_target(2, 4, ACTION_WORKSPACE_LEFT), None);
    assert_eq!(
        workspace_shortcut_target(3, 4, ACTION_WORKSPACE_RIGHT),
        None
    );
    // Six workspaces read 1 2 3 / 4 5 6 with the same same-row rule.
    assert_eq!(
        workspace_shortcut_target(0, 6, ACTION_WORKSPACE_RIGHT),
        Some(1)
    );
    assert_eq!(
        workspace_shortcut_target(2, 6, ACTION_WORKSPACE_RIGHT),
        None
    );
    assert_eq!(
        workspace_shortcut_target(5, 6, ACTION_WORKSPACE_LEFT),
        Some(4)
    );
    assert_eq!(workspace_shortcut_target(3, 6, ACTION_WORKSPACE_LEFT), None);
}

#[test]
fn workspace_up_down_switch_rows_without_wrap() {
    // Four workspaces 1 2 / 3 4: down from 1 lands on 3, never on 2.
    assert_eq!(
        workspace_shortcut_target(0, 4, ACTION_WORKSPACE_DOWN),
        Some(2)
    );
    assert_eq!(
        workspace_shortcut_target(2, 4, ACTION_WORKSPACE_UP),
        Some(0)
    );
    assert_eq!(workspace_shortcut_target(0, 4, ACTION_WORKSPACE_UP), None);
    assert_eq!(workspace_shortcut_target(2, 4, ACTION_WORKSPACE_DOWN), None);
    // Six workspaces 1 2 3 / 4 5 6: down from 3 (index 2) lands on 6 (index 5).
    assert_eq!(
        workspace_shortcut_target(2, 6, ACTION_WORKSPACE_DOWN),
        Some(5)
    );
    assert_eq!(
        workspace_shortcut_target(5, 6, ACTION_WORKSPACE_UP),
        Some(2)
    );
    // Odd tail: five workspaces 1 2 3 / 4 5 hold no cell under 3.
    assert_eq!(workspace_shortcut_target(2, 5, ACTION_WORKSPACE_DOWN), None);
    // Vertical-only topologies: one workspace has no neighbour, a pair moves down/up only.
    assert_eq!(workspace_shortcut_target(0, 1, ACTION_WORKSPACE_DOWN), None);
    assert_eq!(
        workspace_shortcut_target(0, 1, ACTION_WORKSPACE_RIGHT),
        None
    );
    assert_eq!(workspace_shortcut_target(0, 1, ACTION_WORKSPACE_LEFT), None);
    assert_eq!(
        workspace_shortcut_target(0, 2, ACTION_WORKSPACE_DOWN),
        Some(1)
    );
    assert_eq!(
        workspace_shortcut_target(1, 2, ACTION_WORKSPACE_UP),
        Some(0)
    );
    assert_eq!(
        workspace_shortcut_target(0, 2, ACTION_WORKSPACE_RIGHT),
        None
    );
}

#[test]
fn workspace_shortcut_actions_pin_their_grid_direction() {
    assert_eq!(
        flamewm_api::shortcuts::workspace_direction(ACTION_WORKSPACE_LEFT),
        Some(flamewm_api::workspace::GridDirection::Left)
    );
    assert_eq!(
        flamewm_api::shortcuts::workspace_direction(ACTION_WORKSPACE_RIGHT),
        Some(flamewm_api::workspace::GridDirection::Right)
    );
    assert_eq!(
        flamewm_api::shortcuts::workspace_direction(ACTION_WORKSPACE_UP),
        Some(flamewm_api::workspace::GridDirection::Up)
    );
    assert_eq!(
        flamewm_api::shortcuts::workspace_direction(ACTION_WORKSPACE_DOWN),
        Some(flamewm_api::workspace::GridDirection::Down)
    );
    // The window close shortcut must never resolve to a workspace target.
    assert_eq!(workspace_shortcut_target(0, 4, ACTION_WINDOW_CLOSE), None);
    // An out-of-range current workspace is a no-op rather than a panic.
    assert_eq!(workspace_shortcut_target(9, 4, ACTION_WORKSPACE_LEFT), None);
}

#[test]
fn desktop_publish_exposes_the_two_row_grid() {
    let screen = Rect::new(0, 0, 1920, 1080);
    // Six workspaces 1 2 3 / 4 5 6 occupy three columns of two rows.
    assert_eq!(publish::desktop_geometry_payload(6, screen), [5760, 2160]);
    assert_eq!(
        publish::desktop_viewport_payload(6, screen),
        vec![0, 0, 1920, 0, 3840, 0, 0, 1080, 1920, 1080, 3840, 1080]
    );
    // Four workspaces 1 2 / 3 4 use two columns; the odd tail of five keeps its own cell.
    assert_eq!(publish::desktop_geometry_payload(4, screen), [3840, 2160]);
    assert_eq!(
        publish::desktop_viewport_payload(4, screen),
        vec![0, 0, 1920, 0, 0, 1080, 1920, 1080]
    );
    assert_eq!(publish::desktop_geometry_payload(5, screen), [5760, 2160]);
    assert_eq!(
        publish::desktop_viewport_payload(5, screen),
        vec![0, 0, 1920, 0, 3840, 0, 0, 1080, 1920, 1080]
    );
}

#[test]
fn drag_edge_wraps_both_directions_and_warps_opposite_at_same_y() {
    let root = Rect::new(0, 0, 1920, 1080);
    assert_eq!(
        drag_edge_at(RootPoint::new(0, 400), root),
        Some(DragEdge::Left)
    );
    assert_eq!(
        drag_edge_at(RootPoint::new(1919, 400), root),
        Some(DragEdge::Right)
    );
    assert_eq!(drag_edge_at(RootPoint::new(960, 400), root), None);
    assert_eq!(drag_edge_target(0, 4, DragEdge::Left), Some(1));
    assert_eq!(drag_edge_target(3, 4, DragEdge::Right), Some(2));
    assert_eq!(drag_edge_target(0, 1, DragEdge::Right), None);
    // Six workspaces are 1 2 3 / 4 5 6. Both edges wrap *inside* that row.
    assert_eq!(drag_edge_target(2, 6, DragEdge::Right), Some(0));
    assert_eq!(drag_edge_target(3, 6, DragEdge::Left), Some(5));
    assert_eq!(drag_edge_target(4, 6, DragEdge::Left), Some(3));
    assert_eq!(drag_edge_target(4, 6, DragEdge::Right), Some(5));
    // Five workspaces are 1 2 3 / 4 5; never jump from 5 to row 1.
    assert_eq!(drag_edge_target(4, 5, DragEdge::Right), Some(3));
    assert_eq!(drag_edge_target(3, 5, DragEdge::Left), Some(4));
    // A one-cell lower row has no mouse-edge destination, preventing self-switch.
    assert_eq!(drag_edge_target(2, 3, DragEdge::Right), None);
    assert_eq!(drag_edge_target(2, 3, DragEdge::Left), None);
    assert_eq!(
        drag_edge_destination(RootPoint::new(1919, 400), root, DragEdge::Right, 400),
        RootPoint::new(24, 400)
    );
    assert_eq!(
        drag_edge_destination(RootPoint::new(0, 400), root, DragEdge::Left, 400),
        RootPoint::new(1895, 400)
    );
    assert_eq!(
        drag_edge_at(RootPoint::new(960, 0), root),
        Some(DragEdge::Top)
    );
    assert_eq!(
        drag_edge_at(RootPoint::new(960, 1079), root),
        Some(DragEdge::Bottom)
    );
    assert_eq!(
        drag_edge_at(RootPoint::new(0, 0), root),
        Some(DragEdge::Left)
    );
    assert_eq!(
        drag_edge_at(RootPoint::new(1919, 1079), root),
        Some(DragEdge::Right)
    );
    assert_eq!(drag_edge_at(RootPoint::new(1920, 400), root), None);
    assert_eq!(drag_edge_target(0, 6, DragEdge::Bottom), Some(3));
    assert_eq!(drag_edge_target(2, 6, DragEdge::Bottom), Some(5));
    assert_eq!(drag_edge_target(5, 6, DragEdge::Bottom), Some(2));
    assert_eq!(drag_edge_target(3, 6, DragEdge::Top), Some(0));
    assert_eq!(drag_edge_target(2, 5, DragEdge::Bottom), None);
    assert_eq!(drag_edge_target(0, 1, DragEdge::Top), None);
    assert_eq!(drag_edge_target(0, 2, DragEdge::Bottom), Some(1));
    let from_bottom = drag_edge_destination(RootPoint::new(960, 1079), root, DragEdge::Bottom, 400);
    assert_eq!(from_bottom, RootPoint::new(960, 24));
    assert_eq!(drag_entry_frame_y(from_bottom, DragEdge::Bottom), 8);
    let from_top = drag_edge_destination(RootPoint::new(960, 0), root, DragEdge::Top, 400);
    assert_eq!(from_top, RootPoint::new(960, 895));
    assert_eq!(drag_entry_frame_y(from_top, DragEdge::Top), 879);
    assert!(root.bottom() - drag_entry_frame_y(from_top, DragEdge::Top) >= 400 / 2);
    assert_eq!(DragEdge::Top.axis(), DragEdge::Bottom.axis());
    assert_ne!(DragEdge::Right.axis(), DragEdge::Top.axis());
}

#[test]
fn drag_edge_requires_same_active_move_dwell_and_resets_on_direction_change() {
    let now = std::time::Instant::now();
    let ms = std::time::Duration::from_millis;
    let (pending, ready) = drag_edge_ready(None, 42, DragEdge::Right, now);
    assert!(!ready);
    assert_eq!(
        drag_edge_wait(pending, MouseWorkspaceCooldowns::default(), now, None),
        Some(ms(220))
    );
    assert_eq!(
        drag_edge_wait(
            pending,
            MouseWorkspaceCooldowns::default(),
            now + ms(110),
            Some(ms(400))
        ),
        Some(ms(110))
    );
    assert_eq!(
        drag_edge_wait(
            pending,
            MouseWorkspaceCooldowns::default(),
            now + ms(110),
            Some(ms(40))
        ),
        Some(ms(40))
    );
    let (pending, ready) = drag_edge_ready(pending, 42, DragEdge::Right, now + ms(219));
    assert!(!ready);
    let (_, ready) = drag_edge_ready(pending, 42, DragEdge::Right, now + ms(220));
    assert!(
        ready,
        "no additional motion event is needed when the timer expires"
    );
    let (pending, ready) = drag_edge_ready(pending, 42, DragEdge::Left, now + ms(221));
    assert!(!ready);
    let (_, ready) = drag_edge_ready(pending, 42, DragEdge::Left, now + ms(441));
    assert!(ready);
    let (vertical, ready) = drag_edge_ready(pending, 42, DragEdge::Bottom, now + ms(442));
    assert!(!ready, "changing axis must restart edge dwell");
    let (_, ready) = drag_edge_ready(vertical, 42, DragEdge::Bottom, now + ms(662));
    assert!(ready);
    let (pending, ready) = drag_edge_ready(None, 99, DragEdge::Right, now);
    assert!(!ready);
    assert_eq!(
        drag_edge_wait(pending, MouseWorkspaceCooldowns::default(), now, None),
        Some(ms(220))
    );
    assert_eq!(
        drag_edge_wait(None, MouseWorkspaceCooldowns::default(), now, None),
        None
    );
}

#[test]
fn drag_edge_respects_negative_root_origin_and_rebases_titlebar_grip() {
    let root = Rect::new(-1280, 40, 1280, 900);
    assert_eq!(
        drag_edge_at(RootPoint::new(-1279, 240), root),
        Some(DragEdge::Left)
    );
    assert_eq!(
        drag_edge_at(RootPoint::new(-1, 240), root),
        Some(DragEdge::Right)
    );
    assert_eq!(
        drag_edge_destination(RootPoint::new(-1, 240), root, DragEdge::Right, 400),
        RootPoint::new(-1256, 240)
    );
    let dest = drag_edge_destination(
        RootPoint::new(1919, 240),
        Rect::new(0, 0, 1920, 1080),
        DragEdge::Right,
        400,
    );
    assert_eq!(drag_entry_frame_x(dest, 700, DragEdge::Right), 8);
    let dest = drag_edge_destination(
        RootPoint::new(0, 240),
        Rect::new(0, 0, 1920, 1080),
        DragEdge::Left,
        400,
    );
    assert_eq!(drag_entry_frame_x(dest, 700, DragEdge::Left), 1211);
    assert_eq!(
        drag_entry_frame_x(RootPoint::new(24, 240), 40, DragEdge::Right),
        8
    );
    assert_eq!(
        drag_entry_frame_x(RootPoint::new(1895, 240), 40, DragEdge::Left),
        1871
    );
}

/// An edge transfer is still an ordinary frame move. Exercise the same pure
/// reducer that the live transfer uses so a change to move clamping or effect
/// ordering cannot silently break the arrival position or server-grab safety.
#[test]
fn edge_arrival_reducer_preserves_window_geometry_and_defers_renderer_work() {
    let root = RootRect::new(0, 0, 1920, 1080);
    let start = RootRect::new(1200, 100, 700, 400);
    let placement = PlacementState {
        mode: PlacementMode::Floating,
        current: start,
        floating_restore: start,
        resume: None,
    };
    let mut anchor = MoveAnchor::new(20, 700, 30);
    anchor.activate();
    let mut controller = interaction::build_controller(
        42,
        142,
        placement,
        root,
        root,
        ClientSizeHints::default(),
        FrameExtents::new(31, 1),
        InteractionSession::Move(MoveSession {
            client: 42,
            start_pointer: RootPoint::new(1919, 130),
            start_placement: PlacementSnapshot {
                mode: PlacementMode::Floating,
                rect: start,
            },
            anchor,
            grab_window: 142,
        }),
        std::rc::Rc::new(HashMap::new()),
    );
    let pointer = drag_edge_destination(
        RootPoint::new(1919, 130),
        Rect::new(0, 0, 1920, 1080),
        DragEdge::Right,
        400,
    );
    let entry_x = drag_entry_frame_x(pointer, start.w, DragEdge::Right);
    let synthetic_start = RootPoint::new(pointer.x - (entry_x - start.x), pointer.y);
    if let InteractionSession::Move(ref mut move_session) = controller.session {
        move_session.start_pointer = synthetic_start;
        move_session.start_placement = PlacementSnapshot {
            mode: PlacementMode::Floating,
            rect: start,
        };
    }
    let (next, plan) = interaction::step_owned(controller, FrameEvent::Motion { pointer });
    assert_eq!(
        next.placement.current,
        RootRect::new(entry_x, start.y, start.w, start.h)
    );
    assert!(
        plan.effects
            .iter()
            .any(|effect| matches!(effect, FrameEffect::MoveFrameRoot { .. }))
    );
    assert!(
        plan.effects
            .iter()
            .any(|effect| matches!(effect, FrameEffect::PaintChrome))
    );
    let (native, repaint) =
        defer_drag_arrival_paint(plan).expect("pure move effects are safe in a server grab");
    assert!(repaint);
    assert!(
        native
            .effects
            .iter()
            .any(|effect| matches!(effect, FrameEffect::MoveFrameRoot { .. }))
    );
    assert!(
        native
            .effects
            .iter()
            .any(|effect| matches!(effect, FrameEffect::NotifyClientRoot { .. }))
    );
    assert!(
        !native
            .effects
            .iter()
            .any(|effect| matches!(effect, FrameEffect::PaintChrome))
    );
}

#[test]
fn vertical_edge_arrival_keeps_pointer_in_titlebar_and_half_frame_visible() {
    let root = RootRect::new(0, 0, 1920, 1080);
    let start = RootRect::new(500, 100, 600, 400);
    let placement = PlacementState {
        mode: PlacementMode::Floating,
        current: start,
        floating_restore: start,
        resume: None,
    };
    let mut anchor = MoveAnchor::new(20, 600, 30);
    anchor.activate();
    let mut controller = interaction::build_controller(
        42,
        142,
        placement,
        root,
        root,
        ClientSizeHints::default(),
        FrameExtents::new(31, 1),
        InteractionSession::Move(MoveSession {
            client: 42,
            start_pointer: RootPoint::new(760, 0),
            start_placement: PlacementSnapshot {
                mode: PlacementMode::Floating,
                rect: start,
            },
            anchor,
            grab_window: 142,
        }),
        std::rc::Rc::new(HashMap::new()),
    );
    let pointer = drag_edge_destination(
        RootPoint::new(760, 0),
        Rect::new(0, 0, 1920, 1080),
        DragEdge::Top,
        start.h,
    );
    let entry_y = drag_entry_frame_y(pointer, DragEdge::Top);
    let synthetic_start = RootPoint::new(pointer.x, pointer.y - (entry_y - start.y));
    if let InteractionSession::Move(ref mut move_session) = controller.session {
        move_session.start_pointer = synthetic_start;
        move_session.start_placement = PlacementSnapshot {
            mode: PlacementMode::Floating,
            rect: start,
        };
    }
    let (next, commit) = interaction::step_owned(controller, FrameEvent::Motion { pointer });
    assert_eq!(
        next.placement.current,
        RootRect::new(start.x, entry_y, start.w, start.h)
    );
    assert!(pointer.y >= entry_y && pointer.y < entry_y + 31);
    assert!(entry_y + start.h / 2 <= root.y + root.h);
    let (native, repaint) = defer_drag_arrival_paint(commit).expect("vertical move native effects");
    assert!(repaint);
    assert!(
        native
            .effects
            .iter()
            .any(|effect| matches!(effect, FrameEffect::MoveFrameRoot { .. }))
    );
    assert!(
        !native
            .effects
            .iter()
            .any(|effect| matches!(effect, FrameEffect::PaintChrome))
    );
}

#[test]
fn edge_arrival_rejects_non_move_or_grab_effects_while_server_is_grabbed() {
    let mut plan = interaction::PointerCommit::plan(vec![FrameEffect::PaintChrome]);
    assert_eq!(
        defer_drag_arrival_paint(plan.clone()).map(|(_, paint)| paint),
        Some(true)
    );
    plan.effects.push(FrameEffect::LayoutInput);
    assert!(defer_drag_arrival_paint(plan).is_none());
    assert!(
        defer_drag_arrival_paint(interaction::PointerCommit::plan(vec![FrameEffect::Ungrab]))
            .is_none()
    );
}

#[test]
fn workspace_mouse_cooldowns_are_independent_per_axis() {
    let now = std::time::Instant::now();
    let ms = std::time::Duration::from_millis;
    let mut state = MouseWorkspaceCooldowns::default();
    state.record(MouseWorkspaceAxis::Horizontal, now);
    assert_eq!(DragEdge::Left.axis(), DragEdge::Right.axis());
    assert_eq!(
        state.remaining(MouseWorkspaceAxis::Horizontal, now + ms(100)),
        ms(400)
    );
    assert!(
        state
            .remaining(MouseWorkspaceAxis::Vertical, now + ms(100))
            .is_zero()
    );
    state.record(MouseWorkspaceAxis::Vertical, now + ms(200));
    assert_eq!(
        state.remaining(MouseWorkspaceAxis::Vertical, now + ms(300)),
        ms(400)
    );
    assert_eq!(
        state.remaining(MouseWorkspaceAxis::Horizontal, now + ms(300)),
        ms(200)
    );
}

#[test]
fn workspace_mouse_edge_timer_waits_for_cooldown_without_spinning() {
    let now = std::time::Instant::now();
    let ms = std::time::Duration::from_millis;
    let mut state = MouseWorkspaceCooldowns::default();
    state.record(MouseWorkspaceAxis::Horizontal, now);
    let (pending, _) = drag_edge_ready(None, 42, DragEdge::Right, now + ms(100));
    assert_eq!(
        drag_edge_wait(pending, state, now + ms(320), None),
        Some(ms(180))
    );
    assert_eq!(
        drag_edge_wait(pending, state, now + ms(500), None),
        Some(ms(0))
    );
}
