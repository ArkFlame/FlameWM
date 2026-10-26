use super::*;
use flamewm_api::{PanelEdge, Rect};

fn test_output_rect() -> flamewm_api::Rect {
    flamewm_api::Rect::new(0, 0, 1920, 1080)
}

#[test]
fn floating_bottom_bar_inset_is_consistent_and_start_popover_stays_above_it() {
    let output = Rect::new(0, 0, 1350, 641);
    // Floating bottom bar: 8 px margin below it, so bottom != output.bottom().
    let floating = Rect::new(8, 589, 1334, 44);
    assert!(edge_geometry_consistent(
        PanelEdge::Bottom,
        floating,
        output,
        FLOATING_MARGIN_MAX_LOGICAL
    ));
    // A stale Top edge against floating Bottom geometry is still refused.
    assert!(!edge_geometry_consistent(
        PanelEdge::Top,
        floating,
        output,
        FLOATING_MARGIN_MAX_LOGICAL
    ));
    // A bar that does not sit on the edge is refused.
    assert!(!edge_geometry_consistent(
        PanelEdge::Bottom,
        Rect::new(8, 500, 1334, 44),
        output,
        FLOATING_MARGIN_MAX_LOGICAL
    ));

    // Start popover anchored to the floating bar's Start button lands above the bar,
    // fully inside the work area.
    let work = work_area_for_panel(output, floating, PanelEdge::Bottom);
    let anchor = Rect::new(16, 597, 32, 28);
    let placed = crate::popup_controller::fit_start(
        anchor,
        flamewm_api::Size::new(550, 349),
        PanelEdge::Bottom,
        work,
        8,
    )
    .expect("floating start placement")
    .rect;
    assert!(placed.bottom() <= anchor.y - 8);
    assert!(placed.x >= work.x && placed.right() <= work.right());
    assert!(placed.y >= work.y);
}

#[test]
fn panel_ui_scale_matches_platform_physical_thickness() {
    // 44 logical px * scale must equal the document's laid-out bar height.
    for percent in (100_u16..=200).step_by(5) {
        let scale = panel_ui_scale(percent);
        let thickness = (44.0 * scale).round() as i32;
        assert_eq!(thickness, ((44 * u32::from(percent) + 50) / 100) as i32);
    }
    assert_eq!(panel_ui_scale(111), 1.0);
}

fn test_output_id() -> flamewm_api::OutputId {
    flamewm_api::OutputId::new("HDMI-1")
}

#[test]
fn workspace_right_click_rect_is_near_pointer_not_origin() {
    let size = context_menu_size(3);
    let pointer = Some(flamewm_api::Point::new(400, 900));
    let rect = context_menu_rect(test_output_id(), test_output_rect(), pointer, size);
    assert_eq!((rect.x, rect.y), (400, 900));
    assert_eq!((rect.width, rect.height), (size.width, size.height));
}

#[test]
fn context_menu_is_clamped_inside_output() {
    let size = context_menu_size(3);
    let pointer = Some(flamewm_api::Point::new(1900, 1060));
    let rect = context_menu_rect(test_output_id(), test_output_rect(), pointer, size);
    assert_eq!(rect.width, size.width);
    assert_eq!(rect.height, size.height);
    assert!(rect.right() <= 1920);
    assert!(rect.bottom() <= 1080);
    assert!(rect.x >= 0 && rect.y >= 0);
}

#[test]
fn context_menu_size_shrinks_to_visible_rows() {
    use flamewm_ui_core::context_menu::{MENU_PADDING, ROW_HEIGHT};
    let height = |rows: i32| 2 * MENU_PADDING + rows * ROW_HEIGHT;
    assert_eq!(context_menu_size(1).height, height(1));
    assert_eq!(context_menu_size(2).height, height(2));
    assert_eq!(context_menu_size(3).height, height(3));
    assert!(context_menu_size(1).height < 180);
    assert!(context_menu_size(3).height < 180);
}

#[test]
fn absent_pointer_falls_back_to_output_origin() {
    let size = context_menu_size(2);
    let rect = context_menu_rect(test_output_id(), test_output_rect(), None, size);
    assert_eq!((rect.x, rect.y), (0, 0));
}

fn test_window(reference: flamewm_api::WindowRef) -> flamewm_api::window::WindowSnapshot {
    flamewm_api::window::WindowSnapshot {
        reference,
        title: "term".to_owned(),
        app_id: flamewm_api::DesktopAppId::new("term"),
        outer_geometry: flamewm_api::Rect::new(0, 0, 100, 100),
        restore_geometry: flamewm_api::Rect::new(0, 0, 100, 100),
        state: flamewm_api::window::WindowState::Normal,
        sticky: false,
        focused: true,
        workspace: flamewm_api::WorkspaceRef::new(0, 1),
        output: test_output_id(),
        state_generation: 1,
    }
}

// REGRESSION: the weather tray click must reach the popup opener. Without the
// "weather.open" arm, dispatch yields no control and the popup never opens.
#[test]
fn weather_tray_click_dispatches_open_popover() {
    let mut runtime = ShellRuntime::default();
    runtime.dispatch("weather.open");
    let controls: Vec<_> = runtime.drain_controls().collect();
    assert!(
        matches!(controls.as_slice(), [ShellControl::OpenPopover { name, output: None, .. }] if name == "weather"),
        "{controls:?}"
    );
}

#[test]
fn window_snapshot_signal_applies_without_fetch() {
    let mut runtime = ShellRuntime::default();
    let windows = vec![test_window(flamewm_api::WindowRef::new(1, 1))];
    runtime.mark_signal(&ControlSignal::WindowsSnapshotChanged {
        revision: 7,
        windows: windows.clone(),
    });
    assert_eq!(runtime.snapshot().windows, windows);
    let changed = runtime.reconcile_applied();
    assert!(changed.windows);
    assert!(!runtime.dirty().any());
}

#[test]
fn legacy_window_revision_signal_sets_no_flag() {
    let mut runtime = ShellRuntime::default();
    runtime.mark_signal(&ControlSignal::WindowsChanged { revision: 9 });
    runtime.mark_signal(&ControlSignal::WorkspacesChanged { revision: 3 });
    runtime.mark_signal(&ControlSignal::PanelsChanged { revision: 4 });
    runtime.mark_signal(&ControlSignal::SystemChanged { revision: 6 });
    assert!(!runtime.dirty().any());
}

#[test]
fn start_menu_toggle_signal_queues_one_toggle_start_control() {
    let mut runtime = ShellRuntime::default();
    runtime.mark_signal(&ControlSignal::StartMenuToggleRequested);
    let controls: Vec<_> = runtime.drain_controls().collect();
    assert_eq!(
        controls,
        vec![ShellControl::ToggleStart {
            output: None,
            interaction: None
        }]
    );
    assert!(!runtime.dirty().any());
}

#[test]
fn launcher_toggle_signal_queues_one_toggle_launcher_control() {
    let mut runtime = ShellRuntime::default();
    runtime.mark_signal(&ControlSignal::LauncherToggleRequested);
    let controls: Vec<_> = runtime.drain_controls().collect();
    assert_eq!(
        controls,
        vec![ShellControl::ToggleLauncher {
            output: None,
            interaction: None
        }]
    );
    assert!(!runtime.dirty().any());
}

#[test]
fn start_menu_signal_and_click_dispatch_queue_identical_controls() {
    let mut by_signal = ShellRuntime::default();
    by_signal.mark_signal(&ControlSignal::StartMenuToggleRequested);
    assert!(by_signal.has_pending_controls());
    let mut by_click = ShellRuntime::default();
    by_click.dispatch("start.toggle");
    assert!(by_click.has_pending_controls());
    let signal_controls: Vec<_> = by_signal.drain_controls().collect();
    let click_controls: Vec<_> = by_click.drain_controls().collect();
    assert_eq!(signal_controls, click_controls);
    assert!(!by_signal.has_pending_controls());
}

#[test]
fn workspace_snapshot_signal_updates_model_without_fetch() {
    let mut runtime = ShellRuntime::default();
    let snapshot = flamewm_api::workspace::WorkspaceSnapshot {
        revision: 5,
        count: 2,
        active_index: 1,
        last_index: Some(0),
        names: vec!["one".to_owned(), "two".to_owned()],
    };
    runtime.mark_signal(&ControlSignal::WorkspacesSnapshotChanged {
        snapshot: snapshot.clone(),
    });
    assert_eq!(runtime.snapshot().workspaces, Some(snapshot));
    let changed = runtime.reconcile_applied();
    assert!(changed.workspaces);
    assert!(!runtime.dirty().any());
}

#[test]
fn panel_snapshot_signal_updates_task_model_without_fetch() {
    let mut runtime = ShellRuntime::default();
    let window = flamewm_api::WindowRef::new(2, 1);
    let snapshot = flamewm_api::panels::PanelsSnapshot {
        revision: 6,
        panels: Vec::new(),
        tasks: vec![flamewm_api::panels::TaskEntry {
            id: flamewm_api::TaskEntryId::window(window),
            app_id: flamewm_api::DesktopAppId::new("term"),
            kind: flamewm_api::panels::TaskEntryKind::Window { window },
            order_index: 0,
        }],
        pinned_apps: Vec::new(),
    };
    runtime.mark_signal(&ControlSignal::PanelsSnapshotChanged {
        snapshot: snapshot.clone(),
    });
    assert_eq!(runtime.snapshot().panels, snapshot);
    let changed = runtime.reconcile_applied();
    assert!(changed.panels);
    assert!(!runtime.dirty().any());
}

#[test]
fn system_snapshot_signal_updates_model_without_fetch() {
    let mut runtime = ShellRuntime::default();
    let mut snapshot = runtime.snapshot().system.clone();
    snapshot.revision = 9;
    runtime.mark_signal(&ControlSignal::SystemSnapshotChanged {
        snapshot: snapshot.clone(),
    });
    assert_eq!(runtime.snapshot().system, snapshot);
    let changed = runtime.reconcile_dirty();
    assert!(changed.system);
    assert!(!runtime.dirty().any());
}

#[test]
fn identical_snapshot_signal_sets_no_flag() {
    let mut runtime = ShellRuntime::default();
    let windows = runtime.snapshot().windows.clone();
    runtime.mark_signal(&ControlSignal::WindowsSnapshotChanged {
        revision: 1,
        windows,
    });
    assert!(!runtime.dirty().any());
}

#[test]
fn red_t08_steady_state_has_no_blocking_get_system_reconciliation() {
    // RED(T08): current steady state still enters reconcile_dirty with a
    // ControlClient and calls GetSystem from the shell tick.
    let source = include_str!("../runtime.rs")
        .split_once("\n#[cfg(test)]\nmod tests")
        .map(|(source, _)| source)
        .expect("runtime tests must have a production section");
    assert!(
        !source.contains("pub fn reconcile_dirty(&mut self, client: &ControlClient)"),
        "T08 RED: shell runtime must not expose blocking ControlClient reconciliation"
    );
    assert!(
        !source.contains("client.call(&ControlRequest::GetSystem)"),
        "T08 RED: steady-state shell updates must not fetch GetSystem"
    );
}

fn panel_output(id: &str, geometry: flamewm_api::Rect) -> flamewm_api::display::OutputSnapshot {
    flamewm_api::display::OutputSnapshot {
        id: flamewm_api::OutputId::new(id),
        connector: id.to_owned(),
        edid_identity: String::new(),
        connected: true,
        primary: true,
        geometry,
        current_mode: flamewm_api::ModeId(1),
        modes: Vec::new(),
        shell_scale_percent: 100,
    }
}

#[test]
fn root_pointer_selects_negative_origin_output_with_half_open_bounds() {
    let outputs = vec![
        panel_output("A", flamewm_api::Rect::new(0, 0, 1920, 1080)),
        panel_output("B", flamewm_api::Rect::new(-1280, 100, 1280, 720)),
    ];
    assert_eq!(
        output_at_root_point(&outputs, flamewm_api::Point::new(-640, 500)),
        Some(flamewm_api::OutputId::new("B")),
    );
    assert_eq!(
        output_at_root_point(&outputs, flamewm_api::Point::new(0, 500)),
        Some(flamewm_api::OutputId::new("A")),
    );
    assert_eq!(
        output_at_root_point(&outputs, flamewm_api::Point::new(-1281, 500)),
        None
    );
}

fn panel_entry(
    output: &str,
    edge: flamewm_api::PanelEdge,
    geometry: flamewm_api::Rect,
) -> flamewm_api::panels::PanelSnapshot {
    flamewm_api::panels::PanelSnapshot {
        output: flamewm_api::OutputId::new(output),
        edge,
        logical_size: 44,
        visible: true,
        geometry,
    }
}

fn bottom_panel_snapshot(generation: u64, revision: u64) -> ShellSnapshot {
    placement_snapshot(
        flamewm_api::Rect::new(0, 0, 1920, 1080),
        flamewm_api::Rect::new(0, 1036, 1920, 44),
        PanelEdge::Bottom,
        generation,
        revision,
    )
}

fn placement_snapshot(
    output_geo: flamewm_api::Rect,
    panel_geo: flamewm_api::Rect,
    edge: flamewm_api::PanelEdge,
    generation: u64,
    revision: u64,
) -> ShellSnapshot {
    let mut snapshot = ShellSnapshot::default();
    snapshot.displays.generation = generation;
    snapshot.displays.outputs = vec![panel_output("HDMI-1", output_geo)];
    snapshot.panels.revision = revision;
    snapshot.panels.panels = vec![panel_entry("HDMI-1", edge, panel_geo)];
    snapshot
}

#[test]
fn panel_context_top_to_bottom_recompute() {
    let top = placement_snapshot(
        flamewm_api::Rect::new(0, 0, 1920, 1080),
        flamewm_api::Rect::new(0, 0, 1920, 44),
        PanelEdge::Top,
        1,
        1,
    );
    let ctx = panel_context_for_snapshot(&top, PanelEdge::Top).expect("top context");
    assert_eq!(ctx.work_area, flamewm_api::Rect::new(0, 44, 1920, 1036));
    assert_eq!((ctx.display_generation, ctx.panels_revision), (1, 1));
    // Mixed edge rejected: Bottom snapshot against Top expectation.
    let bottom = bottom_panel_snapshot(2, 2);
    assert!(panel_context_for_snapshot(&bottom, PanelEdge::Top).is_none());
    let ctx2 = panel_context_for_snapshot(&bottom, PanelEdge::Bottom).expect("bottom context");
    assert_eq!(ctx2.work_area, flamewm_api::Rect::new(0, 0, 1920, 1036));
}

#[test]
fn panel_context_bottom_excludes_panel_and_starts_above() {
    let snapshot = bottom_panel_snapshot(3, 5);
    let ctx = panel_context_for_snapshot(&snapshot, PanelEdge::Bottom).expect("context");
    assert_eq!(ctx.work_area.bottom(), 1036);
    assert!(ctx.work_area.height < 1080);
    // Start anchor (static layout) sits at the panel; the layout
    // popover must lie inside the work area, not overlap the panel.
    assert_eq!(ctx.start_layout.anchor.bottom(), 1080);
    assert!(ctx.start_layout.popover.bottom() <= ctx.work_area.bottom());
}

#[test]
fn panel_context_quick_anchor_above_and_negative_origins() {
    use flamewm_shell_core::popup::fitted_start_placement;
    let output = flamewm_api::Rect::new(-1920, 0, 1920, 1080);
    let snapshot = placement_snapshot(
        output,
        flamewm_api::Rect::new(-1920, 1036, 1920, 44),
        PanelEdge::Bottom,
        4,
        6,
    );
    let ctx = panel_context_for_snapshot(&snapshot, PanelEdge::Bottom).expect("context");
    assert_eq!(ctx.work_area, flamewm_api::Rect::new(-1920, 0, 1920, 1036));
    // Quick anchor above the panel: end-aligned popover stays inside
    // the work area and above the panel top.
    let anchor = flamewm_api::Rect::new(-100, 1036, 30, 44);
    let placed = fitted_start_placement(
        anchor,
        flamewm_api::Size::new(310, 200),
        PanelEdge::Bottom,
        ctx.work_area,
        8,
    )
    .expect("placed");
    assert!(placed.rect.bottom() <= anchor.y - 8);
    assert!(placed.rect.x >= ctx.work_area.x);
}

#[test]
fn panel_context_hidpi_panel_rect_used_as_is() {
    let snapshot = placement_snapshot(
        flamewm_api::Rect::new(0, 0, 3840, 2160),
        flamewm_api::Rect::new(0, 2072, 3840, 88),
        PanelEdge::Bottom,
        7,
        8,
    );
    let ctx = panel_context_for_snapshot(&snapshot, PanelEdge::Bottom).expect("context");
    assert_eq!(ctx.work_area, flamewm_api::Rect::new(0, 0, 3840, 2072));
}

#[test]
fn f10_top_to_bottom_resync_uses_bottom() {
    use flamewm_shell_core::popup::work_area_for_panel;
    let output = flamewm_api::Rect::new(0, 0, 1920, 1080);
    let top = placement_snapshot(
        output,
        flamewm_api::Rect::new(0, 0, 1920, 44),
        PanelEdge::Top,
        1,
        1,
    );
    let initial = panel_context_for_snapshot(&top, PanelEdge::Top).expect("top context");
    assert_eq!(initial.panel.edge, PanelEdge::Top);
    let bottom = bottom_panel_snapshot(2, 2);
    // Next Start/Quick resolve through the synced resolver: stored Top
    // misses the gated path, the geometry-driven fallback returns the
    // Bottom context with Bottom generation/revision/work area.
    let synced =
        synced_placement_for_snapshot(&bottom, initial.panel.edge).expect("resync to bottom");
    assert_eq!(synced.panel.edge, PanelEdge::Bottom);
    assert_eq!((synced.display_generation, synced.panels_revision), (2, 2));
    assert_eq!(
        synced.work_area,
        work_area_for_panel(output, synced.panel.geometry, PanelEdge::Bottom)
    );
    assert_eq!(synced.work_area, flamewm_api::Rect::new(0, 0, 1920, 1036));
    // Gate the open paths would check: stale Top edge against the new
    // Bottom geometry is inconsistent; synced Bottom is consistent.
    assert!(!edge_geometry_consistent(
        PanelEdge::Top,
        synced.panel.geometry,
        synced.output.geometry,
        FLOATING_MARGIN_MAX_LOGICAL
    ));
    assert!(edge_geometry_consistent(
        PanelEdge::Bottom,
        synced.panel.geometry,
        synced.output.geometry,
        0
    ));
}

#[test]
fn panel_context_missing_output_returns_none() {
    let mut snapshot = bottom_panel_snapshot(1, 1);
    snapshot.displays.outputs.clear();
    assert!(panel_context_for_snapshot(&snapshot, PanelEdge::Bottom).is_none());
    let mut empty = ShellSnapshot::default();
    empty.displays.outputs = vec![panel_output("HDMI-1", test_output_rect())];
    assert!(panel_context_for_snapshot(&empty, PanelEdge::Bottom).is_none());
}

/// Two panels (A shown first, then B) with tasks t1 and t2 on the windows below.
fn task_slot_snapshot(outputs: &[&str]) -> ShellSnapshot {
    use flamewm_api::panels::{PanelSnapshot, TaskEntry, TaskEntryKind};
    use flamewm_api::window::WindowState;
    use flamewm_api::{DesktopAppId, OutputId, PanelEdge, TaskEntryId, WindowRef, WorkspaceRef};
    let window_on = |id: u64, output: &str| WindowSnapshot {
        reference: WindowRef::new(id, 1),
        title: String::new(),
        app_id: DesktopAppId::new(if output == "B" { "konsole" } else { "dolphin" }),
        outer_geometry: flamewm_api::Rect::new(0, 0, 10, 10),
        restore_geometry: flamewm_api::Rect::new(0, 0, 10, 10),
        state: WindowState::Normal,
        sticky: false,
        focused: false,
        workspace: WorkspaceRef {
            index: 0,
            revision: 1,
        },
        output: OutputId::new(output),
        state_generation: 1,
    };
    let task = |id: u64, output: &str| {
        let window = WindowRef::new(id, 1);
        TaskEntry {
            id: TaskEntryId::window(window),
            app_id: DesktopAppId::new(if output == "B" { "konsole" } else { "dolphin" }),
            kind: TaskEntryKind::Window { window },
            order_index: id as usize,
        }
    };
    let panel = |output: &str| PanelSnapshot {
        output: OutputId::new(output),
        edge: PanelEdge::Bottom,
        logical_size: 40,
        visible: true,
        geometry: if output == "B" {
            flamewm_api::Rect::new(-1280, 776, 1280, 44)
        } else {
            flamewm_api::Rect::new(0, 1036, 1920, 44)
        },
    };
    let mut snapshot = ShellSnapshot::default();
    let ids: Vec<u64> = (1..=outputs.len() as u64).collect();
    snapshot.panels = PanelsSnapshot {
        revision: 7,
        panels: vec![panel("A"), panel("B")],
        tasks: ids
            .iter()
            .zip(outputs)
            .map(|(&id, output)| task(id, output))
            .collect(),
        pinned_apps: Vec::new(),
    };
    snapshot.displays = DisplaySnapshot {
        generation: 1,
        outputs: vec![
            panel_output("A", flamewm_api::Rect::new(0, 0, 1920, 1080)),
            panel_output("B", flamewm_api::Rect::new(-1280, 100, 1280, 720)),
        ],
        pending: None,
    };
    snapshot.windows = ids
        .iter()
        .zip(outputs)
        .map(|(&id, output)| window_on(id, output))
        .collect();
    snapshot
}

fn win_ref(id: u64) -> flamewm_api::WindowRef {
    flamewm_api::WindowRef::new(id, 1)
}

fn activated(id: u64) -> Option<Vec<ControlRequest>> {
    Some(vec![ControlRequest::ActivateWindow(win_ref(id))])
}

#[test]
fn task_slot_click_maps_to_the_task_on_the_shown_panel_output() {
    // Window 1 on A: slot 0 of the shown panel A is window 1; B holds window 2 only.
    let mut runtime = ShellRuntime::new(task_slot_snapshot(&["A", "B"]));
    runtime.select_panel_output(flamewm_api::OutputId::new("A"));
    assert_eq!(runtime.task_click_requests_for_slot(0), activated(1));
    assert_eq!(runtime.task_click_requests_for_slot(1), None);
    assert_eq!(
        runtime.task_target_for_slot(0).unwrap().app_id.as_str(),
        "dolphin"
    );

    // Window 1 moves to B: panel A now shows nothing, so slot 0 no longer maps to window 1.
    runtime = ShellRuntime::new(task_slot_snapshot(&["B", "B"]));
    runtime.select_panel_output(flamewm_api::OutputId::new("A"));
    assert_eq!(runtime.task_click_requests_for_slot(0), None);
    // Panel B shown first: slot 0 is window 1, slot 1 is window 2.
    let mut snapshot = task_slot_snapshot(&["B", "B"]);
    snapshot.panels.panels.reverse();
    runtime = ShellRuntime::new(snapshot);
    runtime.select_panel_output(flamewm_api::OutputId::new("B"));
    assert_eq!(runtime.task_click_requests_for_slot(0), activated(1));
    assert_eq!(runtime.task_click_requests_for_slot(1), activated(2));
}

#[test]
fn task_slot_actions_follow_the_output_that_received_the_input() {
    let mut runtime = ShellRuntime::new(task_slot_snapshot(&["A", "B", "B"]));
    let output = flamewm_api::OutputId::new("B");
    runtime.select_panel_output(output.clone());
    assert_eq!(runtime.task_click_requests_for_slot(0), activated(2));
    assert_eq!(runtime.task_click_requests_for_slot(1), activated(3));
    let first = runtime.task_target_for_slot(0).unwrap();
    let second = runtime.task_target_for_slot(1).unwrap();
    assert_eq!(first.app_id.as_str(), "konsole");
    assert_eq!(second.app_id.as_str(), "konsole");
    assert_eq!(first.window, Some(win_ref(2)));
    assert_eq!(second.window, Some(win_ref(3)));
    assert_eq!(runtime.task_click_requests_for_target(&first), activated(2));
    runtime.open_task_context_for_target(&second, None);
    assert!(matches!(
        runtime.controls.back(),
        Some(ShellControl::OpenContextMenu(state))
            if state.kind == ContextMenuKind::Task(second.id.clone())
                && state.output.as_ref() == Some(&output)
    ));

    let _: Vec<_> = runtime.drain_controls().collect();
    runtime.open_task_context_at_slot(0, None);
    match runtime.controls.front() {
        Some(ShellControl::OpenContextMenu(state)) => {
            assert_eq!(
                state.kind,
                ContextMenuKind::Task(flamewm_api::TaskEntryId::window(win_ref(2)))
            );
            assert_eq!(state.output.as_ref(), Some(&output));
        }
        other => panic!("expected second-output task context, got {other:?}"),
    }
}

#[test]
fn running_task_context_menu_pins_and_pinned_menu_unpins() {
    let context_menu = |kind, output| ContextMenuState {
        kind,
        revision: 7,
        output: Some(output),
        interaction: None,
        anchor: None,
    };
    let mut runtime = ShellRuntime::new(task_slot_snapshot(&["A"]));
    runtime.select_panel_output(flamewm_api::OutputId::new("A"));
    let target = runtime.task_target_for_slot(0).unwrap();
    let menu = context_menu(
        ContextMenuKind::Task(target.id.clone()),
        target.output.clone(),
    );
    assert_eq!(
        runtime.context_menu_request_for_action("task.menu.pin", Some(&menu)),
        Some(ControlRequest::PinApp {
            app: target.app_id.clone(),
            expected_revision: 7,
        })
    );

    let mut pinned = task_slot_snapshot(&["A"]);
    pinned.panels.tasks[0].id = flamewm_api::TaskEntryId::pinned(&target.app_id);
    pinned.panels.tasks[0].kind = flamewm_api::panels::TaskEntryKind::PinnedSlot {
        window: Some(win_ref(1)),
    };
    pinned.panels.pinned_apps = vec![target.app_id.clone()];
    let mut runtime = ShellRuntime::new(pinned);
    runtime.select_panel_output(flamewm_api::OutputId::new("A"));
    let target = runtime.task_target_for_slot(0).unwrap();
    let menu = context_menu(
        ContextMenuKind::Task(target.id.clone()),
        target.output.clone(),
    );
    assert_eq!(
        runtime.context_menu_request_for_action("task.menu.unpin", Some(&menu)),
        Some(ControlRequest::UnpinApp {
            app: target.app_id,
            expected_revision: 7,
        })
    );
}

#[test]
fn unbound_task_actions_do_not_fall_back_to_the_first_panel() {
    let runtime = ShellRuntime::new(task_slot_snapshot(&["A", "B"]));
    assert_eq!(runtime.task_target_for_slot(0), None);
    assert_eq!(runtime.task_click_requests_for_slot(0), None);
    assert_eq!(runtime.task_slot_count(), 0);
    assert_eq!(runtime.task_reorder_request(0, 1), None);
    assert_eq!(
        runtime
            .task_target_for_slot_on_output(0, &flamewm_api::OutputId::new("B"))
            .and_then(|target| target.window),
        Some(win_ref(2)),
        "a source-resolved output still identifies its own task slot"
    );
}

#[test]
fn stale_task_slot_release_cannot_retarget_after_projection_refresh() {
    let mut initial = task_slot_snapshot(&["A", "B"]);
    let app = flamewm_api::DesktopAppId::new("konsole");
    initial.panels.tasks[1].id = flamewm_api::TaskEntryId::pinned(&app);
    initial.panels.tasks[1].kind = flamewm_api::panels::TaskEntryKind::PinnedSlot {
        window: Some(win_ref(2)),
    };
    let mut runtime = ShellRuntime::new(initial);
    runtime.select_panel_output(flamewm_api::OutputId::new("B"));
    let pressed = runtime.task_target_for_slot(0).unwrap();
    assert_eq!(pressed.window, Some(win_ref(2)));

    let mut same_task = task_slot_snapshot(&["A", "B"]);
    same_task.panels.tasks[1].id = flamewm_api::TaskEntryId::pinned(&app);
    same_task.panels.tasks[1].kind = flamewm_api::panels::TaskEntryKind::PinnedSlot {
        window: Some(win_ref(2)),
    };
    same_task.windows[1].focused = true;
    runtime = ShellRuntime::new(same_task);
    runtime.select_panel_output(flamewm_api::OutputId::new("B"));
    assert_eq!(
        runtime.task_click_requests_for_target(&pressed),
        Some(vec![ControlRequest::MinimizeWindow(win_ref(2))])
    );

    let mut refreshed = task_slot_snapshot(&["A", "B"]);
    refreshed.panels.tasks[1].id = flamewm_api::TaskEntryId::pinned(&app);
    refreshed.panels.tasks[1].kind = flamewm_api::panels::TaskEntryKind::PinnedSlot {
        window: Some(win_ref(3)),
    };
    refreshed.panels.tasks[1].order_index = 3;
    refreshed.windows[1].reference = win_ref(3);
    refreshed.windows[1].state_generation = 1;
    runtime = ShellRuntime::new(refreshed);
    runtime.select_panel_output(flamewm_api::OutputId::new("B"));
    assert_eq!(runtime.task_click_requests_for_target(&pressed), None);
    assert_eq!(runtime.task_click_requests_for_slot(0), activated(3));
    let _: Vec<_> = runtime.drain_controls().collect();
    runtime.open_task_context_for_target(&pressed, None);
    assert!(
        runtime.controls.is_empty(),
        "stale secondary release must not open another task's menu"
    );
}

#[test]
fn task_reorder_uses_captured_identity_and_current_output_slots() {
    let mut runtime = ShellRuntime::new(task_slot_snapshot(&["A", "B", "B"]));
    runtime.select_panel_output(flamewm_api::OutputId::new("B"));
    let source = runtime.task_target_for_slot(0).unwrap();
    assert_eq!(
        runtime.task_reorder_request_for_target(&source, 1),
        Some(ControlRequest::ReorderTask {
            entry_id: flamewm_api::TaskEntryId::window(win_ref(2)),
            index: 2,
            expected_revision: 7,
        })
    );

    let mut changed = task_slot_snapshot(&["A", "B", "B"]);
    changed.panels.tasks[1].kind = flamewm_api::panels::TaskEntryKind::PinnedSlot {
        window: Some(win_ref(3)),
    };
    changed.panels.tasks[1].id = source.id.clone();
    changed.panels.tasks[1].app_id = source.app_id.clone();
    changed.windows[1].reference = win_ref(3);
    runtime = ShellRuntime::new(changed);
    runtime.select_panel_output(source.output.clone());
    assert_eq!(runtime.task_reorder_request_for_target(&source, 1), None);
}

#[test]
fn popup_openers_capture_explicit_output_across_selection_changes() {
    let mut runtime = ShellRuntime::new(task_slot_snapshot(&["A", "B"]));
    let previous_selection = flamewm_api::OutputId::new("A");
    let output = flamewm_api::OutputId::new("B");
    runtime.select_panel_output(previous_selection.clone());
    for action in [
        "start.toggle",
        "volume.open",
        "network.open",
        "battery.open",
        "clock.open",
        "weather.open",
        "media.open",
    ] {
        runtime.dispatch_with_output(action, Some(output.clone()));
        runtime.select_panel_output(previous_selection.clone());
    }
    let controls: Vec<_> = runtime.drain_controls().collect();
    assert!(matches!(
        controls.first(),
        Some(ShellControl::ToggleStart { output: Some(target), .. }) if target == &output
    ));
    assert_eq!(controls.len(), 7);
    for control in &controls[1..] {
        assert!(matches!(
            control,
            ShellControl::OpenPopover { output: Some(target), .. } if target == &output
        ));
    }
}

#[test]
fn global_start_dispatch_does_not_capture_ambient_panel_selection() {
    let mut runtime = ShellRuntime::new(task_slot_snapshot(&["A", "B"]));
    runtime.select_panel_output(flamewm_api::OutputId::new("A"));
    runtime.dispatch("start.toggle");
    assert!(matches!(
        runtime.drain_controls().next(),
        Some(ShellControl::ToggleStart { output: None, .. })
    ));
}

#[test]
fn switching_workspaces_changes_task_click_targets_without_a_panel_update() {
    let mut snapshot = task_slot_snapshot(&["A", "A"]);
    snapshot.windows[1].workspace.index = 1;
    snapshot.workspaces = Some(flamewm_api::workspace::WorkspaceSnapshot {
        revision: 1,
        count: 2,
        active_index: 0,
        last_index: None,
        names: Vec::new(),
    });
    let mut runtime = ShellRuntime::new(snapshot);
    runtime.select_panel_output(flamewm_api::OutputId::new("A"));
    assert_eq!(runtime.task_click_requests_for_slot(0), activated(1));
    assert_eq!(runtime.task_click_requests_for_slot(1), None);
    let next = flamewm_api::workspace::WorkspaceSnapshot {
        revision: 2,
        count: 2,
        active_index: 1,
        last_index: Some(0),
        names: Vec::new(),
    };
    runtime.mark_signal(&ControlSignal::WorkspacesSnapshotChanged { snapshot: next });
    let changed = runtime.reconcile_applied();
    assert!(changed.workspaces);
    assert!(
        !changed.windows,
        "no window snapshot update should be necessary"
    );
    assert_eq!(runtime.task_click_requests_for_slot(0), activated(2));
    assert_eq!(runtime.task_click_requests_for_slot(1), None);
}

fn workspace_snapshot(
    count: usize,
    active_index: usize,
) -> flamewm_api::workspace::WorkspaceSnapshot {
    flamewm_api::workspace::WorkspaceSnapshot {
        revision: 1,
        count,
        active_index,
        last_index: None,
        names: Vec::new(),
    }
}

// REGRESSION: a pager action names a topology CELL, not a page slot. With
// six workspaces (`1 2 3 / 4 5 6`, column-major cells) cell 2 is workspace
// 4, so the click must activate index 3 instead of the second page slot.
// MUTATION: keep mapping `workspace-N` to page slot `N - 1`.
#[test]
fn pager_cell_actions_activate_the_workspace_in_that_cell() {
    let mut snapshot = ShellSnapshot::default();
    snapshot.workspaces = Some(workspace_snapshot(6, 0));
    let runtime = ShellRuntime::new(snapshot);
    assert_eq!(
        runtime.control_request_for_action("workspace.2"),
        Some(ControlRequest::ActivateWorkspace {
            index: 3,
            expected_revision: 1,
        })
    );
    assert_eq!(
        runtime.control_request_for_action("workspace-4"),
        Some(ControlRequest::ActivateWorkspace {
            index: 4,
            expected_revision: 1,
        })
    );
    assert_eq!(
        runtime.control_request_for_action("workspace.6"),
        Some(ControlRequest::ActivateWorkspace {
            index: 5,
            expected_revision: 1,
        })
    );
    // Cells 7..=18 hold no workspace when only six exist, and a missing
    // snapshot has no topology to resolve against.
    assert_eq!(runtime.control_request_for_action("workspace-18"), None);
    assert_eq!(runtime.control_request_for_action("workspace.0"), None);
    assert_eq!(runtime.control_request_for_action("task.slot.1"), None);
    let runtime = ShellRuntime::new(ShellSnapshot::default());
    assert_eq!(runtime.control_request_for_action("workspace.1"), None);
}

#[test]
fn pager_keeps_global_workspace_identity_while_input_origin_selects_local_tasks() {
    let mut snapshot = task_slot_snapshot(&["A", "B"]);
    snapshot.workspaces = Some(workspace_snapshot(6, 0));
    let mut runtime = ShellRuntime::new(snapshot);
    let workspace_request = Some(ControlRequest::ActivateWorkspace {
        index: 3,
        expected_revision: 1,
    });

    runtime.select_panel_output(flamewm_api::OutputId::new("A"));
    assert_eq!(
        runtime.control_request_for_action("workspace.2"),
        workspace_request
    );
    assert_eq!(runtime.task_click_requests_for_slot(0), activated(1));

    runtime.select_panel_output(flamewm_api::OutputId::new("B"));
    assert_eq!(
        runtime.control_request_for_action("workspace.2"),
        workspace_request
    );
    assert_eq!(runtime.task_click_requests_for_slot(0), activated(2));
}

#[test]
fn global_workspace_snapshot_refreshes_task_views_on_each_output() {
    let mut snapshot = task_slot_snapshot(&["A", "B"]);
    snapshot.windows[1].workspace.index = 1;
    snapshot.workspaces = Some(workspace_snapshot(2, 0));
    let mut runtime = ShellRuntime::new(snapshot);

    runtime.select_panel_output(flamewm_api::OutputId::new("A"));
    assert_eq!(runtime.task_click_requests_for_slot(0), activated(1));
    runtime.select_panel_output(flamewm_api::OutputId::new("B"));
    assert_eq!(runtime.task_click_requests_for_slot(0), None);

    runtime.mark_signal(&ControlSignal::WorkspacesSnapshotChanged {
        snapshot: flamewm_api::workspace::WorkspaceSnapshot {
            revision: 2,
            count: 2,
            active_index: 1,
            last_index: Some(0),
            names: Vec::new(),
        },
    });
    let changed = runtime.reconcile_dirty();
    assert!(changed.workspaces);
    assert!(!changed.windows);

    runtime.select_panel_output(flamewm_api::OutputId::new("A"));
    assert_eq!(runtime.task_click_requests_for_slot(0), None);
    runtime.select_panel_output(flamewm_api::OutputId::new("B"));
    assert_eq!(runtime.task_click_requests_for_slot(0), activated(2));
}

// REGRESSION: the workspace context menu follows the clicked cell, and an
// empty cell must not open the menu of a neighbouring workspace.
#[test]
fn workspace_context_menu_opens_for_the_cell_workspace_only() {
    let mut snapshot = ShellSnapshot::default();
    snapshot.workspaces = Some(workspace_snapshot(6, 0));
    let mut runtime = ShellRuntime::new(snapshot);
    runtime.open_workspace_context_at_cell(6, None);
    match runtime.controls.front() {
        Some(ShellControl::OpenContextMenu(state)) => {
            assert_eq!(state.kind, ContextMenuKind::Workspace { index: 5 });
        }
        other => panic!("expected workspace context menu, got {other:?}"),
    }
    runtime.controls.clear();
    runtime.open_workspace_context_at_cell(18, None);
    assert!(runtime.controls.is_empty(), "empty cell opens nothing");
}

#[test]
fn task_slot_context_menu_follows_the_shown_panel_output() {
    let mut runtime = ShellRuntime::new(task_slot_snapshot(&["B", "B"]));
    runtime.open_task_context_at_slot(0, None);
    assert!(runtime.controls.is_empty(), "panel A shows no tasks");

    let mut snapshot = task_slot_snapshot(&["A", "B"]);
    snapshot.panels.panels.reverse();
    runtime = ShellRuntime::new(snapshot);
    runtime.select_panel_output(flamewm_api::OutputId::new("B"));
    runtime.open_task_context_at_slot(0, None);
    // Panel B shown first: slot 0 is window 2 (task 2) only after window 1 leaves B.
    let expected = flamewm_api::TaskEntryId::window(win_ref(2));
    match runtime.controls.front() {
        Some(ShellControl::OpenContextMenu(state)) => {
            assert_eq!(state.kind, ContextMenuKind::Task(expected));
        }
        other => panic!("expected task context menu, got {other:?}"),
    }
}

#[test]
fn task_slot_reorder_skips_hidden_entries_with_full_list_index() {
    // Tasks 1, 2, 4 on B (shown first), task 3 on A (hidden). Slot 1 is task 2, slot 2 is task 4.
    let mut snapshot = task_slot_snapshot(&["B", "B", "A", "B"]);
    snapshot.panels.panels.reverse();
    let mut runtime = ShellRuntime::new(snapshot);
    runtime.select_panel_output(flamewm_api::OutputId::new("B"));
    assert_eq!(runtime.task_slot_count(), 3);
    assert_eq!(
        runtime.task_reorder_request(1, 2),
        Some(ControlRequest::ReorderTask {
            entry_id: flamewm_api::TaskEntryId::window(win_ref(2)),
            index: 3,
            expected_revision: 7,
        })
    );
}

#[test]
fn task_slot_reorder_uses_shown_output_slots_and_full_list_index() {
    // Panel B shown first with both windows on B: slots equal the full task list.
    let mut snapshot = task_slot_snapshot(&["B", "B"]);
    snapshot.panels.panels.reverse();
    let mut runtime = ShellRuntime::new(snapshot);
    runtime.select_panel_output(flamewm_api::OutputId::new("B"));
    assert_eq!(runtime.task_slot_count(), 2);
    assert_eq!(
        runtime.task_reorder_request(0, 1),
        Some(ControlRequest::ReorderTask {
            entry_id: flamewm_api::TaskEntryId::window(win_ref(1)),
            index: 1,
            expected_revision: 7,
        })
    );

    // Window 1 on A: panel B shows only window 2, so its single slot cannot reorder.
    let mut snapshot = task_slot_snapshot(&["A", "B"]);
    snapshot.panels.panels.reverse();
    let mut runtime = ShellRuntime::new(snapshot);
    runtime.select_panel_output(flamewm_api::OutputId::new("B"));
    assert_eq!(runtime.task_slot_count(), 1);
    assert_eq!(runtime.task_reorder_request(0, 1), None);
}

fn two_output_snapshot() -> ShellSnapshot {
    let mut snapshot = ShellSnapshot::default();
    snapshot.displays.generation = 3;
    snapshot.displays.outputs = vec![
        panel_output("HDMI-1", flamewm_api::Rect::new(0, 0, 1920, 1080)),
        panel_output("HDMI-2", flamewm_api::Rect::new(1920, 0, 1280, 1024)),
    ];
    snapshot.panels.revision = 5;
    snapshot.panels.panels = vec![
        panel_entry(
            "HDMI-1",
            flamewm_api::PanelEdge::Bottom,
            flamewm_api::Rect::new(0, 1036, 1920, 44),
        ),
        panel_entry(
            "HDMI-2",
            flamewm_api::PanelEdge::Bottom,
            flamewm_api::Rect::new(1920, 980, 1280, 44),
        ),
    ];
    snapshot
}

#[test]
fn two_outputs_two_panels_yield_one_context_per_output() {
    let snapshot = two_output_snapshot();
    let contexts = panel_contexts_for_snapshot(&snapshot, PanelEdge::Bottom);
    assert_eq!(contexts.len(), 2);
    assert_eq!(contexts[0].output.id, flamewm_api::OutputId::new("HDMI-1"));
    assert_eq!(
        contexts[0].output.geometry,
        flamewm_api::Rect::new(0, 0, 1920, 1080)
    );
    assert_eq!(
        contexts[0].work_area,
        flamewm_api::Rect::new(0, 0, 1920, 1036)
    );
    assert_eq!(contexts[1].output.id, flamewm_api::OutputId::new("HDMI-2"));
    assert_eq!(
        contexts[1].output.geometry,
        flamewm_api::Rect::new(1920, 0, 1280, 1024)
    );
    assert_eq!(
        contexts[1].panel.geometry,
        flamewm_api::Rect::new(1920, 980, 1280, 44)
    );
    assert_eq!(
        contexts[1].work_area,
        flamewm_api::Rect::new(1920, 0, 1280, 980)
    );
    assert!(panel_contexts_for_snapshot(&snapshot, PanelEdge::Top).is_empty());
}

// REGRESSION (A12-1): a context menu opened near the bottom panel must be
// clamped inside the panel-aware work area, not the raw output geometry.
// MUTATION: pass `placement.output.geometry` to `context_menu_rect` again;
// the menu slides under the taskbar and both assertions below flip.
#[test]
fn context_menu_near_bottom_panel_clamps_above_the_panel_strip() {
    let snapshot = two_output_snapshot();
    let contexts = panel_contexts_for_snapshot(&snapshot, PanelEdge::Bottom);
    // Secondary output: 1280x1024 with a 44px bottom panel at y = 980.
    let placement = &contexts[1];
    assert_eq!(
        placement.work_area,
        flamewm_api::Rect::new(1920, 0, 1280, 980)
    );
    let panel_top = placement.panel.geometry.y;
    assert_eq!(panel_top, 980);
    // Pointer inside the panel strip (a right-click on a task row).
    let pointer = Some(flamewm_api::Point::new(3180, 1010));
    let size = context_menu_size(3);

    let rect = context_menu_rect_for_placement(placement, pointer, size);
    assert_eq!((rect.width, rect.height), (size.width, size.height));
    assert!(
        rect.bottom() <= panel_top,
        "menu must stop above the panel: {rect:?}"
    );
    assert!(
        rect.bottom() <= placement.work_area.bottom(),
        "menu must stay inside the work area: {rect:?}"
    );
    assert!(rect.y >= placement.work_area.y, "{rect:?}");
    assert!(
        rect.x >= placement.work_area.x && rect.right() <= placement.work_area.right(),
        "{rect:?}"
    );

    // The pre-A12-1 argument (raw output geometry) clamps to the output
    // bottom instead, so the menu overlaps the taskbar strip.
    let stale = context_menu_rect(
        placement.panel.output.clone(),
        placement.output.geometry,
        pointer,
        size,
    );
    assert_eq!(stale.bottom(), placement.output.geometry.bottom());
    assert!(stale.bottom() > panel_top, "{stale:?}");
}

#[test]
fn secondary_monitor_swap_with_unchanged_generation_is_detected() {
    let previous = two_output_snapshot();
    let contexts = panel_contexts_for_snapshot(&previous, PanelEdge::Bottom);
    assert!(contexts
        .iter()
        .all(|ctx| panel_context_is_current(ctx, &previous)));
    let mut moved = previous.clone();
    // KDE or xrandr may deliver CRTC geometry before changing the
    // generation/revision: secondary placement must not stay cached.
    moved.displays.outputs[1].geometry.x = 0;
    moved.displays.outputs[0].geometry.x = 1280;
    assert!(!panel_context_is_current(&contexts[1], &moved));
    assert!(!panel_context_is_current(&contexts[0], &moved));
}

#[test]
fn popup_context_survives_unrelated_output_and_global_revision_changes() {
    let previous = two_output_snapshot();
    let old = panel_contexts_for_snapshot(&previous, PanelEdge::Bottom);
    let mut next = previous.clone();
    next.displays.generation += 1;
    next.panels.revision += 1;
    next.displays.outputs[1].geometry.x += 80;
    next.panels.panels[1].geometry.x += 80;
    let new = panel_contexts_for_snapshot(&next, PanelEdge::Bottom);
    assert!(
        popup_context_unchanged(&old[0], &new[0]),
        "unchanged A keeps its popup"
    );
    assert!(
        !popup_context_unchanged(&old[1], &new[1]),
        "changed B invalidates its popup"
    );
}

#[test]
fn mixed_panel_edges_resolve_independently() {
    let mut snapshot = two_output_snapshot();
    snapshot.panels.panels[1].edge = PanelEdge::Top;
    snapshot.panels.panels[1].geometry.y = 0;
    let contexts: Vec<_> = snapshot
        .panels
        .panels
        .iter()
        .filter_map(|panel| panel_context_for_panel(&snapshot, panel))
        .collect();
    assert_eq!(contexts.len(), 2);
    assert_eq!(contexts[0].panel.edge, PanelEdge::Bottom);
    assert_eq!(contexts[1].panel.edge, PanelEdge::Top);
}

#[test]
fn single_output_contexts_match_legacy_single_context() {
    let snapshot = bottom_panel_snapshot(1, 1);
    let contexts = panel_contexts_for_snapshot(&snapshot, PanelEdge::Bottom);
    let legacy = panel_context_for_snapshot(&snapshot, PanelEdge::Bottom);
    assert_eq!(contexts.len(), 1);
    assert_eq!(Some(contexts[0].clone()), legacy);
}

#[test]
fn hotplug_removal_of_one_output_removes_only_its_panel_surface() {
    let mut snapshot = two_output_snapshot();
    snapshot.displays.outputs[1].connected = false;
    let existing = vec![
        flamewm_api::OutputId::new("HDMI-1"),
        flamewm_api::OutputId::new("HDMI-2"),
    ];
    let stale = panel_surface_outputs_to_remove(&existing, &snapshot);
    assert_eq!(stale, vec![flamewm_api::OutputId::new("HDMI-2")]);
    let kept = panel_surface_outputs_to_remove(&existing[..1], &snapshot);
    assert!(kept.is_empty());
}

#[test]
fn panel_reconciliation_plans_updates_creates_and_removals_before_mutation() {
    let snapshot = two_output_snapshot();
    let current = panel_contexts_for_snapshot(&snapshot, PanelEdge::Bottom);
    let existing = current
        .iter()
        .map(|context| (context.output.id.clone(), context.clone()))
        .collect::<Vec<_>>();

    let mut updated_a = current[0].clone();
    updated_a.panels_revision += 1;
    let mut added_c = current[1].clone();
    added_c.output.id = flamewm_api::OutputId::new("C");
    added_c.panel.output = flamewm_api::OutputId::new("C");
    let desired = vec![updated_a, current[1].clone(), added_c];
    assert_eq!(
        panel_reconciliation_plan(&existing, &desired),
        PanelReconcilePlan {
            update: vec![flamewm_api::OutputId::new("HDMI-1")],
            create: vec![flamewm_api::OutputId::new("C")],
            remove: Vec::new(),
        },
        "unchanged B keeps its record while A updates and C is staged"
    );

    let only_a = vec![current[0].clone()];
    assert_eq!(
        panel_reconciliation_plan(&existing, &only_a).remove,
        vec![flamewm_api::OutputId::new("HDMI-2")],
        "disconnect retires only its output record"
    );
}
