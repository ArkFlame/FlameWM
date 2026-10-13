pub mod drag;
pub mod icon_geometry;

use super::intent::TaskbarIntent;
use flamewm_api::panels::PanelsSnapshot;
use flamewm_api::window::WindowSnapshot;
use flamewm_api::{DesktopAppId, TaskEntryId, WindowRef};
use flamewm_control_core::ControlRequest;
use flamewm_platform::task::{ReorderSession, TaskStrip};
use flamewm_shell_core::{task_click_action, TaskClickAction};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskIntent {
    Activate(WindowRef),
    Minimize(WindowRef),
    Restore(WindowRef),
    Pin {
        app: DesktopAppId,
        expected_revision: u64,
    },
    Unpin {
        app: DesktopAppId,
        expected_revision: u64,
    },
    Reorder {
        entry_id: TaskEntryId,
        index: usize,
        expected_revision: u64,
    },
}

impl TaskIntent {
    #[must_use]
    pub fn into_control_request(self) -> ControlRequest {
        match self {
            Self::Activate(window) => ControlRequest::ActivateWindow(window),
            Self::Minimize(window) => ControlRequest::MinimizeWindow(window),
            Self::Restore(window) => ControlRequest::RestoreWindow(window),
            Self::Pin {
                app,
                expected_revision,
            } => ControlRequest::PinApp {
                app,
                expected_revision,
            },
            Self::Unpin {
                app,
                expected_revision,
            } => ControlRequest::UnpinApp {
                app,
                expected_revision,
            },
            Self::Reorder {
                entry_id,
                index,
                expected_revision,
            } => ControlRequest::ReorderTask {
                entry_id,
                index,
                expected_revision,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskReorderSession {
    revision: u64,
    source: usize,
    session: ReorderSession,
}

impl TaskReorderSession {
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub const fn source(&self) -> usize {
        self.source
    }

    #[must_use]
    pub fn cancel(self, panels: &PanelsSnapshot) -> bool {
        self.revision == panels.revision && self.session.original_order() == current_ids(panels)
    }

    #[must_use]
    pub fn commit(self, panels: &PanelsSnapshot, index: usize) -> Option<TaskIntent> {
        if self.revision != panels.revision
            || self.session.original_order() != current_ids(panels)
            || index >= panels.tasks.len()
        {
            return None;
        }
        Some(TaskIntent::Reorder {
            entry_id: self.session.original_order()[self.source].clone(),
            index,
            expected_revision: self.revision,
        })
    }
}

fn current_ids(panels: &PanelsSnapshot) -> Vec<TaskEntryId> {
    panels.tasks.iter().map(|entry| entry.id.clone()).collect()
}

#[must_use]
pub fn intent_for_click(
    id: &TaskEntryId,
    panels: &PanelsSnapshot,
    windows: &[WindowSnapshot],
) -> Option<TaskbarIntent> {
    let entry = panels.tasks.iter().find(|entry| &entry.id == id)?;
    Some(match task_click_action(entry, windows) {
        TaskClickAction::Launch(app) => TaskbarIntent::Launch(app),
        TaskClickAction::RestoreAndActivate(window) => TaskbarIntent::RestoreAndActivate(window),
        TaskClickAction::Minimize(window) => TaskbarIntent::Minimize(window),
        TaskClickAction::Activate(window) => TaskbarIntent::Activate(window),
    })
}

/// Map a task entry action to a typed operation. Window state stays platform-owned.
#[must_use]
pub fn intent_for_action(
    id: &TaskEntryId,
    action: TaskAction,
    panels: &PanelsSnapshot,
) -> Option<TaskIntent> {
    let entry = panels.tasks.iter().find(|entry| &entry.id == id)?;
    match action {
        TaskAction::Activate => entry_window(entry).map(TaskIntent::Activate),
        TaskAction::Minimize => entry_window(entry).map(TaskIntent::Minimize),
        TaskAction::Restore => entry_window(entry).map(TaskIntent::Restore),
        TaskAction::Close => None,
        TaskAction::Pin => (!entry.id.is_pinned()).then(|| TaskIntent::Pin {
            app: entry.app_id.clone(),
            expected_revision: panels.revision,
        }),
        TaskAction::Unpin => entry.id.is_pinned().then(|| TaskIntent::Unpin {
            app: entry.app_id.clone(),
            expected_revision: panels.revision,
        }),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskAction {
    Activate,
    Minimize,
    Restore,
    Close,
    Pin,
    Unpin,
}

fn entry_window(entry: &flamewm_api::panels::TaskEntry) -> Option<WindowRef> {
    match entry.kind {
        flamewm_api::panels::TaskEntryKind::PinnedSlot { window } => window,
        flamewm_api::panels::TaskEntryKind::Window { window } => Some(window),
    }
}

#[must_use]
pub fn begin_reorder(panels: &PanelsSnapshot, source: usize) -> Option<TaskReorderSession> {
    let mut strip = TaskStrip::default();
    for entry in &panels.tasks {
        match entry.kind {
            flamewm_api::panels::TaskEntryKind::PinnedSlot { window } => {
                strip.pin_app(entry.app_id.clone());
                if let Some(window) = window {
                    strip.add_window(entry.app_id.clone(), window);
                }
            }
            flamewm_api::panels::TaskEntryKind::Window { window } => {
                strip.add_window(entry.app_id.clone(), window);
            }
        };
    }
    Some(TaskReorderSession {
        revision: panels.revision,
        source,
        session: strip.begin_reorder(source)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_api::panels::{TaskEntry, TaskEntryKind};

    fn panels() -> PanelsSnapshot {
        PanelsSnapshot {
            revision: 7,
            panels: Vec::new(),
            tasks: vec![TaskEntry {
                id: TaskEntryId::window(WindowRef::new(4, 1)),
                app_id: DesktopAppId::new("terminal"),
                kind: TaskEntryKind::Window {
                    window: WindowRef::new(4, 1),
                },
                order_index: 0,
            }],
            pinned_apps: Vec::new(),
        }
    }

    #[test]
    fn action_maps_to_control_request_with_snapshot_revision() {
        let snapshot = panels();
        let intent = intent_for_action(&snapshot.tasks[0].id, TaskAction::Pin, &snapshot)
            .expect("unpinned task can be pinned");
        assert_eq!(
            intent.into_control_request(),
            ControlRequest::PinApp {
                app: DesktopAppId::new("terminal"),
                expected_revision: 7,
            }
        );
    }

    #[test]
    fn reorder_session_rejects_changed_revision() {
        let snapshot = panels();
        let session = begin_reorder(&snapshot, 0).expect("task exists");
        let mut changed = snapshot;
        changed.revision = 8;
        assert!(session.commit(&changed, 0).is_none());
    }
}
