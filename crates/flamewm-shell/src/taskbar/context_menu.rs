use flamewm_api::panels::PanelsSnapshot;
use flamewm_api::window::WindowSnapshot;
use flamewm_api::workspace::WorkspaceSnapshot;
use flamewm_ui_core::MenuEntry;

pub use flamewm_shell_core::menu_policy::{
    task_menu_for_entry, TASK_ROW_ACTIVATE, TASK_ROW_CLOSE, TASK_ROW_UNPIN, WORKSPACE_ROW_ACTIVATE,
    WORKSPACE_ROW_INSERT_AFTER, WORKSPACE_ROW_REMOVE,
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
    let mut entries = vec![(
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
    )];
    entries.push((
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
    ));
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
        WORKSPACE_ROW_REMOVE => {
            if snapshot.count > 1 {
                Some(ContextAction::RemoveWorkspace {
                    index,
                    expected_revision: snapshot.revision,
                })
            } else {
                None
            }
        }
        _ => None,
    }
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
        crate::ContextMenuKind::Workspace { index } => workspaces
            .map(|snapshot| workspace_menu(snapshot, *index).len())
            .unwrap_or(0),
    }
}

/// Stable row identity for a task entry slot: entry id plus policy row id.
pub use flamewm_shell_core::menu_policy::row_identity;
