use super::*;

impl ShellRuntime {
    /// Maps a pager cell action (`workspace-{k}` / `workspace.{k}`) to the
    /// activation control for the workspace the live snapshot places in cell
    /// `k`. Cells are column-major and count-dependent, so an empty cell (or a
    /// missing snapshot) is a no-op instead of a nearby workspace.
    #[must_use]
    pub fn control_request_for_action(&self, action: &str) -> Option<ControlRequest> {
        let cell = action
            .strip_prefix("workspace-")
            .or_else(|| action.strip_prefix("workspace."))
            .and_then(|value| value.parse::<usize>().ok())?;
        let snapshot = self.snapshot.workspaces.as_ref()?;
        let index = crate::taskbar::workspaces::cell_index(cell, snapshot.count)?;
        crate::taskbar::workspaces::control_request_for_click(Some(snapshot), index)
    }

    /// Map a stable context-menu row action (`task.menu.<row>` /
    /// `workspace.menu.<row>`) plus the open menu state to a typed control.
    /// Row identity is the row id, never the label. Pinned task entries
    /// resolve `unpin`; running entries resolve `activate`/`close`/`pin`.
    /// Workspace rows are revision-fenced against the
    /// open menu.
    #[must_use]
    pub fn context_menu_request_for_action(
        &self,
        action: &str,
        menu: Option<&ContextMenuState>,
    ) -> Option<ControlRequest> {
        let menu = menu?;
        match &menu.kind {
            ContextMenuKind::Task(id) => {
                if self.snapshot.panels.revision != menu.revision {
                    return None;
                }
                let row = action.strip_prefix("task.menu.")?;
                let entry = self
                    .snapshot
                    .panels
                    .tasks
                    .iter()
                    .find(|entry| &entry.id == id)?;
                if entry.id.is_pinned() {
                    if row != crate::taskbar::context_menu::TASK_ROW_UNPIN {
                        return None;
                    }
                    return Some(ControlRequest::UnpinApp {
                        app: entry.app_id.clone(),
                        expected_revision: self.snapshot.panels.revision,
                    });
                }
                let window = match entry.kind {
                    flamewm_api::panels::TaskEntryKind::PinnedSlot { window } => window,
                    flamewm_api::panels::TaskEntryKind::Window { window } => Some(window),
                }?;
                if !self
                    .snapshot
                    .windows
                    .iter()
                    .any(|snapshot| snapshot.reference == window)
                {
                    return None;
                }
                match row {
                    crate::taskbar::context_menu::TASK_ROW_ACTIVATE => {
                        Some(ControlRequest::ActivateWindow(window))
                    }
                    crate::taskbar::context_menu::TASK_ROW_CLOSE => {
                        Some(ControlRequest::CloseWindow(window))
                    }
                    crate::taskbar::context_menu::TASK_ROW_PIN => Some(ControlRequest::PinApp {
                        app: entry.app_id.clone(),
                        expected_revision: self.snapshot.panels.revision,
                    }),
                    _ => None,
                }
            }
            ContextMenuKind::Taskbar => None,
            ContextMenuKind::Workspace { index } => {
                let row = action.strip_prefix("workspace.menu.")?;
                let snapshot = self.snapshot.workspaces.as_ref()?;
                if snapshot.revision != menu.revision {
                    return None;
                }
                crate::taskbar::context_menu::workspace_action(snapshot, *index, row)
                    .map(crate::taskbar::context_menu::ContextAction::into_control_request)
            }
        }
    }

    /// Typed `SystemAction` control for audio rows from a provider snapshot.
    /// The provider owns volumes/mutes; the shell only forwards the id-keyed
    /// action. Rejects out-of-range percents the same way the provider does.
    #[must_use]
    pub fn audio_system_action_for_row(
        &self,
        row_id: &str,
        percent: Option<u8>,
        muted: Option<bool>,
    ) -> Option<ControlRequest> {
        crate::taskbar::status::audio::system_action_for_row(
            &self.snapshot.system,
            row_id,
            percent,
            muted,
        )
    }

    /// Typed `SystemAction` control for network rows from a provider snapshot.
    /// Secrets travel via the `SecretAgent` snapshot only; this never carries
    /// a secret value.
    #[must_use]
    pub fn network_system_action_for_path(
        &self,
        path: &str,
        scan: bool,
        disconnect: bool,
    ) -> Option<ControlRequest> {
        crate::taskbar::status::network::system_action_for_path(
            &self.snapshot.system,
            path,
            scan,
            disconnect,
        )
    }
}
