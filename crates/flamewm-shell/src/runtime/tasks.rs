use super::*;

impl ShellRuntime {
    /// Reorder request moving the task entry at `from` to final index `to`.
    /// Slot numbers are local to the selected panel output.
    pub(super) fn task_visual_states(&self) -> Vec<flamewm_shell_core::TaskVisualState> {
        let Some(output) = self.shown_panel_output() else {
            return Vec::new();
        };
        self.task_states_on(output)
    }

    /// Task slots of one output on the active workspace.
    pub(super) fn task_states_on(
        &self,
        output: &flamewm_api::OutputId,
    ) -> Vec<flamewm_shell_core::TaskVisualState> {
        flamewm_shell_core::task_visual_states_on_output_and_workspace(
            &self.snapshot.panels,
            &self.snapshot.windows,
            &self.snapshot.applications,
            Some(output),
            self.snapshot
                .workspaces
                .as_ref()
                .map(|workspaces| workspaces.active_index),
        )
    }

    #[must_use]
    pub fn task_slot_count(&self) -> usize {
        self.task_visual_states().len()
    }

    /// `from`/`to` are task-slot indices on the shown output; the request carries the
    /// full task-list index of the entry now occupying `to`.
    #[must_use]
    pub fn task_reorder_request(&self, from: usize, to: usize) -> Option<ControlRequest> {
        self.shown_panel_output()?;
        let states = self.task_visual_states();
        let entry_id = states.get(from)?.id.clone();
        let target = &states.get(to)?.id;
        let index = self
            .snapshot
            .panels
            .tasks
            .iter()
            .position(|entry| &entry.id == target)?;
        Some(ControlRequest::ReorderTask {
            entry_id,
            index,
            expected_revision: self.snapshot.panels.revision,
        })
    }

    /// Reorder from a press-captured task identity, rejecting a stale drag
    /// and resolving the destination slot against the current output list.
    #[must_use]
    pub fn task_reorder_request_for_target(
        &self,
        source: &TaskSlotTarget,
        to_slot: usize,
    ) -> Option<ControlRequest> {
        self.current_task_for_target(source)?;
        let states = self.task_states_on(&source.output);
        let destination = &states.get(to_slot)?.id;
        let index = self
            .snapshot
            .panels
            .tasks
            .iter()
            .position(|entry| &entry.id == destination)?;
        Some(ControlRequest::ReorderTask {
            entry_id: source.id.clone(),
            index,
            expected_revision: self.snapshot.panels.revision,
        })
    }

    /// Output of the panel whose task slots are shown; `None` on a single-panel desktop.
    pub(super) fn shown_panel_output(&self) -> Option<&flamewm_api::OutputId> {
        self.selected_panel_output.as_ref().filter(|selected| {
            self.snapshot
                .panels
                .panels
                .iter()
                .any(|panel| &panel.output == *selected)
        })
    }

    #[must_use]
    pub fn task_click_requests_for_slot(&self, slot: usize) -> Option<Vec<ControlRequest>> {
        let target = self.task_target_for_slot(slot)?;
        self.task_click_requests_for_target(&target)
    }

    /// Capture the task identity at input press time, including its owning
    /// output and exact window generation.
    #[must_use]
    pub fn task_target_for_slot(&self, slot: usize) -> Option<TaskSlotTarget> {
        let output = self.shown_panel_output()?.clone();
        self.task_target_for_slot_on_output(slot, &output)
    }

    /// Capture a task target using the output resolved from the event's
    /// source surface. Pointer routing must not infer an output from ambient
    /// selection or fall back to the first panel.
    #[must_use]
    pub fn task_target_for_slot_on_output(
        &self,
        slot: usize,
        output: &flamewm_api::OutputId,
    ) -> Option<TaskSlotTarget> {
        if !self
            .snapshot
            .panels
            .panels
            .iter()
            .any(|panel| &panel.output == output)
        {
            return None;
        }
        let states = self.task_states_on(output);
        let state = states.get(slot)?;
        Some(TaskSlotTarget {
            id: state.id.clone(),
            app_id: state.app_id.clone(),
            window: state.window,
            output: output.clone(),
        })
    }

    /// Resolve a press-captured target only while the same task identity and
    /// window generation still occupy that output. A refreshed slot cannot
    /// redirect a delayed release to another application.
    #[must_use]
    pub fn task_click_requests_for_target(
        &self,
        target: &TaskSlotTarget,
    ) -> Option<Vec<ControlRequest>> {
        self.current_task_for_target(target)?;
        let entry = self
            .snapshot
            .panels
            .tasks
            .iter()
            .find(|entry| entry.id == target.id)?;
        // A pinned launcher whose live window belongs to another workspace
        // must not activate that hidden window when clicked here.
        if target.window.is_none() {
            return Some(vec![ControlRequest::LaunchApplication {
                app: target.app_id.clone(),
                options: ApplicationLaunchOptions::default(),
            }]);
        }
        Some(
            match flamewm_shell_core::task_click_action(entry, &self.snapshot.windows) {
                flamewm_shell_core::TaskClickAction::Launch(app) => {
                    vec![ControlRequest::LaunchApplication {
                        app,
                        options: ApplicationLaunchOptions::default(),
                    }]
                }
                // Restore maps the frame and focuses it, and bumps the window generation: a second
                // ActivateWindow carrying the pre-restore reference would fail with StaleRevision.
                flamewm_shell_core::TaskClickAction::RestoreAndActivate(window) => {
                    flamewm_debug::emit(
                        flamewm_debug::DebugEventId("restore.trace.shell.send"),
                        std::time::Duration::ZERO,
                        || {
                            let ms = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map_or(0, |d| d.as_millis());
                            format!("t_ms={ms} window={window:?} request=RestoreWindow")
                        },
                    );
                    vec![ControlRequest::RestoreWindow(window)]
                }
                flamewm_shell_core::TaskClickAction::Minimize(window) => {
                    vec![ControlRequest::MinimizeWindow(window)]
                }
                flamewm_shell_core::TaskClickAction::Activate(window) => {
                    vec![ControlRequest::ActivateWindow(window)]
                }
            },
        )
    }

    pub(super) fn current_task_for_target(
        &self,
        target: &TaskSlotTarget,
    ) -> Option<&flamewm_api::panels::TaskEntry> {
        if !self
            .snapshot
            .panels
            .panels
            .iter()
            .any(|panel| panel.output == target.output)
        {
            return None;
        }
        let states = self.task_states_on(&target.output);
        let state = states.iter().find(|state| state.id == target.id)?;
        if state.app_id != target.app_id || state.window != target.window {
            return None;
        }
        self.snapshot
            .panels
            .tasks
            .iter()
            .find(|entry| entry.id == target.id)
    }
}
