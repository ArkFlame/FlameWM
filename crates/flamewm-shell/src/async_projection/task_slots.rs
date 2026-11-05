//! Taskbar task-slot projection, task icon resolution and icon priming.
use super::*;

pub(super) const TASK_LINE_IDLE: UiColor = UiColor {
    r: 135,
    g: 145,
    b: 155,
    a: 255,
};

pub(super) fn task_states(view: PanelView<'_>) -> Vec<flamewm_shell_core::TaskVisualState> {
    let snapshot = view.snapshot;
    flamewm_shell_core::task_visual_states_on_output_and_workspace(
        &snapshot.panels,
        &snapshot.windows,
        &snapshot.applications,
        view.panel.map(|panel| &panel.output),
        snapshot
            .workspaces
            .as_ref()
            .map(|workspaces| workspaces.active_index),
    )
}

/// Icon lookup name for one task slot. A window with no desktop entry of
/// its own resolves through a desktop entry whose `StartupWMClass` matches
/// its WM_CLASS-derived name; otherwise the state's name is used unchanged.
pub(super) fn task_icon_name(
    cache: &mut TaskIconCache,
    state: &flamewm_shell_core::TaskVisualState,
    applications: &[flamewm_api::applications::DesktopApplication],
) -> String {
    if let Some(entry) = cache
        .resolved
        .iter()
        .find(|entry| entry.app_id == state.app_id && entry.icon_in == state.icon_name)
    {
        return entry.icon_out.clone();
    }
    let has_own_entry = applications
        .iter()
        .any(|application| application.id == state.app_id);
    let icon_out = if has_own_entry {
        state.icon_name.clone()
    } else {
        desktop_icon_for_wm_class(applications, &state.icon_name)
            .unwrap_or_else(|| state.icon_name.clone())
    };
    cache.resolved.retain(|entry| entry.app_id != state.app_id);
    cache.resolved.push(ResolvedTaskIcon {
        app_id: state.app_id.clone(),
        icon_in: state.icon_name.clone(),
        icon_out: icon_out.clone(),
    });
    icon_out
}

/// One resolved task icon, keyed by `app_id` and the state icon it came from.
pub(super) struct ResolvedTaskIcon {
    app_id: DesktopAppId,
    icon_in: String,
    icon_out: String,
}

/// Task icon resolutions valid for one applications list. `sync` drops every
/// entry when the list's icon-relevant fields differ from the last refresh.
/// Owned by the caller (app state), passed to each task-slot projection.
#[derive(Default)]
pub struct TaskIconCache {
    /// (id, `icon_name`, `startup_wm_class`) of the list the entries were built from.
    applications: Vec<(DesktopAppId, String, String)>,
    resolved: Vec<ResolvedTaskIcon>,
}

impl TaskIconCache {
    fn sync(&mut self, applications: &[flamewm_api::applications::DesktopApplication]) {
        let unchanged = self.applications.len() == applications.len()
            && self
                .applications
                .iter()
                .zip(applications)
                .all(|((id, icon, wm_class), app)| {
                    *id == app.id && *icon == app.icon_name && *wm_class == app.startup_wm_class
                });
        if unchanged {
            return;
        }
        self.applications = applications
            .iter()
            .map(|app| {
                (
                    app.id.clone(),
                    app.icon_name.clone(),
                    app.startup_wm_class.clone(),
                )
            })
            .collect();
        self.resolved.clear();
    }
}

/// Desktop-entry `Icon=` whose `StartupWMClass` equals one lookup candidate
/// of `name` (case-insensitive). Non-empty icons only.
#[must_use]
pub fn desktop_icon_for_wm_class(
    applications: &[flamewm_api::applications::DesktopApplication],
    name: &str,
) -> Option<String> {
    crate::icon_loader::icon_name_candidates(name)
        .iter()
        .find_map(|candidate| {
            applications
                .iter()
                .find(|application| {
                    !application.icon_name.is_empty()
                        && application.startup_wm_class.eq_ignore_ascii_case(candidate)
                })
                .map(|application| application.icon_name.clone())
        })
}

/// Startup task-icon prime: whether the one-shot task-slot projection may run.
/// It needs live task states to request icons for and a loader to request on.
#[must_use]
pub fn task_icon_prime_should_run(
    armed: bool,
    task_state_count: usize,
    loader_present: bool,
) -> bool {
    armed && task_state_count > 0 && loader_present
}

/// Startup task-icon prime: whether the prime stays armed after a run.
/// Disarms only once a run with task states, a loader, and a successful
/// projection has submitted icon requests; an empty snapshot keeps it armed.
#[must_use]
pub fn task_icon_prime_still_armed(
    armed: bool,
    task_state_count: usize,
    loader_present: bool,
    projection_ok: bool,
) -> bool {
    armed && !(task_state_count > 0 && loader_present && projection_ok)
}

/// `WindowsChanged` -> task slots only. Resolves task visual states (cheap
/// in-memory join, no fetch) and projects labels/icon/visibility/background
/// and the status-line geometry for the 8 task slots. Pager, tray, and
/// geometry nodes are untouched, so a focus-only window update never
/// reprojects the full panel.
pub fn project_task_slots_async(
    document: &mut impl UiDocumentAccess,
    view: PanelView<'_>,
    mut loader: Option<&mut IconLoader>,
    icon_cache: &mut TaskIconCache,
) -> Result<(), String> {
    let _span = crate::runtime::shell_span("shell.tasks.project").start();
    bump_task_view_epoch();
    let snapshot = view.snapshot;
    let states = task_states(view);
    icon_cache.sync(&snapshot.applications);
    // Task-slot nodes only: labels, icons, line visibility, backgrounds,
    // slot visibility. Pager/tray/geometry untouched.
    let (mut slot_id, mut icon_id, mut label_id, mut line_id) =
        (String::new(), String::new(), String::new(), String::new());
    for index in 0..TASK_SLOT_COUNT {
        let n = index + 1;
        slot_id.clear();
        write!(slot_id, "task-slot-{n}").expect("write to String is infallible");
        icon_id.clear();
        write!(icon_id, "task-slot-{n}-icon").expect("write to String is infallible");
        label_id.clear();
        write!(label_id, "task-slot-{n}-label").expect("write to String is infallible");
        line_id.clear();
        write!(line_id, "task-slot-{n}-line").expect("write to String is infallible");
        crate::projection::project_task_progress(
            document,
            index,
            states.get(index).map(|state| &state.id),
            view.task_progress,
        )?;
        if let Some(state) = states.get(index) {
            let label = snapshot
                .applications
                .iter()
                .find(|application| application.id == state.app_id)
                .map(|application| application.name.as_str())
                .filter(|name| !name.is_empty())
                .unwrap_or(state.app_id.as_str());
            document.text_ref(&label_id, label)?;
            let px = crate::projection::task_icon_device_px(view.output);
            schedule_visible(
                document,
                &icon_id,
                &mut loader.as_deref_mut(),
                IconTarget::TaskSlot(index as u8),
                &task_icon_name(icon_cache, state, &snapshot.applications),
                px,
                px,
            )?;
            document.visible(&line_id, state.running)?;
            document.background_clear(&slot_id)?;
            let line = if state.focused {
                crate::projection::task_accent()
            } else {
                TASK_LINE_IDLE
            };
            document.background(&line_id, line)?;
            crate::projection::project_task_line(document, &line_id, state.focused)?;
        } else {
            document.text(&label_id, String::new())?;
            document.visible(&line_id, false)?;
            document.background_clear(&slot_id)?;
            crate::projection::project_task_line(document, &line_id, false)?;
        }
        document.visible(
            &slot_id,
            view.items.activities && states.get(index).is_some(),
        )?;
    }
    crate::projection::project_task_slots_extent(
        document,
        crate::projection::task_slots_rendered(states.len(), view.items),
    )?;
    document.visible("task-slot-insert", view.items.activities)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::async_projection::tests::*;
    use flamewm_api::applications::DesktopApplication;

    #[test]
    fn task_icon_prime_stays_armed_until_a_request_round_runs() {
        // Empty snapshot at first tick: nothing to request, stays armed.
        assert!(!task_icon_prime_should_run(true, 0, true));
        assert!(task_icon_prime_still_armed(true, 0, true, true));
        // No loader yet: cannot request, stays armed.
        assert!(!task_icon_prime_should_run(true, 2, false));
        assert!(task_icon_prime_still_armed(true, 2, false, true));
        // Failed projection: stays armed for a retry.
        assert!(task_icon_prime_still_armed(true, 2, true, false));
        // Task states plus loader plus successful projection: disarms.
        assert!(task_icon_prime_should_run(true, 2, true));
        assert!(!task_icon_prime_still_armed(true, 2, true, true));
        // Once disarmed it never re-runs.
        assert!(!task_icon_prime_should_run(false, 2, true));
        assert!(!task_icon_prime_still_armed(false, 2, true, true));
    }

    #[test]
    fn task_slot_bindings_keep_one_edge_per_output_scale() {
        let _guard = icon_test_lock().lock();
        test_reset_icon_bindings();
        let mut doc = FakeDoc::default();
        // Two outputs share TaskSlot(0) but project different icon edges.
        test_record_binding(IconTarget::TaskSlot(0), "app", 25, 25, 1);
        test_record_binding(IconTarget::TaskSlot(0), "app", 31, 31, 1);
        let at_25 = apply_icon_result(
            &mut doc,
            IconTarget::TaskSlot(0),
            "app",
            25,
            25,
            1,
            sized_test_image("a", 25),
        )
        .expect("edge 25 ok");
        let at_31 = apply_icon_result(
            &mut doc,
            IconTarget::TaskSlot(0),
            "app",
            31,
            31,
            1,
            sized_test_image("a", 31),
        )
        .expect("edge 31 ok");
        assert!(at_25 && at_31, "both edges keep their own binding");
        assert_eq!(doc.images.len(), 2);
    }

    #[test]
    fn wm_class_matches_desktop_entry_icon_before_fallback() {
        let app = DesktopApplication {
            id: flamewm_api::DesktopAppId::new("xterm-desktop"),
            name: "XTerm".to_owned(),
            generic_name: String::new(),
            comment: String::new(),
            startup_wm_class: "XTerm".to_owned(),
            argv: Vec::new(),
            keywords: Vec::new(),
            icon_name: "utilities-terminal".to_owned(),
            categories: Vec::new(),
        };
        assert_eq!(
            desktop_icon_for_wm_class(std::slice::from_ref(&app), "mini.xterm").as_deref(),
            Some("utilities-terminal")
        );
        assert_eq!(
            desktop_icon_for_wm_class(std::slice::from_ref(&app), "gimp"),
            None
        );
    }

    #[test]
    fn task_slot_node_id_uses_source_ids() {
        assert_eq!(
            icon_node_id_for_target(IconTarget::TaskSlot(2)),
            "task-slot-3-icon"
        );
        assert_eq!(
            icon_node_id_for_target(IconTarget::StartSlot(0)),
            "start-app-slot-1-icon"
        );
    }

    #[test]
    fn task_slots_list_only_windows_on_the_shown_panel_output() {
        use flamewm_api::panels::{
            PanelEdge, PanelSnapshot, PanelsSnapshot, TaskEntry, TaskEntryKind,
        };
        use flamewm_api::window::{WindowSnapshot, WindowState};
        use flamewm_api::{DesktopAppId, OutputId, Rect, TaskEntryId, WindowRef, WorkspaceRef};
        let _guard = icon_test_lock().lock();
        let window_on = |id: u64, output: &str| WindowSnapshot {
            reference: WindowRef::new(id, 1),
            title: String::new(),
            app_id: DesktopAppId::new("term"),
            outer_geometry: Rect::new(0, 0, 10, 10),
            restore_geometry: Rect::new(0, 0, 10, 10),
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
        let task = |id: u64| {
            let window = WindowRef::new(id, 1);
            TaskEntry {
                id: TaskEntryId::window(window),
                app_id: DesktopAppId::new("term"),
                kind: TaskEntryKind::Window { window },
                order_index: id as usize,
            }
        };
        let mut snapshot = ShellSnapshot {
            panels: PanelsSnapshot {
                revision: 1,
                panels: vec![PanelSnapshot {
                    output: OutputId::new("A"),
                    edge: PanelEdge::Bottom,
                    logical_size: 40,
                    visible: true,
                    geometry: Rect::new(0, 760, 1000, 40),
                }],
                tasks: vec![task(1), task(2)],
                pinned_apps: Vec::new(),
            },
            windows: vec![window_on(1, "A"), window_on(2, "B")],
            ..ShellSnapshot::default()
        };
        let mut doc = FakeDoc::default();
        project_task_slots_async(
            &mut doc,
            PanelView::first(&snapshot),
            None,
            &mut TaskIconCache::default(),
        )
        .expect("projects");
        assert_eq!(doc.visible.get("task-slot-1"), Some(&true));
        assert_eq!(doc.visible.get("task-slot-2"), Some(&false));
        // Window 1 moved A->B; window 2 is already on B. Panel A lists neither.
        snapshot.windows[0].output = OutputId::new("B");
        let mut doc = FakeDoc::default();
        project_task_slots_async(
            &mut doc,
            PanelView::first(&snapshot),
            None,
            &mut TaskIconCache::default(),
        )
        .expect("panel A projects");
        assert_eq!(doc.visible.get("task-slot-1"), Some(&false));
        assert_eq!(doc.visible.get("task-slot-2"), Some(&false));
        // Panel B lists both.
        snapshot.panels.panels[0].output = OutputId::new("B");
        let mut doc = FakeDoc::default();
        project_task_slots_async(
            &mut doc,
            PanelView::first(&snapshot),
            None,
            &mut TaskIconCache::default(),
        )
        .expect("panel B projects");
        assert_eq!(doc.visible.get("task-slot-1"), Some(&true));
        assert_eq!(doc.visible.get("task-slot-2"), Some(&true));

        // Same output, different workspaces. A workspace-only snapshot
        // change must replace both the task label AND the click-target slot;
        // minimized windows belong to their original workspace too.
        snapshot.panels.tasks[0].app_id = DesktopAppId::new("first-app");
        snapshot.panels.tasks[1].app_id = DesktopAppId::new("second-app");
        snapshot.windows[0].app_id = DesktopAppId::new("first-app");
        snapshot.windows[1].app_id = DesktopAppId::new("second-app");
        snapshot.windows[0].workspace.index = 0;
        snapshot.windows[1].workspace.index = 1;
        snapshot.windows[1].state = WindowState::Minimized;
        snapshot.workspaces = Some(flamewm_api::workspace::WorkspaceSnapshot {
            revision: 1,
            count: 2,
            active_index: 0,
            last_index: None,
            names: vec![],
        });
        let changed = crate::runtime::ChangedDomains {
            workspaces: true,
            ..Default::default()
        };
        let first = project_changed(PanelView::first(&snapshot), changed);
        assert_eq!(first.visible.get("task-slot-1"), Some(&true));
        assert_eq!(first.visible.get("task-slot-2"), Some(&false));
        assert_eq!(
            first.texts.get("task-slot-1-label").map(String::as_str),
            Some("first-app")
        );
        snapshot
            .workspaces
            .as_mut()
            .expect("workspace snapshot")
            .active_index = 1;
        let second = project_changed(PanelView::first(&snapshot), changed);
        assert_eq!(second.visible.get("task-slot-1"), Some(&true));
        assert_eq!(second.visible.get("task-slot-2"), Some(&false));
        assert_eq!(
            second.texts.get("task-slot-1-label").map(String::as_str),
            Some("second-app")
        );
    }

    #[test]
    fn eight_slots_project_distinct_ids() {
        let mut ids = std::collections::HashSet::new();
        for index in 0..START_APP_SLOT_COUNT {
            ids.insert(format!("start-app-slot-{}", index + 1));
        }
        assert_eq!(ids.len(), 8);
    }

    #[test]
    fn task_progress_css_is_a_background_fill_behind_slot_content() {
        let css = include_str!("../../../../ui/shell/flamewm.css");
        let html = include_str!("../../../../ui/shell/panel.html");
        let rule = css
            .split('}')
            .find(|rule| rule.contains(".task-progress {"))
            .expect("task-progress rule");
        assert!(!rule.contains("z-index"), "the shell CSS compiler has no stacking token");
        assert!(rule.contains("height:100%"), "fill spans the slot height");
        assert!(rule.contains("var(--accent-soft)"), "fill uses the accent tint token");
        assert!(!rule.contains("bottom:"), "the old bottom track is gone");
        let icon = css
            .split('}')
            .find(|rule| rule.contains(".task-slot-icon"))
            .expect("task-slot icon rule");
        assert!(icon.contains("position:relative"), "icon paints above the fill");
        for index in 1..=8 {
            let fill = html
                .find(&format!("id=\"task-slot-{index}-progress\""))
                .expect("fill node");
            let icon = html
                .find(&format!("id=\"task-slot-{index}-icon\""))
                .expect("icon node");
            assert!(fill < icon, "fill precedes icon in tree order");
        }
    }

    #[test]
    fn task_progress_is_slot_identity_bound_scaled_and_cleared() {
        let _scale = crate::taskbar_scale::test_lock();
        crate::taskbar_scale::set_factor(1.0);
        let id = flamewm_api::TaskEntryId::new("win:7:1");
        let entry = flamewm_api::panels::TaskProgressEntry {
            task_id: id.clone(),
            progress: flamewm_api::panels::TaskProgress::new(50).unwrap(),
        };
        let mut doc = FakeDoc::default();
        crate::projection::project_task_progress(&mut doc, 0, Some(&id), &[entry]).unwrap();
        assert_eq!(doc.visible.get("task-slot-1-progress"), Some(&true));
        let inner = crate::taskbar_scale::inner_node_height(1.0);
        assert_eq!(doc.sizes.get("task-slot-1-progress"), Some(&(21.0, inner)));

        crate::projection::project_task_progress(&mut doc, 0, None, &[]).unwrap();
        assert_eq!(doc.visible.get("task-slot-1-progress"), Some(&false));
        assert_eq!(doc.sizes.get("task-slot-1-progress"), Some(&(0.0, inner)));
        assert_eq!(doc.visible.get("task-slot-2-progress"), None);
    }
}
