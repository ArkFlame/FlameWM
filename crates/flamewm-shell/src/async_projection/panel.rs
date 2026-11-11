//! Panel projection: task slots, pager, status tray, geometry and scoped panel passes.
use super::*;

/// Panel projection without any synchronous icon resolve: task labels,
/// visibility, backgrounds, workspaces, clock, and tray visibility project
/// immediately; icons apply only when cached, otherwise a request is queued.
/// Fine-grained entry points (`project_task_slots_async`,
/// `project_pager_async`, `project_status_tray_async`,
/// `project_panel_geometry`) let each changed domain reproject only the
/// nodes it owns; `project_panel_async` is the full-panel fallback for
/// cold start / snapshot replace.
pub fn project_panel_async(
    document: &mut impl UiDocumentAccess,
    view: PanelView<'_>,
    loader: Option<&mut IconLoader>,
    icon_cache: &mut TaskIconCache,
) -> Result<(), String> {
    project_panel_scoped(document, view, loader, icon_cache, PanelScope::full())
}

/// Subset of panel nodes a changed domain may reproject. Windows owns
/// task slots only, Workspaces owns the pager and workspace-filtered tasks, Panels owns geometry/
/// layout + task-slot/pager ownership counts, System owns the status tray
/// visibility only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelScope {
    pub task_slots: bool,
    pub pager: bool,
    pub tray: bool,
    pub geometry: bool,
}

impl PanelScope {
    #[must_use]
    pub fn full() -> Self {
        Self {
            task_slots: true,
            pager: true,
            tray: true,
            geometry: true,
        }
    }

    #[must_use]
    pub fn for_changed(changed: crate::runtime::ChangedDomains) -> Self {
        Self {
            task_slots: changed.windows || changed.workspaces,
            pager: changed.workspaces,
            tray: changed.system,
            geometry: changed.panels,
        }
    }

    #[must_use]
    pub fn none() -> Self {
        Self {
            task_slots: false,
            pager: false,
            tray: false,
            geometry: false,
        }
    }
}

/// Domain-scoped panel dispatcher for the shell loop (`refresh_dynamic`):
/// `WindowsChanged` -> task slots only, `WorkspacesChanged` -> pager and task slots,
/// `PanelsChanged` -> geometry only, `SystemChanged` -> tray only. Combined
/// flags compose (e.g. panels+windows projects geometry+slots, never the
/// tray). Empty scope is a no-op returning `Ok`. Clock projects only on
/// the full scope (cold start / snapshot replace via
/// `project_panel_async`); per-tick clock uses `project_clock_at`.
/// Focus-only window updates therefore reproject 8 task slots, not the
/// full panel.
pub fn project_panel_for_changed(
    document: &mut impl UiDocumentAccess,
    view: PanelView<'_>,
    loader: Option<&mut IconLoader>,
    icon_cache: &mut TaskIconCache,
    changed: crate::runtime::ChangedDomains,
) -> Result<(), String> {
    let scope = PanelScope::for_changed(changed);
    if scope == PanelScope::none() {
        return Ok(());
    }
    project_panel_scoped(document, view, loader, icon_cache, scope)
}

/// `WorkspacesChanged` also refreshes tasks via `PanelScope`; this projection
/// itself owns only pager nodes. Projects the same pager cells, columns and
/// fills as the sync panel pass (`crate::projection::project_pager`).
pub fn project_pager_async(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
) -> Result<(), String> {
    let _span = crate::runtime::shell_span("shell.pager.project").start();
    crate::projection::project_pager(document, snapshot.workspaces.as_ref())
}

/// `SystemChanged` -> status tray visibility only. The open popup content
/// projects on its own open path (`refresh_dynamic` reprojects media when
/// open); the panel itself only flips tray visibility nodes.
pub fn project_status_tray_async(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
    items: TaskbarItems,
) -> Result<(), String> {
    crate::projection::project_status_tray(document, &snapshot.system, items)
}

/// `PanelsChanged` -> geometry/layout ownership counts only (slot/pager
/// visibility for added/removed entries). Content styling stays with the
/// task/pager projectors. Currently geometry is node-identical to the
/// visibility pass, so this reuses the slot/pager visibility tails without
/// labels, icons, or backgrounds.
pub fn project_panel_geometry(
    document: &mut impl UiDocumentAccess,
    view: PanelView<'_>,
) -> Result<(), String> {
    let snapshot = view.snapshot;
    crate::projection::project_taskbar_height(document, view)?;
    let states = task_states(view);
    for index in 0..TASK_SLOT_COUNT {
        document.visible(
            &format!("task-slot-{}", index + 1),
            view.items.activities && states.get(index).is_some(),
        )?;
    }
    crate::projection::project_task_slots_extent(
        document,
        crate::projection::task_slots_extent_count(states.len(), view.items),
        crate::projection::taskbar_is_vertical(&view),
    )?;
    document.visible(
        "task-overflow",
        crate::projection::task_overflow_count(states.len(), view.items.activities) > 0,
    )?;
    document.visible("task-slot-insert", view.items.activities)?;
    crate::projection::project_pager_visibility(document, snapshot.workspaces.as_ref())?;
    crate::projection::project_element_backgrounds(
        document,
        view.items,
        crate::projection::task_slots_rendered(states.len(), view.items),
        crate::projection::tray_any_visible(&snapshot.system, view.items),
        view.items.clock,
    )?;
    Ok(())
}

pub(super) fn project_panel_scoped(
    document: &mut impl UiDocumentAccess,
    view: PanelView<'_>,
    loader: Option<&mut IconLoader>,
    icon_cache: &mut TaskIconCache,
    scope: PanelScope,
) -> Result<(), String> {
    if scope.task_slots {
        project_task_slots_async(document, view, loader, icon_cache)?;
    }
    if scope.geometry {
        project_panel_geometry(document, view)?;
    }
    if scope.pager {
        project_pager_async(document, view.snapshot)?;
        document.visible(
            "workspace-pager",
            crate::projection::workspace_pager_visible(
                view.items,
                view.snapshot.workspaces.as_ref(),
            ),
        )?;
    }
    if scope.tray {
        project_status_tray_async(document, view.snapshot, view.items)?;
    }
    if scope.task_slots && scope.pager && scope.tray && scope.geometry {
        crate::projection::project_clock(document)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::async_projection::tests::*;

    #[test]
    fn windows_scope_projects_task_slots_only() {
        use crate::runtime::ChangedDomains;
        let _guard = icon_test_lock().lock();
        let snapshot = ShellSnapshot::default();
        let changed = ChangedDomains {
            windows: true,
            ..ChangedDomains::default()
        };
        let scope = PanelScope::for_changed(changed);
        assert_eq!(
            scope,
            PanelScope {
                task_slots: true,
                pager: false,
                tray: false,
                geometry: false,
            }
        );
        let doc = project_changed(PanelView::first(&snapshot), changed);
        for index in 0..8 {
            assert!(
                doc.visible
                    .contains_key(&format!("task-slot-{}", index + 1)),
                "task slot projected",
            );
        }
        assert!(
            !doc.visible.contains_key("workspace-1"),
            "pager untouched on windows-only change",
        );
        assert!(
            !doc.visible.contains_key("tray-volume"),
            "tray untouched on windows-only change",
        );
    }

    #[test]
    fn workspaces_scope_projects_pager_and_tasks() {
        use crate::runtime::ChangedDomains;
        let _guard = icon_test_lock().lock();
        let snapshot = ShellSnapshot::default();
        let changed = ChangedDomains {
            workspaces: true,
            ..ChangedDomains::default()
        };
        let doc = project_changed(PanelView::first(&snapshot), changed);
        assert!(doc.visible.contains_key("workspace-1"), "pager projected");
        assert!(
            doc.visible.contains_key("task-slot-1"),
            "task slots must update on workspace-only change",
        );
        assert!(
            !doc.visible.contains_key("tray-volume"),
            "tray untouched on workspaces-only change",
        );
    }

    #[test]
    fn asynchronous_workspace_count_transition_hides_empty_capsule() {
        use crate::runtime::ChangedDomains;
        let _guard = icon_test_lock().lock();
        let changed = ChangedDomains {
            workspaces: true,
            ..ChangedDomains::default()
        };
        let mut snapshot = ShellSnapshot::default();
        let pager = |snapshot: &ShellSnapshot| project_changed(PanelView::first(snapshot), changed);
        snapshot.workspaces = Some(flamewm_api::workspace::WorkspaceSnapshot {
            revision: 1,
            count: 1,
            active_index: 0,
            last_index: None,
            occupancy: Vec::new(),
            names: Vec::new(),
        });
        assert_eq!(
            pager(&snapshot).visible.get("workspace-pager"),
            Some(&false)
        );
        snapshot
            .workspaces
            .as_mut()
            .expect("workspace snapshot")
            .count = 2;
        assert_eq!(pager(&snapshot).visible.get("workspace-pager"), Some(&true));
        snapshot
            .workspaces
            .as_mut()
            .expect("workspace snapshot")
            .count = 1;
        assert_eq!(
            pager(&snapshot).visible.get("workspace-pager"),
            Some(&false)
        );
    }

    // REGRESSION: the async pager pass projects the same two-row topology as
    // the sync panel pass. Six workspaces read `1 2 3 / 4 5 6`, so workspace 4
    // (index 3) is the lower cell of the first column (cell 2), not the page
    // slot of an old 2x2 page.
    // MUTATION: keep the fixed 4-slot page in either pager path.
    #[test]
    fn async_pager_projects_the_two_row_topology() {
        let _guard = icon_test_lock().lock();
        let snapshot = ShellSnapshot {
            workspaces: Some(flamewm_api::workspace::WorkspaceSnapshot {
                revision: 1,
                count: 6,
                active_index: 3,
                last_index: None,
                occupancy: Vec::new(),
                names: Vec::new(),
            }),
            ..ShellSnapshot::default()
        };
        let mut doc = FakeDoc::default();
        project_pager_async(&mut doc, &snapshot).expect("pager projects");
        // Labels follow the cell each workspace sits in.
        assert_eq!(doc.texts.get("workspace-1").map(String::as_str), Some("1"));
        assert_eq!(doc.texts.get("workspace-3").map(String::as_str), Some("2"));
        assert_eq!(doc.texts.get("workspace-5").map(String::as_str), Some("3"));
        assert_eq!(doc.texts.get("workspace-2").map(String::as_str), Some("4"));
        assert_eq!(doc.texts.get("workspace-4").map(String::as_str), Some("5"));
        assert_eq!(doc.texts.get("workspace-6").map(String::as_str), Some("6"));
        // Cells 1..=6 are shown, every later cell and column is hidden.
        for cell in 1..=6 {
            assert_eq!(
                doc.visible.get(&format!("workspace-{cell}")),
                Some(&true),
                "cell {cell}"
            );
        }
        for cell in 7..=18 {
            assert_eq!(
                doc.visible.get(&format!("workspace-{cell}")),
                Some(&false),
                "cell {cell}"
            );
        }
        for column in 1..=3 {
            assert_eq!(
                doc.visible.get(&format!("workspace-column-{column}")),
                Some(&true),
                "column {column}"
            );
        }
        for column in 4..=9 {
            assert_eq!(
                doc.visible.get(&format!("workspace-column-{column}")),
                Some(&false),
                "column {column}"
            );
        }
        // Only the active workspace carries the accent fill.
        let active = crate::projection::pager_active_fill();
        assert_eq!(doc.fills.get("workspace-2"), Some(&active));
        for cell in [1, 3, 4, 5, 6] {
            assert_ne!(
                doc.fills.get(&format!("workspace-{cell}")),
                Some(&active),
                "cell {cell} is not the active workspace"
            );
        }
    }

    #[test]
    fn system_scope_projects_tray_only() {
        use crate::runtime::ChangedDomains;
        let _guard = icon_test_lock().lock();
        let snapshot = ShellSnapshot::default();
        let changed = ChangedDomains {
            system: true,
            ..ChangedDomains::default()
        };
        let doc = project_changed(PanelView::first(&snapshot), changed);
        assert!(doc.visible.contains_key("tray-volume"), "tray projected");
        assert!(
            !doc.visible.contains_key("task-slot-1"),
            "task slots untouched on system-only change",
        );
        assert!(
            !doc.visible.contains_key("workspace-1"),
            "pager untouched on system-only change",
        );
    }

    #[test]
    fn status_tray_matches_sync_full_and_async_system_projection() {
        use flamewm_api::system::{PlaybackState, ServiceAvailability};
        let _scale = crate::taskbar_scale::test_lock();
        crate::taskbar_scale::set_factor(1.0);
        let _guard = icon_test_lock().lock();
        let mut snapshot = ShellSnapshot::default();
        snapshot.system.media.availability = ServiceAvailability::Available;
        snapshot.system.audio.availability = ServiceAvailability::Available;
        snapshot.system.network.availability = ServiceAvailability::Available;

        for (
            playback,
            muted,
            network_available,
            volume_item,
            network_item,
            clock_item,
            media,
            play,
            pause,
        ) in [
            (
                PlaybackState::Playing,
                false,
                true,
                true,
                true,
                true,
                true,
                false,
                true,
            ),
            (
                PlaybackState::Paused,
                true,
                true,
                false,
                false,
                false,
                true,
                true,
                false,
            ),
            (
                PlaybackState::Stopped,
                false,
                false,
                true,
                true,
                true,
                false,
                false,
                false,
            ),
        ] {
            snapshot.system.media.playback = playback;
            snapshot.system.audio.muted = muted;
            snapshot.system.network.availability = if network_available {
                ServiceAvailability::Available
            } else {
                ServiceAvailability::Unavailable
            };
            let items = TaskbarItems {
                volume: volume_item,
                network: network_item,
                clock: clock_item,
                ..TaskbarItems::default()
            };

            let mut sync = FakeDoc::default();
            let mut resolver = flamewm_integrations_linux::icons::IconResolver::new(
                std::path::PathBuf::new(),
                Vec::new(),
                Vec::new(),
                0,
            );
            crate::projection::project_panel(
                &mut sync,
                PanelView::first(&snapshot).with_items(items),
                &mut resolver,
            )
            .expect("sync panel projects");

            let changed = crate::runtime::ChangedDomains {
                system: true,
                ..Default::default()
            };
            let scoped = project_changed(PanelView::first(&snapshot).with_items(items), changed);
            let mut full = FakeDoc::default();
            project_panel_async(
                &mut full,
                PanelView::first(&snapshot).with_items(items),
                None,
                &mut TaskIconCache::default(),
            )
            .expect("async full panel projects");

            let sync_tray = tray_visibility(&sync);
            assert_eq!(tray_visibility(&scoped), sync_tray, "scoped: {playback:?}");
            assert_eq!(tray_visibility(&full), sync_tray, "full: {playback:?}");
            assert_eq!(sync_tray.get("tray-media"), Some(&media));
            assert_eq!(sync_tray.get("tray-play-icon"), Some(&play));
            assert_eq!(sync_tray.get("tray-pause-icon"), Some(&pause));
            assert_eq!(sync_tray.get("tray-volume"), Some(&volume_item));
            assert_eq!(sync_tray.get("tray-volume-icon"), Some(&!muted));
            assert_eq!(sync_tray.get("tray-volume-muted-icon"), Some(&muted));
            assert_eq!(
                sync_tray.get("tray-network"),
                Some(&(network_available && network_item))
            );
            assert_eq!(sync_tray.get("clock-button"), Some(&clock_item));
        }
    }

    #[test]
    fn panels_scope_projects_geometry_only() {
        use crate::runtime::ChangedDomains;
        let _scale = crate::taskbar_scale::test_lock();
        crate::taskbar_scale::set_factor(1.0);
        let _guard = icon_test_lock().lock();
        let snapshot = ShellSnapshot::default();
        let changed = ChangedDomains {
            panels: true,
            ..ChangedDomains::default()
        };
        let doc = project_changed(PanelView::first(&snapshot), changed);
        assert!(
            doc.visible.contains_key("task-slot-1"),
            "geometry owns slot count"
        );
        assert!(
            doc.visible.contains_key("workspace-1"),
            "geometry owns pager count"
        );
        assert!(doc.texts.is_empty(), "geometry writes no labels");
        assert!(doc.images.is_empty(), "geometry writes no icons");
    }

    // REGRESSION: a power-only SystemChanged reprojects through the tray
    // scope; the battery button must follow the power snapshot there, not
    // only on the sync projection path. A 45% discharge shows the 50 glyph,
    // an 87% reading shows the whole button with the 90 glyph.
    // MUTATION: drop the project_battery_tray call from project_status_tray_async.
    #[test]
    fn system_scope_projects_battery_from_power_snapshot() {
        use crate::runtime::ChangedDomains;
        use flamewm_api::system::{PowerSnapshot, PowerState, ServiceAvailability};
        let _guard = icon_test_lock().lock();
        let mut snapshot = ShellSnapshot::default();
        snapshot.system.power = PowerSnapshot {
            available: ServiceAvailability::Available,
            present: true,
            percent: 45,
            state: PowerState::Discharging,
            cpu_percent: None,
            power_rate_mw: None,
        };
        let changed = ChangedDomains {
            system: true,
            ..ChangedDomains::default()
        };
        let doc = project_changed(PanelView::first(&snapshot), changed);
        assert_eq!(doc.visible.get("tray-battery"), Some(&true));
        assert_eq!(doc.visible.get("tray-battery-050"), Some(&true));
        assert_eq!(doc.visible.get("tray-battery-caution"), Some(&false));

        // Unknown (plugged in, not charging) stays shown by default; only the
        // battery-only-when-relevant setting hides it.
        snapshot.system.power.percent = 87;
        snapshot.system.power.state = PowerState::Unknown;
        let doc = project_changed(PanelView::first(&snapshot), changed);
        assert_eq!(doc.visible.get("tray-battery"), Some(&true));
        let relevant_only = TaskbarItems {
            battery_only_when_relevant: true,
            ..TaskbarItems::default()
        };
        let doc = project_changed(
            PanelView::first(&snapshot).with_items(relevant_only),
            changed,
        );
        assert_eq!(doc.visible.get("tray-battery"), Some(&false));

        // Charging shows the charging glyph, at 80% too.
        snapshot.system.power.percent = 45;
        snapshot.system.power.state = PowerState::Charging;
        let doc = project_changed(PanelView::first(&snapshot), changed);
        assert_eq!(doc.visible.get("tray-battery"), Some(&true));
        assert_eq!(doc.visible.get("tray-battery-050-charging"), Some(&true));
        assert_eq!(doc.visible.get("tray-battery-050"), Some(&false));

        snapshot.system.power.percent = 80;
        let doc = project_changed(PanelView::first(&snapshot), changed);
        assert_eq!(doc.visible.get("tray-battery"), Some(&true));
        assert_eq!(doc.visible.get("tray-battery-080-charging"), Some(&true));
    }

    // A08: each cluster item is its setting AND availability; the clock
    // cluster and workspace pager follow their own settings.
    #[test]
    fn taskbar_item_settings_hide_cluster_items_clock_and_pager() {
        let _scale = crate::taskbar_scale::test_lock();
        crate::taskbar_scale::set_factor(1.0);
        let _guard = icon_test_lock().lock();
        let snapshot = ShellSnapshot::default();
        let hidden = TaskbarItems {
            weather: false,
            volume: false,
            network: false,
            battery: false,
            battery_only_when_relevant: false,
            clock: false,
            workspaces: false,
            ..TaskbarItems::default()
        };
        let mut doc = FakeDoc::default();
        project_panel_async(
            &mut doc,
            PanelView::first(&snapshot).with_items(hidden),
            None,
            &mut TaskIconCache::default(),
        )
        .expect("projects");
        for id in [
            "tray-volume",
            "tray-network",
            "tray-battery",
            "clock-button",
            "workspace-pager",
        ] {
            assert_eq!(doc.visible.get(id), Some(&false), "{id}");
        }
    }

    // A08: status cluster order in the panel template.
    #[test]
    fn panel_status_cluster_order_is_weather_volume_network_battery_clock() {
        let html = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../ui/shell/panel.html"
        ))
        .expect("panel.html");
        let at = |id: &str| {
            html.find(&format!("id=\"{id}\""))
                .unwrap_or_else(|| panic!("{id} missing"))
        };
        let order = [
            "tray-weather",
            "tray-volume",
            "tray-network",
            "tray-battery",
            "clock-button",
        ]
        .map(at);
        assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");
        assert!(at("tray-media") < at("tray-weather"));
        assert!(at("workspace-pager") < at("tray-media"));
    }

    #[test]
    fn empty_scope_is_noop() {
        use crate::runtime::ChangedDomains;
        let _guard = icon_test_lock().lock();
        let snapshot = ShellSnapshot::default();
        let doc = project_changed(PanelView::first(&snapshot), ChangedDomains::default());
        assert!(doc.visible.is_empty(), "empty scope touches nothing");
    }

    #[test]
    fn async_panel_projects_labels_without_sync_resolve() {
        let _scale = crate::taskbar_scale::test_lock();
        crate::taskbar_scale::set_factor(1.0);
        let _guard = icon_test_lock().lock();
        let snapshot = ShellSnapshot::default();
        let mut doc = FakeDoc::default();
        project_panel_async(
            &mut doc,
            PanelView::first(&snapshot),
            None,
            &mut TaskIconCache::default(),
        )
        .expect("projects");
        // No loader/cache: rows apply the Flame fallback synchronously, so
        // images are present but no worker request could be queued.
        assert!(doc.images.is_empty(), "no task states, no icon nodes");
        for index in 0..8 {
            let id = format!("task-slot-{}", index + 1);
            assert!(doc.visible.contains_key(&id), "slot {id} projected");
        }
    }

    #[test]
    fn no_sync_resolve_calls_in_async_path() {
        let sources = [
            include_str!("../async_projection.rs"),
            include_str!("icon_binding.rs"),
            include_str!("icon_cache.rs"),
            include_str!("metrics.rs"),
            include_str!("panel.rs"),
            include_str!("start_view.rs"),
            include_str!("task_slots.rs"),
        ];
        // Import of the loader handle is fine; synchronous resolve calls are not.
        let probe = ["prepare", "_", "name", "("].concat();
        let env = ["from", "_", "environment"].concat();
        for src in sources {
            assert!(!src.contains(&probe), "async path must not resolve sync");
            assert!(
                !src.contains(&env),
                "async path must not construct resolver"
            );
        }
    }
}
