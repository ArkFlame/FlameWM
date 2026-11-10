use super::*;

#[test]
fn app_shortcut_worker_propagates_the_failure_and_completes() {
    // An id absent from every catalog must surface as the worker's Err, not vanish or hang.
    let missing = flamewm_api::DesktopAppId::new("flamewm-test-absent-app.desktop");
    let handle = spawn_app_shortcut(missing).expect("worker thread starts");
    let result = handle.join().expect("worker must not panic");
    assert!(result.is_err(), "absent app must fail, got {result:?}");
}

#[test]
fn monitor_scoped_interaction_origin_requires_a_registered_surface() {
    assert_eq!(resolve_interaction_origin(None), InteractionOrigin::Ignore);
    let output = flamewm_api::OutputId::new("B");
    assert_eq!(
        resolve_interaction_origin(Some(output.clone())),
        InteractionOrigin::Output(output),
    );
}

#[test]
fn stale_or_disconnected_origin_is_rejected_without_fallback() {
    let output = flamewm_api::OutputId::new("B");
    let connected = flamewm_api::OutputId::new("A");
    let outputs = [(&connected, true), (&output, false)];
    assert!(!interaction_origin_is_connected(
        &output,
        outputs.into_iter()
    ));
    assert!(!interaction_origin_is_connected(
        &flamewm_api::OutputId::new("gone"),
        [(&connected, true)].into_iter()
    ));
    assert!(interaction_origin_is_connected(
        &connected,
        [(&connected, true)].into_iter()
    ));
}

#[test]
fn task_drag_is_cancelled_when_release_origin_differs_from_pressed_task() {
    let origin = flamewm_api::OutputId::new("A");
    let other = flamewm_api::OutputId::new("B");
    let window = flamewm_api::WindowRef::new(7, 1);
    let target = flamewm_shell::runtime::TaskSlotTarget {
        id: flamewm_api::TaskEntryId::window(window),
        app_id: flamewm_api::DesktopAppId::new("org.example.editor"),
        window: Some(window),
        output: origin.clone(),
    };

    assert!(task_drag_stays_on_origin(Some(&target), &origin));
    assert!(!task_drag_stays_on_origin(Some(&target), &other));
    assert!(!task_drag_stays_on_origin(None, &origin));
    assert_eq!(task_drag_cleanup_output(Some(&target)), Some(&origin));
    assert_eq!(task_drag_cleanup_output(None), None);
    let captured = Some((2, target));
    assert!(task_drag_can_start(captured.as_ref(), 2, &origin));
    assert!(!task_drag_can_start(captured.as_ref(), 1, &origin));
    assert!(!task_drag_can_start(captured.as_ref(), 2, &other));
    assert!(!task_drag_can_start(None, 2, &origin));
}

#[test]
fn release_keeps_the_pressed_task_target_only_for_clicks_or_armed_drags() {
    // Cross-slot drag release: the drag still owns the capture.
    assert!(release_keeps_task_press(
        ActionPhase::Release,
        false,
        "task.slot.3",
        true
    ));
    // Plain release outside the pressed node with no drag clears it.
    assert!(!release_keeps_task_press(
        ActionPhase::Release,
        false,
        "task.slot.3",
        false
    ));
    // Release inside a task slot keeps it for the click path.
    assert!(release_keeps_task_press(
        ActionPhase::Release,
        true,
        "task.slot.3",
        false
    ));
    assert!(!release_keeps_task_press(
        ActionPhase::Release,
        true,
        "taskbar.surface",
        false
    ));
    assert!(release_keeps_task_press(
        ActionPhase::Press,
        false,
        "taskbar.surface",
        false
    ));
}

#[test]
fn secondary_release_without_press_capture_uses_the_slot_task() {
    let output = flamewm_api::OutputId::new("A");
    let window = flamewm_api::WindowRef::new(9, 1);
    let target = flamewm_shell::runtime::TaskSlotTarget {
        id: flamewm_api::TaskEntryId::window(window),
        app_id: flamewm_api::DesktopAppId::new("org.example.terminal"),
        window: Some(window),
        output,
    };
    // Right-click: no primary press capture exists.
    assert_eq!(
        secondary_task_target(None, 2, || Some(target.clone())),
        Some(target.clone())
    );
    // A matching capture wins over the currently shown task.
    assert_eq!(
        secondary_task_target(Some((2, target.clone())), 2, || None),
        Some(target.clone())
    );
    // A stale capture from another slot never redirects the release.
    assert_eq!(
        secondary_task_target(Some((0, target.clone())), 2, || Some(target.clone())),
        Some(target.clone())
    );
    assert_eq!(secondary_task_target(None, 2, || None), None);
}

#[test]
fn monitor_scoped_start_transfer_and_unknown_surface_are_not_fail_open() {
    let app = include_str!("../app/control.rs");
    let controls = app
        .split("ShellControl::ToggleStart {")
        .nth(1)
        .and_then(|tail| tail.split("ShellControl::CloseStart").next())
        .expect("ToggleStart control branch exists");
    let router = include_str!("route.rs");
    let route = router
        .split("pub(crate) fn route_event(")
        .nth(1)
        .and_then(|tail| tail.split("// Presses are queued").next())
        .expect("route_event origin handling exists");

    let mut missing_contracts = Vec::new();
    if !controls.contains("open_start_surface(runtime, &target_output, started)") {
        missing_contracts.push("an already-open Start must transfer to the newly requested output");
    }
    if route.contains("select_input_output(event.surface)") {
        missing_contracts.push("an unknown/stale surface must be rejected before routing actions");
    }
    if !route.contains("interaction_origin_is_connected")
        || !router.contains("source_surface: event.surface")
        || !router.contains("dispatch_with_context(action, interaction.clone())")
    {
        missing_contracts.push("pointer controls must use a connected, explicit source output");
    }

    assert!(
        missing_contracts.is_empty(),
        "missing monitor-scoped contracts: {missing_contracts:?}"
    );
}

#[test]
fn taskbar_settings_row_maps_to_general_settings_program_only() {
    assert_eq!(
        general_settings_launch("taskbar.menu.settings"),
        Some("flamewm-settings")
    );
    assert_eq!(
        general_settings_launch("taskbar.menu.task-manager"),
        Some(TASK_MANAGER_PROGRAM)
    );
    assert_eq!(general_settings_launch("taskbar.menu.new-desktop"), None);
    assert_eq!(general_settings_launch("task.menu.close"), None);
}

#[test]
fn taskbar_menu_settings_markup_action_is_the_routed_settings_action() {
    let html = include_str!("../../../../ui/shell/task-menu.html");
    assert!(html.contains("data-action=\"taskbar.menu.settings\""));
    assert!(general_settings_launch("taskbar.menu.settings").is_some());
}

#[test]
fn tray_release_while_audio_open_toggles_closed_without_reopen() {
    assert_eq!(
        outside_release_decision(TRAY_VOLUME_ACTION, true, false, false, true),
        OutsideReleaseDecision::ToggleTrayVolumeClosed
    );
}

#[test]
fn tray_release_with_no_audio_popup_falls_through_to_open() {
    assert_eq!(
        outside_release_decision(TRAY_VOLUME_ACTION, true, false, false, false),
        OutsideReleaseDecision::Ignore
    );
}

#[test]
fn non_escape_keyboard_release_never_closes_popups() {
    assert_eq!(
        outside_release_decision("keyboard.input", true, true, false, true),
        OutsideReleaseDecision::Ignore
    );
}

#[test]
fn genuine_outside_release_closes_and_inside_release_is_kept() {
    assert_eq!(
        outside_release_decision(OUTSIDE_RELEASE_ACTION, true, false, false, true),
        OutsideReleaseDecision::Close
    );
    assert_eq!(
        outside_release_decision("network.open", true, false, false, true),
        OutsideReleaseDecision::Close
    );
    assert_eq!(
        outside_release_decision("network.open", true, false, true, true),
        OutsideReleaseDecision::Ignore
    );
    assert_eq!(
        outside_release_decision("network.open", false, false, false, true),
        OutsideReleaseDecision::Ignore
    );
}

/// Taskbar middle-click: a middle release on a task slot is dropped before task activation, so it
/// neither activates nor closes the window. Primary releases on the same slot still route.
#[test]
fn middle_release_on_a_task_slot_never_activates_the_task() {
    assert!(non_primary_release_is_dropped(
        "task.slot.0",
        true,
        PointerButton::Middle
    ));
    assert!(!non_primary_release_is_dropped(
        "task.slot.0",
        true,
        PointerButton::Primary
    ));
}

#[test]
fn right_button_outside_release_reaches_dismissal_but_other_secondary_releases_drop() {
    assert!(!non_primary_release_is_dropped(
        OUTSIDE_RELEASE_ACTION,
        true,
        PointerButton::Secondary
    ));
    assert!(!non_primary_release_is_dropped(
        OUTSIDE_RELEASE_ACTION,
        true,
        PointerButton::Primary
    ));
    assert!(non_primary_release_is_dropped(
        "network.open",
        true,
        PointerButton::Secondary
    ));
    assert!(!non_primary_release_is_dropped(
        "keyboard.input",
        true,
        PointerButton::Other(0)
    ));
    assert!(!non_primary_release_is_dropped(
        OUTSIDE_RELEASE_ACTION,
        false,
        PointerButton::Secondary
    ));
}

pub(super) fn latch_for(group: &'static str, at: Instant) -> Option<DismissLatch> {
    Some(DismissLatch { group, at })
}

#[test]
fn opener_release_after_its_own_dismiss_is_consumed_for_every_popup_group() {
    let now = Instant::now();
    for (group, opener) in [
        ("start", "start.toggle"),
        ("audio", TRAY_VOLUME_ACTION),
        ("network", "network.open"),
        ("battery", "battery.open"),
        ("calendar", "clock.open"),
    ] {
        let mut latch = latch_for(group, now);
        assert!(latch_consumes_opener(&mut latch, opener, now), "{opener}");
        assert_eq!(latch, None, "{opener} consumes the latch");
    }
}

#[test]
fn closed_popup_opener_is_not_consumed_and_opens() {
    let now = Instant::now();
    let mut latch = None;
    assert!(!latch_consumes_opener(&mut latch, "start.toggle", now));
    assert_eq!(latch, None);
}

#[test]
fn other_group_opener_is_not_consumed_and_latch_survives() {
    let now = Instant::now();
    let mut latch = latch_for("start", now);
    assert!(!latch_consumes_opener(&mut latch, "network.open", now));
    assert_eq!(latch, latch_for("start", now));
}

#[test]
fn latch_expires_so_a_later_genuine_click_reopens() {
    let now = Instant::now();
    let inside = now + (DISMISS_LATCH_TTL - Duration::from_millis(1));
    let expired = now + DISMISS_LATCH_TTL + Duration::from_millis(1);
    let mut latch = latch_for("start", now);
    assert!(latch_consumes_opener(&mut latch, "start.toggle", inside));
    let mut latch = latch_for("start", now);
    assert!(!latch_consumes_opener(&mut latch, "start.toggle", expired));
    assert_eq!(latch, None);
}

/// Replays one click as the shell sees it: the outside release closes `group`
/// (recording the latch when enabled), then the same click's opener release arrives.
/// Returns whether the popup is open afterwards.
pub(super) fn replay_dismiss_then_opener(
    latch_enabled: bool,
    group: &'static str,
    opener: &str,
) -> bool {
    let at = Instant::now();
    let mut latch = latch_enabled.then_some(DismissLatch { group, at });
    let mut open = false;
    open |= !latch_consumes_opener(&mut latch, opener, at + Duration::from_millis(5));
    open
}

#[test]
fn replayed_opener_after_outside_dismiss_leaves_popup_closed() {
    for (group, opener) in [
        ("start", "start.toggle"),
        ("audio", TRAY_VOLUME_ACTION),
        ("network", "network.open"),
        ("battery", "battery.open"),
        ("calendar", "clock.open"),
        ("media", "media.open"),
        ("media", "media.toggle"),
    ] {
        assert!(!replay_dismiss_then_opener(true, group, opener), "{opener}");
    }
}

#[test]
fn opener_group_covers_quick_popups_and_ignores_other_actions() {
    assert_eq!(opener_group("start.toggle"), Some("start"));
    assert_eq!(opener_group(TRAY_VOLUME_ACTION), Some("audio"));
    assert_eq!(opener_group("network.open"), Some("network"));
    assert_eq!(opener_group("battery.open"), Some("battery"));
    assert_eq!(opener_group("clock.open"), Some("calendar"));
    assert_eq!(opener_group("media.open"), Some("media"));
    assert_eq!(opener_group("media.toggle"), Some("media"));
    assert_eq!(opener_group("start.category.apps"), None);
}

#[test]
fn observed_outside_press_latches_each_quick_group_and_its_opener_is_consumed_once() {
    let now = Instant::now();
    for (kind, opener) in [
        (QuickControlKind::Audio, TRAY_VOLUME_ACTION),
        (QuickControlKind::Network, "network.open"),
        (QuickControlKind::Calendar, "clock.open"),
        (QuickControlKind::Battery, "battery.open"),
        (QuickControlKind::Weather, "weather.open"),
    ] {
        let group = quick_popup_group(kind);
        assert_eq!(opener_group(opener), Some(group), "{opener}");
        let mut latch = Some(DismissLatch { group, at: now });
        assert!(latch_consumes_opener(&mut latch, opener, now), "{opener}");
        assert!(
            !latch_consumes_opener(&mut latch, opener, now),
            "{opener} second release opens"
        );
    }
}

#[test]
fn press_classification_treats_only_quick_popup_titles_as_inside() {
    for title in [
        "FlameWM audio",
        "FlameWM network",
        "FlameWM calendar",
        "FlameWM battery",
        "FlameWM weather",
        "FlameWM media",
    ] {
        assert!(
            !raw_press_is_outside_quick_popups(Some(title)),
            "{title} is inside"
        );
    }
    assert!(
        raw_press_is_outside_quick_popups(None),
        "no window under pointer"
    );
    assert!(
        raw_press_is_outside_quick_popups(Some("FlameWM Render 0.0.9")),
        "panel"
    );
    assert!(
        raw_press_is_outside_quick_popups(Some("FlameWM audio2")),
        "prefix is not a match"
    );
}
