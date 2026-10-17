pub mod drag;
pub mod icon_geometry;

use super::intent::TaskbarIntent;
use flamewm_api::panels::PanelsSnapshot;
use flamewm_api::window::WindowSnapshot;
use flamewm_api::{TaskEntryId, WindowRef};
use flamewm_control_core::ControlRequest;
use flamewm_platform::task::{ReorderSession, TaskStrip};
use flamewm_shell_core::{task_click_action, TaskClickAction};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskIntent {
    Activate(WindowRef),
    Minimize(WindowRef),
    Restore(WindowRef),
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
        self.revision == panels.revision && same_order(self.session.original_order(), panels)
    }

    #[must_use]
    pub fn commit(self, panels: &PanelsSnapshot, index: usize) -> Option<TaskIntent> {
        if self.revision != panels.revision
            || !same_order(self.session.original_order(), panels)
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

fn same_order(original: &[TaskEntryId], panels: &PanelsSnapshot) -> bool {
    original.iter().eq(panels.tasks.iter().map(|entry| &entry.id))
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
        TaskClickAction::RestoreAndActivate(window) => {
            flamewm_debug::emit(
                flamewm_debug::DebugEventId("restore.trace.shell.click"),
                std::time::Duration::ZERO,
                || {
                    let ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_millis());
                    format!("t_ms={ms} window={window:?} intent=RestoreAndActivate")
                },
            );
            TaskbarIntent::RestoreAndActivate(window)
        }
        TaskClickAction::Minimize(window) => TaskbarIntent::Minimize(window),
        TaskClickAction::Activate(window) => TaskbarIntent::Activate(window),
    })
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
    use flamewm_api::DesktopAppId;

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
    fn reorder_session_rejects_changed_revision() {
        let snapshot = panels();
        let session = begin_reorder(&snapshot, 0).expect("task exists");
        let mut changed = snapshot;
        changed.revision = 8;
        assert!(session.commit(&changed, 0).is_none());
    }
}
