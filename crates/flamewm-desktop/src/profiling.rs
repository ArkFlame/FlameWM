use super::*;

point_fn!(scan_point, ProfilePoint, "desktop.scan");

point_fn!(sync_point, ProfilePoint, "desktop.sync_document");

point_fn!(rescan_point, ProfilePoint, "desktop.rescan");

point_fn!(icon_point, ProfilePoint, "desktop.icon.resolve");

point_fn!(
    inotify_wakeup_counter,
    CounterPoint,
    "desktop.inotify.wakeup"
);

point_fn!(
    inotify_rescan_counter,
    CounterPoint,
    "desktop.inotify.rescan"
);

point_fn!(
    selection_begin_counter,
    CounterPoint,
    "desktop.selection.begin"
);

point_fn!(
    selection_visible_counter,
    CounterPoint,
    "desktop.selection.visible"
);

point_fn!(selection_end_counter, CounterPoint, "desktop.selection.end");

point_fn!(
    context_secondary_counter,
    CounterPoint,
    "desktop.context.secondary"
);

point_fn!(context_open_counter, CounterPoint, "desktop.context.open");

point_fn!(
    context_open_total_point,
    ProfilePoint,
    "desktop.context.open.total"
);

point_fn!(
    context_open_project_point,
    ProfilePoint,
    "desktop.context.open.project"
);

point_fn!(
    context_open_measure_point,
    ProfilePoint,
    "desktop.context.open.measure"
);

point_fn!(
    context_open_place_point,
    ProfilePoint,
    "desktop.context.open.place"
);

point_fn!(
    context_open_present_point,
    ProfilePoint,
    "desktop.context.open.present"
);

point_fn!(
    context_open_close_point,
    ProfilePoint,
    "desktop.context.open.close"
);

point_fn!(
    context_action_counter,
    CounterPoint,
    "desktop.context.action"
);

point_fn!(render_sync_counter, CounterPoint, "desktop.render.sync");

point_fn!(
    drag_ghost_show_counter,
    CounterPoint,
    "desktop.drag.ghost.show"
);

point_fn!(
    drag_ghost_motion_counter,
    CounterPoint,
    "desktop.drag.ghost.motion"
);

point_fn!(rename_open_counter, CounterPoint, "desktop.rename.open");

point_fn!(
    rename_success_counter,
    CounterPoint,
    "desktop.rename.success"
);

point_fn!(
    rename_failure_counter,
    CounterPoint,
    "desktop.rename.failure"
);

point_fn!(
    action_failure_counter,
    CounterPoint,
    "desktop.action.failure"
);

point_fn!(spawn_failure_counter, CounterPoint, "desktop.spawn.failure");

point_fn!(
    workspace_change_counter,
    CounterPoint,
    "desktop.workspace.change"
);

point_fn!(
    workspace_signal_point,
    ProfilePoint,
    "desktop.workspace.signal"
);

point_fn!(
    workspace_apply_point,
    ProfilePoint,
    "desktop.workspace.apply"
);

point_fn!(
    workspace_project_point,
    ProfilePoint,
    "desktop.workspace.project"
);

point_fn!(
    workspace_present_point,
    ProfilePoint,
    "desktop.workspace.present"
);

point_fn!(
    workspace_snapshot_received_counter,
    CounterPoint,
    "desktop.workspace.snapshot_received"
);

point_fn!(
    workspace_legacy_ignored_counter,
    CounterPoint,
    "desktop.workspace.legacy_ignored"
);

point_fn!(model_gauge, MemoryGauge, "desktop.model");

point_fn!(launcher_gauge, MemoryGauge, "desktop.launcher");

point_fn!(icon_gauge, MemoryGauge, "desktop.icon");
