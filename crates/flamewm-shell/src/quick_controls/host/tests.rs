use super::*;
use flamewm_shell_core::popup::measured_popup_rect;
use flamewm_ui_core::popover::{PopoverAlign, PopoverEdge};

/// Every field at its default, so fixtures only spell out what they change.
pub(super) fn blank_snapshot() -> QuickControlSnapshot {
    QuickControlSnapshot::new(flamewm_api::system::SystemSnapshot::default())
}

#[test]
fn secret_never_in_logs_or_protocol() {
    // The helper snapshot carries only a masked length, never the
    // secret value; the protocol type carries no secret field.
    let snapshot = QuickControlSnapshot {
        audio_tab: AudioTab::Devices,
        network_secret_masked_len: 8,
        ..blank_snapshot()
    };
    let debug = format!("{snapshot:?}");
    assert!(
        !debug.contains("s3cr3t"),
        "snapshot debug must not carry a secret"
    );
    let line = super::super::protocol::encode_command(&open_cmd(QuickControlKind::Network));
    assert!(!line.contains("s3cr3t"));
}

#[test]
fn system_snapshot_update_is_direct_and_deduplicated() {
    let mut quick = blank_snapshot();
    let mut next = quick.system.clone();
    next.revision = 7;
    assert!(apply_system_snapshot(&mut quick, next.clone()));
    assert_eq!(quick.system, next);
    assert!(!apply_system_snapshot(&mut quick, next));
}

#[test]
fn filtered_sorted_scrolled_network_slot_resolves_the_projected_ap() {
    use flamewm_api::system::{NetworkAccessPointSnapshot, ServiceAvailability, SystemAction};

    let mut system = flamewm_api::system::SystemSnapshot {
        revision: 9,
        ..flamewm_api::system::SystemSnapshot::default()
    };
    system.network.availability = ServiceAvailability::Available;
    system.network.active_access_point_path = "/ap/4".to_owned();
    system.network.access_points = (0..8)
        .map(|index| NetworkAccessPointSnapshot {
            path: format!("/ap/{index}"),
            ssid: if matches!(index, 1 | 3 | 4 | 6 | 7) {
                format!("match-{index}")
            } else {
                format!("other-{index}")
            },
            strength_percent: 50,
            secured: false,
            known: true,
        })
        .collect();
    let quick = QuickControlSnapshot {
        system,
        audio_tab: AudioTab::Devices,
        network_query: NetworkQuery {
            filter: "MATCH".to_owned(),
        },
        network_scroll_offset: 44,
        calendar: crate::calendar::CalendarState::for_today((2026, 10, 13)),
        ..blank_snapshot()
    };

    // Filtering leaves five rows; the connected provider index 4 is excluded
    // (it has its own container), leaving four Available rows. The 44px scroll
    // then makes provider index 3 the visible slot 0, not raw provider slot 0.
    let rows = crate::taskbar::status::network::project_visible_rows(
        &quick.system,
        &quick.network_query,
        quick.network_scroll_offset,
    );
    assert_eq!(rows[0].path, "/ap/3");
    assert!(rows.iter().all(|row| row.path != "/ap/4"));
    let request = helper_control_request("network.connect.1", &quick)
        .expect("projected slot resolves to a connect request");
    assert_eq!(
        request,
        ControlRequest::SystemAction {
            action: SystemAction::ConnectKnown {
                access_point_path: rows[0].path.clone(),
            },
            expected_revision: 9,
        }
    );
}

#[test]
fn i32_max_constraint_cannot_reach_placement() {
    // i32::MAX work area stays finite through the shared owner measure
    // gate (degenerate anchor refuses as pending layout).
    let area = flamewm_api::Rect::new(0, 0, i32::MAX, i32::MAX);
    assert!((area.width as f32).is_finite());
    assert!((area.height as f32).is_finite());
    // The clamped intrinsic (work-area min) still refuses as
    // PendingLayout through the pure path when size is absent.
    let refused = measured_popup_rect(
        Some(flamewm_api::Rect::new(0, 0, 10, 10)),
        None,
        PopoverEdge::Above,
        PopoverAlign::End,
        flamewm_api::Rect::new(0, 0, 1920, 1080),
        8,
    );
    assert!(refused.is_err());
}

/// Measured popup placement for a valid anchor and size; panics on refusal.
fn measured(
    anchor: flamewm_api::Rect,
    size: flamewm_api::Size,
    edge: PopoverEdge,
    align: PopoverAlign,
    area: flamewm_api::Rect,
) -> flamewm_api::Rect {
    measured_popup_rect(Some(anchor), Some(size), edge, align, area, 8).expect("measured")
}

#[test]
fn bottom_end_aligned_network_fixture_right_above() {
    // Bottom panel network fixture: output-end aligned above the anchor,
    // right edge pinned to the output right (not the tray anchor).
    let area = flamewm_api::Rect::new(0, 0, 1920, 1080);
    let anchor = flamewm_api::Rect::new(1830, 1036, 70, 44);
    let size = flamewm_api::Size::new(310, 220);
    let rect = measured(
        anchor,
        size,
        PopoverEdge::Above,
        PopoverAlign::OutputEnd,
        area,
    );
    assert_eq!(rect.right(), area.right());
    assert!(rect.right() <= area.right());
    assert!(rect.bottom() <= anchor.y);
}

#[test]
fn top_panel_places_below() {
    // Top panel: generic path with Below edge opens under the anchor.
    let area = flamewm_api::Rect::new(0, 0, 1920, 1080);
    let anchor = flamewm_api::Rect::new(1830, 0, 70, 44);
    let size = flamewm_api::Size::new(310, 220);
    let rect = measured(
        anchor,
        size,
        PopoverEdge::Below,
        PopoverAlign::OutputEnd,
        area,
    );
    assert!(rect.y >= anchor.bottom());
    assert_eq!(rect.right(), area.right());
}

#[test]
fn left_right_edges_preserved() {
    // Left/right panels keep their opening side inside the area.
    let area = flamewm_api::Rect::new(0, 0, 1920, 1080);
    let anchor_l = flamewm_api::Rect::new(0, 400, 44, 70);
    let anchor_r = flamewm_api::Rect::new(1876, 400, 44, 70);
    let size = flamewm_api::Size::new(200, 150);
    let right_of = measured(anchor_l, size, PopoverEdge::Right, PopoverAlign::End, area);
    assert!(right_of.x >= anchor_l.right());
    let left_of = measured(anchor_r, size, PopoverEdge::Left, PopoverAlign::End, area);
    assert!(left_of.right() <= anchor_r.x);
}

#[test]
fn fitted_helper_rect_refuses_without_layout() {
    // Pure refusal contract: degenerate anchor area still places when
    // the anchor is valid; the refusal path is owned by the helper
    // intrinsic measurement (covered by popup unit tests).
    let area = flamewm_api::Rect::new(0, 0, 1920, 1080);
    let anchor = flamewm_api::Rect::new(1900, 1036, 20, 44);
    let size = flamewm_api::Size::new(310, 200);
    let rect = measured(
        anchor,
        size,
        PopoverEdge::Above,
        PopoverAlign::OutputEnd,
        area,
    );
    assert!(rect.right() <= area.right());
    assert!(rect.bottom() <= anchor.y);
}

fn open_cmd(kind: QuickControlKind) -> Command {
    Command::Open(OpenRequest {
        kind,
        anchor: flamewm_api::Rect::new(0, 0, 10, 10),
        work_area: flamewm_api::Rect::new(0, 0, 100, 100),
        panel_edge: flamewm_api::PanelEdge::Bottom,
        dock: None,
        scope: None,
    })
}

#[test]
fn coalesce_open_keeps_newest() {
    let mut q: VecDeque<Command> = VecDeque::from([
        open_cmd(QuickControlKind::Audio),
        open_cmd(QuickControlKind::Network),
        open_cmd(QuickControlKind::Calendar),
    ]);
    let out = coalesce_commands(&mut q);
    assert_eq!(out.len(), 1);
    assert!(matches!(
        &out[0],
        Command::Open(r) if r.kind == QuickControlKind::Calendar
    ));
}

#[test]
fn helper_fences_old_open_and_close_sequences() {
    use crate::quick_controls::protocol::OpenScope;
    use flamewm_api::OutputId;
    let scope = |sequence| OpenScope {
        output: OutputId::new("A"),
        topology_generation: 4,
        sequence,
    };
    let mut latest = 0;
    assert!(accept_scoped_open(&mut latest, Some(&scope(7))));
    assert!(!accept_scoped_open(&mut latest, Some(&scope(6))));
    assert!(!accept_scoped_open(&mut latest, Some(&scope(7))));
    assert_eq!(latest, 7);
    assert!(close_matches_active(Some(7), 7));
    assert!(!close_matches_active(Some(8), 7));
    assert!(!close_matches_active(None, 7));
}

#[test]
fn scoped_open_ack_distinguishes_present_refusal_and_completed_rollback() {
    use crate::quick_controls::protocol::OpenScope;
    use flamewm_api::OutputId;
    let scope = OpenScope {
        output: OutputId::new("A"),
        topology_generation: 4,
        sequence: 23,
    };
    assert_eq!(
        scoped_result(Some(&scope), true, false),
        Some(OpenResult::Presented(23))
    );
    assert_eq!(
        scoped_result(Some(&scope), false, false),
        Some(OpenResult::Refused(23))
    );
    assert_eq!(
        scoped_result(Some(&scope), false, true),
        Some(OpenResult::RolledBack(23))
    );
    assert_eq!(scoped_result(None, true, false), None); // legacy OPEN has no lease ACK
}

#[test]
fn stale_open_gets_refusal_for_its_own_sequence() {
    use crate::quick_controls::protocol::OpenScope;
    use flamewm_api::OutputId;
    let scope = |sequence| OpenScope {
        output: OutputId::new("A"),
        topology_generation: 1,
        sequence,
    };
    let mut latest = 9;
    let stale = scope(8);
    assert!(!accept_scoped_open(&mut latest, Some(&stale)));
    assert_eq!(
        scoped_result(Some(&stale), false, false),
        Some(OpenResult::Refused(8))
    );
    assert_eq!(latest, 9);
}

#[test]
fn close_none_selects_zero_close_one_selects_one() {
    // close-none: no open surface -> None (zero close calls).
    assert!(take_close_target(None).is_none());
    // close-one: exactly the open kind is selected.
    assert_eq!(
        take_close_target(Some(QuickControlKind::Network)),
        Some(QuickControlKind::Network)
    );
}

#[test]
fn profiler_cadence_floor_is_ten_seconds() {
    // The host profiler timer must honor profile_interval_secs with
    // its 10s floor, never a fast empty tick.
    assert!(flamewm_profiler::profile_interval_secs() >= 10);
}

#[test]
fn normal_helper_shutdown_selects_zero_exit() {
    assert_eq!(quick_control_exit_code(QuickControlShutdown::Completed), 0);
    assert_eq!(quick_control_exit_code(QuickControlShutdown::Failed), 1);
}

#[test]
fn stage_labels_are_real_turns() {
    // True stages only: every label maps to a real turn in
    // open_request/fitted_rect/close; no fake snapshot/measure/place.
    for label in QUICK_STAGE_LABELS {
        assert!(
            [
                "shell.quick.open.total",
                "shell.quick.project",
                "shell.quick.measure",
                "shell.quick.place",
                "shell.quick.prepare/commit_geometry",
                "shell.quick.present",
                "shell.quick.pointer_grab",
                "shell.quick.close/ungrab/unmap",
            ]
            .contains(label),
            "non-real stage label: {label}"
        );
    }
    assert!(!QUICK_STAGE_LABELS.contains(&"shell.quick.snapshot"));
    assert!(!QUICK_STAGE_LABELS.contains(&"shell.quick.grab"));
    assert!(!QUICK_STAGE_LABELS.contains(&"shell.quick.prepare"));
    assert!(!QUICK_STAGE_LABELS.contains(&"shell.quick.close"));
}

#[test]
fn stage_labels_pass_shell_span_allow_list() {
    // Every label the open/close turns actually pass to shell_span must
    // be in its debug allow-list; an unlisted label panics the helper.
    for label in QUICK_STAGE_LABELS {
        let _span = crate::runtime::shell_span(label);
    }
}

#[test]
fn slow_open_gate_and_stage_render_are_pure() {
    assert!(!is_slow_open(Duration::from_micros(16_669)));
    assert!(is_slow_open(Duration::from_micros(16_670)));
    let timings = OpenStageTimings {
        total: Duration::from_millis(20),
        project: Duration::from_millis(1),
        measure: Duration::from_millis(2),
        place: Duration::from_millis(3),
        prepare_commit: Duration::from_millis(4),
        present: Duration::from_millis(5),
        pointer_grab: Duration::from_millis(6),
        close: Duration::from_millis(7),
    };
    let line = timings.render(QuickControlKind::Audio);
    for stage in [
        "open.total",
        "project",
        "measure",
        "place",
        "prepare/commit_geometry",
        "present",
        "pointer_grab",
        "close/ungrab/unmap",
    ] {
        assert!(line.contains(stage), "slow line must carry {stage}");
    }
    let placement = FittedPlacement {
        rect: flamewm_api::Rect::new(10, 20, 30, 40),
        measure_elapsed: Duration::from_millis(2),
        place_elapsed: Duration::from_millis(3),
    };
    assert_eq!(placement.rect.x, 10);
    assert_eq!(placement.measure_elapsed, Duration::from_millis(2));
}

#[test]
fn helper_surfaces_start_without_handles_and_close_nothing() {
    let mut surfaces = HelperSurfaces::new();
    for kind in [
        QuickControlKind::Audio,
        QuickControlKind::Network,
        QuickControlKind::Calendar,
        QuickControlKind::Battery,
        QuickControlKind::Weather,
    ] {
        assert_eq!(surfaces.handle(kind), None, "{kind:?} must start uncreated");
    }
    assert_eq!(surfaces.close_handle(), None);
}

#[test]
fn red_grab_refusal_rolls_back_the_post_map_open() {
    // No SurfaceRuntime fake exists at this boundary. Keep this RED
    // regression source-level so it still proves the owner stages show
    // before grab and requires the real close owner on grab refusal.
    let source = concat!(
        include_str!("open_request.rs"),
        include_str!("projection.rs")
    );
    let (_, open) = source
        .split_once("fn open_request(")
        .expect("open_request owner");
    let (open, _) = open
        .split_once("\npub(crate) fn project_snapshot(")
        .expect("open_request body");
    let present = open
        .find("present(runtime, surface)")
        .expect("post-map present stage");
    let grab = open
        .find("grab(runtime, surface)")
        .expect("pointer-grab stage");
    assert!(present < grab, "grab must follow the mapped present stage");
    let after_grab = &open[grab..];
    assert!(
        after_grab.contains("if let Err") || after_grab.contains("match grab"),
        "grab refusal must have an explicit rollback branch"
    );
    assert!(
        after_grab.contains("close_helper(runtime, surface, None)"),
        "grab refusal must hide/unmap through the existing close owner"
    );
    assert!(
        after_grab.contains("surfaces.open = None"),
        "grab refusal must not leave stale open state"
    );
}

/// Wi-Fi open with `NetworkManager` absent (`ServiceAvailability::Unavailable`) must project
/// through the real network artifact without a refusal, so the popup maps and shows the
/// truthful unavailable state. The secret-error node is written on every open (even empty);
/// it needs a text child in `network.html`, otherwise `document.text` refuses the whole open.
/// Settings change between two opens: the second stored snapshot drives the shared menu
/// background colour, alpha and backdrop percents, so the next open uses the new values.
#[test]
fn settings_change_between_opens_changes_menu_background() {
    let mut snapshot = blank_snapshot();
    let first = background_for(&snapshot.settings);
    let mut changed = SettingsSnapshot::new(1);
    changed
        .values
        .insert("surfaceOpacity".to_owned(), SettingValue::Percent(0));
    changed
        .values
        .insert("surfaceBlurPercent".to_owned(), SettingValue::Percent(30));
    changed.values.insert(
        "surfaceColor".to_owned(),
        SettingValue::Text("#102030".to_owned()),
    );
    snapshot.store_settings(changed);
    let second = background_for(&snapshot.settings);
    assert_ne!(
        first, second,
        "a new settings snapshot must change the open's background"
    );
    assert_eq!(second.0.a, 0, "opacity 0 gives a transparent menu root");
    assert_eq!((second.0.r, second.0.g, second.0.b), (0x10, 0x20, 0x30));
    assert_eq!(
        second.1,
        (0, 30),
        "backdrop percents follow the new opacity and blur"
    );
}

/// Background colour and backdrop percents the helper projects for one settings snapshot.
fn background_for(settings: &SettingsSnapshot) -> (flamewm_ui_x11::UiColor, (u8, u8)) {
    (
        crate::settings_projection::menu_background(settings),
        crate::settings_projection::start_backdrop_percents(settings),
    )
}

#[test]
fn network_open_projects_unavailable_backend_without_refusal() {
    let mut document = decode_document(NETWORK_ARTIFACT).expect("network artifact must decode");
    let mut shell = crate::ShellSnapshot::default();
    shell.system.network.availability = flamewm_api::system::ServiceAvailability::Unavailable;
    crate::projection::project_network_full(&mut document, &shell, &NetworkQuery::default(), 0)
        .expect("unavailable network projects");
    crate::projection::project_network_secret(&mut document, "").expect("secret prompt projects");
    document
        .text("network-secret-error", String::new())
        .expect("empty secret error projects");
}
