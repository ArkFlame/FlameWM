# u28-shell-core

- main: the first profiler window timer (5 s one-shot) is registered only when
  `flamewm_profiler::enabled()`, matching the repeating timer's existing gate. An idle shell no
  longer wakes every five seconds to run `report_window`, which returns immediately when the
  profiler is off (the default), so this removed one guaranteed idle wakeup per five seconds.
- main: notification centre reprojection compares the live active ids against the last projected
  list by iterator instead of cloning `center_active` into a fresh `Vec<u32>` on every wake while
  the centre is open; the clone is now only built when a projection actually runs.
- event_router/route: menu row actions (`format!` per row) are built only for a keyboard menu
  navigation key press or an activation key (Enter/Space). Pointer, wheel and hover events are the
  bulk of the event stream and no longer allocate one `String` per open menu row.
- app/settings + app/panels: a domain refresh (window/workspace/system change) no longer re-runs
  the full panel task-slot reprojection and its Start reprojection from the settings pass. Those
  slots were just projected from the unchanged settings; only a real `SettingsSnapshot` change
  (accent/theme) needs them reprojected. This removes a duplicate full panel projection on every
  window/workspace/system change (volume, focus, titles) while keeping the style pass that restores
  the settings-derived accent and element backgrounds.
- app/dock: `sync_dock` compares the computed dock frame/name/entry lists against the last
  published state by reference and clones into `dock_last` only when it changes. An idle tick no
  longer clones the frame, every slot label `String`, and the entry list just to discard them.
- app: `ShellLoop::new` returns `Result<Self, String>` instead of panicking with
  `expect("result wake pair")`; the entry point reports a failed wake pipe as a startup error.
- runtime: removed the unused `panel_surface_outputs_to_remove` helper and its test; hotplug
  removal is owned by `panel_reconciliation_plan`, so this was dead production code.
