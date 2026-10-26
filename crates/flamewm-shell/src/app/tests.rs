#[test]
fn shared_wake_slot_takes_the_earliest_owner_deadline() {
    use std::time::{Duration, Instant};
    let t0 = Instant::now();
    let soon = t0 + Duration::from_millis(100);
    assert_eq!(
        super::earliest_wake([None, None, None, None, None]),
        None,
        "idle loop"
    );
    assert_eq!(
        super::earliest_wake([Some(t0), None, Some(soon), None, None]),
        Some(t0)
    );
    assert_eq!(
        super::earliest_wake([None, Some(soon), None, None, None]),
        Some(soon)
    );
    assert_eq!(
        super::earliest_wake([None, None, None, Some(soon), None]),
        Some(soon)
    );
}

#[test]
fn monitor_scoped_start_toggle_transfers_only_when_the_target_output_changes() {
    let output_a = flamewm_api::OutputId::new("A");
    let output_b = flamewm_api::OutputId::new("B");
    assert_eq!(
        super::start_toggle_action(false, None, &output_a),
        super::StartToggleAction::Open,
    );
    assert_eq!(
        super::start_toggle_action(true, Some(&output_a), &output_a),
        super::StartToggleAction::Close,
    );
    assert_eq!(
        super::start_toggle_action(true, Some(&output_a), &output_b),
        super::StartToggleAction::Transfer,
    );
    assert_eq!(
        super::start_toggle_action(true, None, &output_b),
        super::StartToggleAction::Transfer,
    );
}

#[test]
fn start_launch_result_cannot_cross_the_popup_session_origin() {
    use flamewm_shell_core::popup_session::{PopupKind, PopupLease};
    let context = super::StartLaunchContext {
        invocation: 8,
        output: flamewm_api::OutputId::new("A"),
        popup_sequence: 12,
    };
    let lease_a = PopupLease {
        output: flamewm_api::OutputId::new("A"),
        kind: PopupKind::Start,
        topology_generation: 3,
        panels_revision: 6,
        sequence: 12,
    };
    let lease_b = PopupLease {
        output: flamewm_api::OutputId::new("B"),
        sequence: 13,
        ..lease_a.clone()
    };
    assert!(super::start_launch_session_matches(
        &context,
        true,
        Some(&lease_a)
    ));
    assert!(!super::start_launch_session_matches(
        &context,
        true,
        Some(&lease_b)
    ));
    assert!(!super::start_launch_session_matches(
        &context,
        false,
        Some(&lease_a)
    ));
}

#[test]
fn quick_open_ack_commits_only_the_matching_lease_and_failures_release_it() {
    use flamewm_shell::quick_controls::protocol::OpenResult;
    use flamewm_shell_core::popup_session::{PopupKind, PopupSession, PopupTarget};
    let target = || PopupTarget {
        output: flamewm_api::OutputId::new("A"),
        topology_generation: 3,
        panels_revision: 6,
    };
    let mut presented = PopupSession::default();
    let open = presented.open(PopupKind::Audio, target()).unwrap();
    assert!(presented.commit(&open));
    let sequence = presented.active().unwrap().sequence;
    assert_eq!(
        super::apply_quick_open_result(&mut presented, OpenResult::Presented(sequence)),
        super::QuickOpenResolution::Presented,
    );
    assert_eq!(presented.active().unwrap().sequence, sequence);

    for failure in [
        OpenResult::Refused(sequence),
        OpenResult::RolledBack(sequence),
        OpenResult::HostExited(sequence),
    ] {
        let mut session = PopupSession::default();
        let open = session.open(PopupKind::Audio, target()).unwrap();
        assert!(session.commit(&open));
        assert_eq!(
            super::apply_quick_open_result(&mut session, failure),
            super::QuickOpenResolution::Failed,
        );
        assert!(session.active().is_none());
    }
}

#[test]
fn stale_quick_open_result_cannot_clear_a_newer_popup_lease() {
    use flamewm_shell::quick_controls::protocol::OpenResult;
    use flamewm_shell_core::popup_session::{PopupKind, PopupSession, PopupTarget};
    let target = PopupTarget {
        output: flamewm_api::OutputId::new("A"),
        topology_generation: 3,
        panels_revision: 6,
    };
    let mut session = PopupSession::default();
    let old = session.open(PopupKind::Audio, target.clone()).unwrap();
    assert!(session.commit(&old));
    let old_sequence = session.active().unwrap().sequence;
    let next = session.open(PopupKind::Network, target).unwrap();
    assert!(session.commit(&next));
    let current = session.active().cloned().unwrap();
    assert_eq!(
        super::apply_quick_open_result(&mut session, OpenResult::Refused(old_sequence)),
        super::QuickOpenResolution::Stale,
    );
    assert_eq!(session.active(), Some(&current));
}

#[test]
fn control_output_prefers_keyboard_focus_and_rejects_invalid_explicit_targets() {
    use flamewm_api::OutputId;
    let a = OutputId::new("A");
    let b = OutputId::new("B");
    let panel_outputs = [a.clone(), b.clone()];
    let connected = panel_outputs.clone();
    let primary = [a.clone()];
    assert_eq!(
        super::resolve_control_output(
            None,
            &panel_outputs,
            &connected,
            Some(&b),
            Some(&a),
            &primary
        ),
        Some(b.clone()),
        "global keyboard action follows focused window before pointer history"
    );
    assert_eq!(
        super::resolve_control_output(
            Some(&OutputId::new("gone")),
            &panel_outputs,
            &connected,
            Some(&b),
            Some(&a),
            &primary
        ),
        None,
        "an explicit stale target must not fall back to another panel"
    );
    assert_eq!(
        super::resolve_control_output(None, &panel_outputs, &connected, None, Some(&b), &primary),
        Some(b),
        "pointer is the fallback when no window is focused"
    );
}

#[test]
fn interaction_context_rejects_output_and_snapshot_drift() {
    let a = flamewm_api::OutputId::new("A");
    let b = flamewm_api::OutputId::new("B");
    assert!(super::interaction_target_is_current(
        Some(&b),
        &b,
        7,
        11,
        7,
        11
    ));
    assert!(!super::interaction_target_is_current(
        Some(&a),
        &b,
        7,
        11,
        7,
        11
    ));
    assert!(!super::interaction_target_is_current(
        Some(&b),
        &b,
        6,
        11,
        7,
        11
    ));
    assert!(!super::interaction_target_is_current(
        Some(&b),
        &b,
        7,
        10,
        7,
        11
    ));
}

fn production_source() -> &'static str {
    include_str!("../app.rs")
        .split_once("\n#[cfg(test)]\nmod tests")
        .map(|(source, _)| source)
        .expect("app tests must have a production section")
}

#[test]
fn open_media_popup_closes_when_media_unavailable_or_idle() {
    use flamewm_api::system::{PlaybackState, ServiceAvailability, SystemSnapshot};
    let mut system = SystemSnapshot::default();
    system.media.availability = ServiceAvailability::Unavailable;
    system.media.playback = PlaybackState::Playing;
    assert!(
        super::media_popup_must_close(&system, true),
        "player gone must close the open media popup"
    );
    system.media.availability = ServiceAvailability::Available;
    system.media.playback = PlaybackState::Unavailable;
    assert!(
        super::media_popup_must_close(&system, true),
        "no playing/paused media must close the open media popup"
    );
    system.media.playback = PlaybackState::Playing;
    assert!(
        !super::media_popup_must_close(&system, true),
        "playing media keeps the popup open"
    );
    assert!(
        !super::media_popup_must_close(&system, false),
        "closed popup is never closed again"
    );
}

#[test]
fn panel_projection_needed_true_for_system_only_change() {
    let changed = flamewm_shell::runtime::ChangedDomains {
        system: true,
        ..Default::default()
    };
    assert!(super::panel_projection_needed(changed, false));
    assert!(super::panel_projection_needed(changed, true));
}

#[test]
fn panel_projection_needed_panels_only_when_not_resynced() {
    let changed = flamewm_shell::runtime::ChangedDomains {
        panels: true,
        ..Default::default()
    };
    assert!(super::panel_projection_needed(changed, false));
    assert!(!super::panel_projection_needed(changed, true));
}

#[test]
fn panel_projection_needed_false_without_projection_domains() {
    let changed = flamewm_shell::runtime::ChangedDomains {
        applications: true,
        ..Default::default()
    };
    assert!(!super::panel_projection_needed(changed, false));
}

#[test]
fn start_slide_pass_also_arms_popup_slide_without_start_button_target() {
    let source = production_source();
    let body = source
        .split_once("fn apply_start_slide(")
        .and_then(|(_, rest)| rest.split_once("\n    }\n"))
        .map(|(body, _)| body)
        .expect("apply_start_slide must exist");
    assert!(
        body.contains("self.surfaces.set_popup_slide(popup_slide);"),
        "apply_start_slide must arm the popup slide on every Start-slide refresh"
    );
    assert!(
        body.contains("crate::entry::settings_projection::start_slide(snapshot, self.surfaces.panel_edge(), None)"),
        "popup slide must use the shared start_slide owner with no Magic Lamp target"
    );
}

#[test]
fn media_node_actions_route_to_active_player_with_revision() {
    let media = flamewm_api::system::MediaSnapshot {
        active_bus_name: "org.mpris.MediaPlayer2.test".to_owned(),
        ..Default::default()
    };
    let bus = "org.mpris.MediaPlayer2.test";
    let cases = [
        (
            "previous",
            flamewm_api::system::SystemAction::Previous {
                bus_name: bus.to_owned(),
            },
        ),
        (
            "playpause",
            flamewm_api::system::SystemAction::PlayPause {
                bus_name: bus.to_owned(),
            },
        ),
        (
            "next",
            flamewm_api::system::SystemAction::Next {
                bus_name: bus.to_owned(),
            },
        ),
    ];
    for (command, expected) in cases {
        match super::media_system_request(&media, 42, command) {
            Some(super::ControlRequest::SystemAction {
                action,
                expected_revision,
            }) => {
                assert_eq!(format!("{action:?}"), format!("{expected:?}"), "{command}");
                assert_eq!(expected_revision, 42, "{command}");
            }
            _ => panic!("media.{command} must produce a SystemAction request"),
        }
    }
}

#[test]
fn media_node_actions_without_player_or_unknown_command_are_dropped() {
    let no_player = flamewm_api::system::MediaSnapshot::default();
    for command in ["previous", "playpause", "next"] {
        assert!(
            super::media_system_request(&no_player, 1, command).is_none(),
            "media.{command} without an active player must be dropped"
        );
    }
    let media = flamewm_api::system::MediaSnapshot {
        active_bus_name: "org.mpris.MediaPlayer2.test".to_owned(),
        ..Default::default()
    };
    assert!(super::media_system_request(&media, 1, "stop").is_none());
}

#[test]
fn pager_cell_rect_follows_panel_resize_scale() {
    // Panel origin at (0, 1000) on a 1080 px output: 44 px bar at 100%,
    // 55 px bar at 125%. Cell 1 sits at window y=6 in both cases.
    let origin = (0.0, 1000.0);
    assert_eq!(
        super::pager_cell_window_rect((1166.0, 1006.0, 22.0, 16.0), origin),
        [1166, 6, 22, 16]
    );
    // 125% layout: cell y and height scale by 1.25, rounded.
    assert_eq!(
        super::pager_cell_window_rect((1457.5, 1007.5, 27.5, 20.0), origin),
        [1458, 8, 28, 20]
    );
    // Negative offset clamps to zero.
    assert_eq!(
        super::pager_cell_window_rect((5.0, 990.0, 10.0, 10.0), origin),
        [5, 0, 10, 10]
    );
}

#[test]
fn stale_start_icons_project_once_while_open() {
    // A theme change projects the open Start once; later unrelated refreshes do not.
    let mut stale = true;
    let mut projections = 0;
    for applications_changed in [false, false, true, false] {
        if super::start_icon_refresh_due(&mut stale, applications_changed, true) {
            projections += 1;
        }
    }
    assert_eq!(
        projections, 2,
        "one theme projection plus one app-set projection"
    );
    assert!(!stale, "the theme refresh consumes the stale flag");

    // A hidden Start keeps the flag until it is next projected on open.
    let mut hidden_stale = true;
    assert!(!super::start_icon_refresh_due(
        &mut hidden_stale,
        true,
        false
    ));
    assert!(hidden_stale);
}

#[test]
fn escape_close_clears_start_and_transient_state() {
    let mut start_open = true;
    let mut start_query = "fire".to_owned();
    let mut context_menu = Some(flamewm_shell::ContextMenuState {
        kind: flamewm_shell::ContextMenuKind::Workspace { index: 0 },
        revision: 1,
        output: None,
        interaction: None,
        anchor: None,
    });

    super::clear_transient_state_fields(&mut start_open, &mut start_query, &mut context_menu);

    assert!(!start_open);
    assert!(start_query.is_empty(), "close clears the search query");
    assert!(context_menu.is_none());
}

#[test]
fn quick_popup_open_takes_and_closes_open_context_menu_only() {
    let mut context_menu = Some(flamewm_shell::ContextMenuState {
        kind: flamewm_shell::ContextMenuKind::Workspace { index: 0 },
        revision: 1,
        output: None,
        interaction: None,
        anchor: None,
    });
    assert!(super::take_context_menu_for_popup(&mut context_menu));
    assert!(context_menu.is_none(), "popup open releases the menu slot");
    assert!(
        !super::take_context_menu_for_popup(&mut context_menu),
        "no surface close is issued when no context menu is open"
    );
}

#[test]
fn red_t06_settings_toggle_has_no_follow_up_get_settings_fetch() {
    // RED(T06): the current toggle submits ApplySettings, then performs a
    // blocking UI-thread GetSettings roundtrip instead of using its result.
    let source = production_source();
    assert!(
        !source.contains("self.control.call(&ControlRequest::GetSettings"),
        "T06 RED: settings mutation must not be followed by UI GetSettings"
    );
    assert!(
        !source.contains("refresh_settings_revision"),
        "T06 RED: remove the blocking settings refresh seam"
    );
}

#[test]
fn red_t08_shell_loop_has_no_steady_state_control_client_field() {
    // RED(T08): the current ShellLoop retains a blocking ControlClient
    // solely for steady-state reconciliation.
    let source = production_source();
    let shell_loop = source
        .split_once("pub(crate) struct ShellLoop {")
        .map(|(_, rest)| rest)
        .and_then(|rest| rest.split_once("\n}\n\nimpl ShellLoop"))
        .map(|(body, _)| body)
        .expect("ShellLoop definition must remain discoverable");
    assert!(
        !shell_loop.contains("ControlClient"),
        "T08 RED: ShellLoop steady state must not own a blocking ControlClient"
    );
}
