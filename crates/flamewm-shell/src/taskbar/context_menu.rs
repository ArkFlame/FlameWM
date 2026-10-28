use flamewm_api::panels::PanelsSnapshot;
use flamewm_api::window::WindowSnapshot;
use flamewm_api::workspace::WorkspaceSnapshot;
use flamewm_ui_core::context_menu::{settings_row, MenuRow, MenuRowKind};
use flamewm_ui_core::MenuEntry;

pub use flamewm_shell_core::menu_policy::{
    task_menu_for_entry, TASK_ROW_ACTIVATE, TASK_ROW_CLOSE, TASK_ROW_PIN, TASK_ROW_UNPIN,
    WORKSPACE_ROW_ACTIVATE, WORKSPACE_ROW_INSERT_AFTER, WORKSPACE_ROW_REMOVE,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextAction {
    ActivateWorkspace {
        index: usize,
        expected_revision: u64,
    },
    InsertWorkspaceAfter {
        index: usize,
        expected_revision: u64,
    },
    RemoveWorkspace {
        index: usize,
        expected_revision: u64,
    },
}

impl ContextAction {
    #[must_use]
    pub fn into_control_request(self) -> flamewm_control_core::ControlRequest {
        use flamewm_control_core::ControlRequest;
        match self {
            Self::ActivateWorkspace {
                index,
                expected_revision,
            } => ControlRequest::ActivateWorkspace {
                index,
                expected_revision,
            },
            Self::InsertWorkspaceAfter {
                index,
                expected_revision,
            } => ControlRequest::InsertWorkspaceAfter {
                index,
                expected_revision,
            },
            Self::RemoveWorkspace {
                index,
                expected_revision,
            } => ControlRequest::RemoveWorkspace {
                index,
                expected_revision,
            },
        }
    }
}

#[must_use]
pub fn workspace_menu(
    snapshot: &WorkspaceSnapshot,
    index: usize,
) -> Vec<(MenuEntry, ContextAction)> {
    if index >= snapshot.count {
        return Vec::new();
    }
    let mut entries = vec![
        (
            MenuEntry::Action {
                id: WORKSPACE_ROW_ACTIVATE.to_owned(),
                label: "Activate".to_owned(),
                icon: None,
                enabled: index != snapshot.active_index,
            },
            ContextAction::ActivateWorkspace {
                index,
                expected_revision: snapshot.revision,
            },
        ),
        (
            MenuEntry::Action {
                id: WORKSPACE_ROW_INSERT_AFTER.to_owned(),
                label: "Insert workspace after".to_owned(),
                icon: None,
                enabled: true,
            },
            ContextAction::InsertWorkspaceAfter {
                index,
                expected_revision: snapshot.revision,
            },
        ),
    ];
    if snapshot.count > 1 {
        entries.push((
            MenuEntry::Action {
                id: WORKSPACE_ROW_REMOVE.to_owned(),
                label: "Remove workspace".to_owned(),
                icon: None,
                enabled: true,
            },
            ContextAction::RemoveWorkspace {
                index,
                expected_revision: snapshot.revision,
            },
        ));
    }
    entries
}

/// Map a stable workspace row id to a typed control, revision-fenced.
/// Row identity is the id, never the label.
#[must_use]
pub fn workspace_action(
    snapshot: &WorkspaceSnapshot,
    index: usize,
    row: &str,
) -> Option<ContextAction> {
    if index >= snapshot.count {
        return None;
    }
    match row {
        WORKSPACE_ROW_ACTIVATE => Some(ContextAction::ActivateWorkspace {
            index,
            expected_revision: snapshot.revision,
        }),
        WORKSPACE_ROW_INSERT_AFTER => Some(ContextAction::InsertWorkspaceAfter {
            index,
            expected_revision: snapshot.revision,
        }),
        WORKSPACE_ROW_REMOVE => (snapshot.count > 1).then_some(ContextAction::RemoveWorkspace {
            index,
            expected_revision: snapshot.revision,
        }),
        _ => None,
    }
}

/// Taskbar-background actions, with stable ids used by the context-menu projection.
pub const TASKBAR_ROW_SETTINGS: &str = "taskbar-settings";
pub const TASKBAR_ROW_TASK_MANAGER: &str = "taskbar-task-manager";

#[must_use]
pub fn taskbar_menu() -> Vec<MenuRow> {
    vec![
        settings_row(TASKBAR_ROW_SETTINGS),
        MenuRow {
            id: TASKBAR_ROW_TASK_MANAGER,
            label: "Task Manager",
            icon: Some("utilities-system-monitor"),
            enabled: true,
            visible: true,
            kind: MenuRowKind::Action,
        },
    ]
}

/// Visible context-menu row count for a menu kind. Row identity is the
/// row id, never the label. Used to size the native surface to its content
/// rows so no spare (black) area remains.
/// Visible context-menu parts (C08): one `MenuPart::Row` per visible row.
/// Height derives from these parts via `menu_policy::context_menu_size`;
/// minimum width is the canonical `MIN_ROW_WIDTH`.
#[must_use]
pub fn visible_row_count_for_kind(
    panels: &PanelsSnapshot,
    windows: &[WindowSnapshot],
    workspaces: Option<&WorkspaceSnapshot>,
    kind: &crate::ContextMenuKind,
) -> usize {
    match kind {
        crate::ContextMenuKind::Task(id) => task_menu_for_entry(panels, windows, id).len(),
        crate::ContextMenuKind::Workspace { index } => {
            workspaces.map_or(0, |snapshot| workspace_menu(snapshot, *index).len())
        }
        crate::ContextMenuKind::Taskbar => taskbar_menu().len(),
    }
}

/// Stable row identity for a task entry slot: entry id plus policy row id.
pub use flamewm_shell_core::menu_policy::row_identity;

#[cfg(test)]
mod taskbar_menu_tests {
    use super::*;
    use flamewm_shell_core::popup::context_menu_size;
    use flamewm_ui_core::context_menu::{
        settings_row, MenuRowKind, MENU_MIN_WIDTH, MENU_PADDING, ROW_HEIGHT, ROW_ICON_SIZE,
    };

    const TASK_MENU_HTML: &str = include_str!("../../../../ui/shell/task-menu.html");

    /// Every `class="menu-row"` button line in the task menu markup.
    fn menu_rows(html: &str) -> Vec<&str> {
        html.lines()
            .filter(|line| line.contains("class=\"menu-row\""))
            .collect()
    }

    fn row_for<'a>(html: &'a str, id: &str) -> &'a str {
        let marker = format!("id=\"{id}\"");
        menu_rows(html)
            .into_iter()
            .find(|line| line.contains(&marker))
            .unwrap_or_else(|| panic!("no menu row `{id}` in markup"))
    }

    #[test]
    fn every_markup_menu_row_has_canonical_icon_size_and_action() {
        let rows = menu_rows(TASK_MENU_HTML);
        assert_eq!(rows.len(), 9);
        for line in rows {
            assert!(
                line.contains(&format!(
                    "width=\"{ROW_ICON_SIZE}\" height=\"{ROW_ICON_SIZE}\""
                )),
                "{line}"
            );
            assert!(line.contains("data-action=\""), "{line}");
        }
    }

    #[test]
    fn markup_rows_carry_policy_row_ids_and_actions() {
        let expected = [
            (
                format!("context-task-{TASK_ROW_ACTIVATE}"),
                "task.menu.activate",
            ),
            (format!("context-task-{TASK_ROW_CLOSE}"), "task.menu.close"),
            (format!("context-task-{TASK_ROW_PIN}"), "task.menu.pin"),
            (format!("context-task-{TASK_ROW_UNPIN}"), "task.menu.unpin"),
            (
                format!("context-workspace-{WORKSPACE_ROW_ACTIVATE}"),
                "workspace.menu.activate",
            ),
            (
                format!("context-workspace-{WORKSPACE_ROW_INSERT_AFTER}"),
                "workspace.menu.insert-after",
            ),
            (
                format!("context-workspace-{WORKSPACE_ROW_REMOVE}"),
                "workspace.menu.remove",
            ),
            (
                format!("context-{TASKBAR_ROW_SETTINGS}"),
                "taskbar.menu.settings",
            ),
            (
                format!("context-{TASKBAR_ROW_TASK_MANAGER}"),
                "taskbar.menu.task-manager",
            ),
        ];
        for (id, action) in expected {
            let row = row_for(TASK_MENU_HTML, &id);
            assert!(row.contains(&format!("data-action=\"{action}\"")), "{row}");
        }
    }

    #[test]
    fn taskbar_kind_size_matches_its_single_settings_row() {
        let rows = taskbar_menu();
        assert_eq!(rows.len(), 2);
        let size = context_menu_size(rows.len());
        assert_eq!(size.width, MENU_MIN_WIDTH);
        assert_eq!(size.height, 2 * MENU_PADDING + 2 * ROW_HEIGHT);
    }

    #[test]
    fn workspace_kind_size_matches_its_markup_rows() {
        let snapshot = WorkspaceSnapshot {
            revision: 1,
            count: 2,
            active_index: 0,
            last_index: None,
            names: vec!["1".to_owned(), "2".to_owned()],
        };
        let rows = workspace_menu(&snapshot, 0);
        assert_eq!(rows.len(), 3);
        let size = context_menu_size(rows.len());
        assert_eq!(size.width, MENU_MIN_WIDTH);
        assert_eq!(size.height, 2 * MENU_PADDING + 3 * ROW_HEIGHT);
        for (entry, _) in rows {
            let MenuEntry::Action { id, .. } = entry else {
                panic!("workspace menu must contain only action rows");
            };
            row_for(TASK_MENU_HTML, &format!("context-workspace-{id}"));
        }
    }

    #[test]
    fn settings_markup_row_matches_shared_settings_row_contract() {
        let row = settings_row(TASKBAR_ROW_SETTINGS);
        let markup = row_for(TASK_MENU_HTML, &format!("context-{}", row.id));
        assert!(markup.contains(&format!("src=\"assets/breeze/{}.svg\"", row.icon.unwrap())));
        assert!(markup.contains(&format!("<span>{}</span>", row.label)));
        assert!(markup.contains("data-action=\"taskbar.menu.settings\""));
    }

    #[test]
    fn taskbar_settings_row_is_the_only_visible_taskbar_row() {
        let policy_ids: Vec<String> = taskbar_menu()
            .iter()
            .map(|row| format!("context-{}", row.id))
            .collect();
        assert_eq!(
            policy_ids,
            vec![
                "context-taskbar-settings".to_owned(),
                "context-taskbar-task-manager".to_owned()
            ]
        );
    }

    #[test]
    fn taskbar_background_menu_has_exactly_one_flamewm_settings_row() {
        let rows = taskbar_menu();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], settings_row(TASKBAR_ROW_SETTINGS));
        assert_eq!(rows[1].label, "Task Manager");
        assert_eq!(rows[0].kind, MenuRowKind::Action);
        assert_eq!(rows[0].label, "FlameWM Settings");
        assert_eq!(rows[0].icon, Some("preferences-desktop"));
    }

    #[test]
    fn task_menu_frame_is_the_shared_neutral_border() {
        const SHARED_MENU_CSS: &str = include_str!("../../../../ui/shared/menu.css");
        let frame = SHARED_MENU_CSS
            .lines()
            .find(|line| line.starts_with(".context-menu {"))
            .expect("shared menu frame rule");
        assert!(frame.contains("border:1px solid #ffffff3f"), "{frame}");
        assert!(
            !TASK_MENU_HTML.contains("style="),
            "task menu markup must not override the frame inline"
        );
    }

    #[test]
    fn task_and_desktop_menu_rows_share_one_structure() {
        const DESKTOP_HTML: &str = include_str!("../../../../ui/desktop/index.html");
        for (name, html) in [("task", TASK_MENU_HTML), ("desktop", DESKTOP_HTML)] {
            // `confirm.*`/`downloads.notice.*` rows live in the text-only `.desktop-confirm` notice dialogs, not icon menus.
            let rows: Vec<&str> = menu_rows(html)
                .into_iter()
                .filter(|line| {
                    !line.contains("data-action=\"confirm.")
                        && !line.contains("data-action=\"downloads.notice.")
                })
                .collect();
            assert!(!rows.is_empty(), "{name} menu has rows");
            for line in rows {
                assert!(line.contains("<img "), "{name}: {line}");
                assert!(
                    line.contains("<span>") && line.contains("</span></button>"),
                    "{name}: {line}"
                );
            }
        }
    }

    #[test]
    fn taskbar_html_uses_shared_labels_and_settings_icon() {
        let html = include_str!("../../../../ui/shell/task-menu.html");
        assert!(
            !html.contains("Add Virtual Desktop"),
            "unrouted row removed"
        );
        assert!(!html.contains("Create New Virtual Desktop"));
        assert!(html.contains(
            "preferences-desktop.svg\" width=\"16\" height=\"16\"><span>FlameWM Settings</span>"
        ));
    }
}
