use super::*;

impl ShellRuntime {
    /// Open a task context menu for a press-captured identity, rejecting a
    /// stale release instead of opening a menu for the task now in that slot.
    pub fn open_task_context_for_target(
        &mut self,
        target: &TaskSlotTarget,
        pointer: Option<flamewm_api::Point>,
    ) {
        self.open_task_context_for_target_scoped(target, pointer, None);
    }

    pub fn open_task_context_for_target_scoped(
        &mut self,
        target: &TaskSlotTarget,
        pointer: Option<flamewm_api::Point>,
        interaction: Option<InteractionContext>,
    ) {
        if self.current_task_for_target(target).is_none() {
            return;
        }
        self.push_context_menu(
            ContextMenuKind::Task(target.id.clone()),
            self.snapshot.panels.revision,
            pointer,
            interaction,
        );
    }

    /// Pointer-anchored variant: `pointer` is the root-coordinate click
    /// point (local x/y + source surface global rect).
    pub fn open_task_context_at_slot(&mut self, slot: usize, pointer: Option<flamewm_api::Point>) {
        let states = self.task_visual_states();
        if let Some(state) = states.get(slot) {
            let kind = ContextMenuKind::Task(state.id.clone());
            self.push_context_menu(kind, self.snapshot.panels.revision, pointer, None);
        }
    }

    /// Opens the overflow popup that lists the taskbar windows the shown output does not render.
    pub fn open_task_overflow_menu(&mut self, pointer: Option<flamewm_api::Point>) {
        let entries = self.task_overflow_entries();
        if entries.is_empty() {
            return;
        }
        self.push_context_menu(
            ContextMenuKind::TaskOverflow { entries },
            self.snapshot.panels.revision,
            pointer,
            None,
        );
    }

    /// Activatable windows past the rendered slots, split exactly as the taskbar projection splits them.
    pub(crate) fn task_overflow_entries(&self) -> Vec<flamewm_shell_core::TaskVisualState> {
        let Some(output) = self.shown_panel_output() else {
            return Vec::new();
        };
        let states = self.task_states_on(output);
        let panel = self
            .snapshot
            .panels
            .panels
            .iter()
            .find(|panel| &panel.output == output);
        let scale = self
            .snapshot
            .displays
            .outputs
            .iter()
            .find(|candidate| &candidate.id == output)
            .map_or(100, |candidate| candidate.shell_scale_percent);
        let capacity = crate::projection::task_slot_capacity_for_panel(panel, scale);
        let (rendered, _) = crate::projection::task_slot_split(states.len(), true, capacity);
        states
            .into_iter()
            .skip(rendered)
            .filter(|state| state.window.is_some())
            .collect()
    }

    /// Opens the flat dbusmenu popup for a tray item at a root-coordinate pointer anchor.
    pub fn open_tray_menu(
        &mut self,
        key: flamewm_tray::ItemKey,
        menu_path: String,
        rows: Vec<flamewm_tray::TrayMenuRow>,
        pointer: flamewm_api::Point,
    ) {
        self.push_context_menu(
            ContextMenuKind::TrayItem {
                key,
                menu_path,
                rows,
            },
            self.snapshot.panels.revision,
            Some(pointer),
            None,
        );
    }

    pub(super) fn push_context_menu(
        &mut self,
        kind: ContextMenuKind,
        revision: u64,
        anchor: Option<flamewm_api::Point>,
        interaction: Option<InteractionContext>,
    ) {
        self.controls
            .push_back(ShellControl::OpenContextMenu(ContextMenuState {
                kind,
                revision,
                output: interaction
                    .as_ref()
                    .map(|context| context.output.clone())
                    .or_else(|| self.selected_output()),
                interaction,
                anchor,
            }));
    }

    /// Pointer-anchored variant: `pointer` is the root-coordinate click
    /// point (local x/y + source surface global rect). `cell` is a pager cell
    /// id; an empty cell opens nothing.
    pub fn open_workspace_context_at_cell(
        &mut self,
        cell: usize,
        pointer: Option<flamewm_api::Point>,
    ) {
        self.push_workspace_context(cell, pointer, None);
    }

    pub(super) fn push_workspace_context(
        &mut self,
        cell: usize,
        pointer: Option<flamewm_api::Point>,
        interaction: Option<InteractionContext>,
    ) {
        let Some(snapshot) = self.snapshot.workspaces.as_ref() else {
            return;
        };
        let Some(index) = crate::taskbar::workspaces::cell_index(cell, snapshot.count) else {
            return;
        };
        self.push_context_menu(
            ContextMenuKind::Workspace { index },
            snapshot.revision,
            pointer,
            interaction,
        );
    }

    pub fn open_workspace_context_at_cell_scoped(
        &mut self,
        cell: usize,
        pointer: Option<flamewm_api::Point>,
        interaction: InteractionContext,
    ) {
        self.push_workspace_context(cell, pointer, Some(interaction));
    }

    /// Start application row context menu (add/remove favorite). Start is a popup, so the
    /// menu opens on the same output with the pointer anchor of the release.
    pub fn open_start_app_context_scoped(
        &mut self,
        app: flamewm_api::DesktopAppId,
        pointer: Option<flamewm_api::Point>,
        interaction: InteractionContext,
    ) {
        self.push_context_menu(
            ContextMenuKind::StartApp(app),
            self.snapshot.panels.revision,
            pointer,
            Some(interaction),
        );
    }

    pub fn open_taskbar_context_at_scoped(
        &mut self,
        pointer: Option<flamewm_api::Point>,
        interaction: InteractionContext,
    ) {
        self.push_context_menu(
            ContextMenuKind::Taskbar,
            self.snapshot.panels.revision,
            pointer,
            Some(interaction),
        );
    }
}
