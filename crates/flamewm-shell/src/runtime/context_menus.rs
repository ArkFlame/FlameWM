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

    /// Taskbar-background context menu, anchored at the root-coordinate click.
    pub fn open_taskbar_context_at(&mut self, pointer: Option<flamewm_api::Point>) {
        self.push_context_menu(
            ContextMenuKind::Taskbar,
            self.snapshot.panels.revision,
            pointer,
            None,
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
