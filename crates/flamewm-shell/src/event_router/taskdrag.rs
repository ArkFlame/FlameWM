use super::*;

impl ShellLoop {
    /// Task-slot drag gesture. Returns true when the event is consumed; a
    /// short press returns false so the normal release click still runs.
    pub(super) fn route_task_drag(
        &mut self,
        event: &SurfaceEvent,
        runtime: &mut SurfaceRuntime,
        event_output: &flamewm_api::OutputId,
    ) -> bool {
        use flamewm_shell::taskbar::tasks::drag::{along_strip, TaskDrag, TaskDragRelease};
        let action = event.action.action.as_str();
        if self.task_drag.is_some()
            && action == "keyboard.input"
            && event.action.text.as_deref() == Some("\u{1b}")
        {
            self.cancel_task_drag(runtime);
            return true;
        }
        if self.task_drag.is_some()
            && !task_drag_stays_on_origin(
                self.task_press_target.as_ref().map(|(_, target)| target),
                event_output,
            )
        {
            self.cancel_task_drag(runtime);
            return true;
        }
        let slot = slot_of(action, "task.slot.");
        match event.action.phase {
            ActionPhase::Press => {
                let primary = matches!(
                    PointerButton::from_raw_x(event.action.button),
                    PointerButton::Primary
                );
                self.task_drag = slot
                    .filter(|slot| {
                        primary
                            && task_drag_can_start(
                                self.task_press_target.as_ref(),
                                *slot,
                                event_output,
                            )
                    })
                    .map(|slot| {
                        let vertical = self.task_strip_is_vertical(event_output);
                        TaskDrag::press_on_axis(
                            slot,
                            along_strip(vertical, event.action.x, event.action.y),
                            vertical,
                        )
                    });
                false
            }
            ActionPhase::Motion => {
                let count = self.shell.task_slot_count();
                let Some(drag) = self.task_drag.as_mut() else {
                    return false;
                };
                let vertical = drag.is_vertical();
                let pos = along_strip(vertical, event.action.x, event.action.y);
                if drag.motion(pos) {
                    let target = drag.target(pos, count);
                    self.set_task_insert_marker(runtime, event_output, Some(target), vertical);
                }
                true
            }
            ActionPhase::Release => {
                let Some(drag) = self.task_drag.take() else {
                    return false;
                };
                self.set_task_insert_marker(runtime, event_output, None, drag.is_vertical());
                let count = self.shell.task_slot_count();
                match drag.release_on_strip(event.action.x, event.action.y, count) {
                    TaskDragRelease::Click => false,
                    TaskDragRelease::Noop => {
                        self.task_press_target = None;
                        true
                    }
                    TaskDragRelease::Reorder { to, .. } => {
                        let target = self.task_press_target.take().map(|(_, target)| target);
                        if let Some(request) = target
                            .as_ref()
                            .and_then(|target| self.reorder_task_target(target, to))
                        {
                            self.submit(request);
                        }
                        true
                    }
                }
            }
            ActionPhase::Hover => false,
        }
    }

    pub(super) fn route_start_app_drag(
        &mut self,
        event: &SurfaceEvent,
        runtime: &mut SurfaceRuntime,
    ) -> Result<bool, UiBackendError> {
        let action = event.action.action.as_str();
        match event.action.phase {
            ActionPhase::Press => {
                let primary = matches!(
                    PointerButton::from_raw_x(event.action.button),
                    PointerButton::Primary
                );
                self.start_drag = slot_of(action, "start.app.")
                    .filter(|_| primary && self.start_open)
                    .and_then(|slot| {
                        crate::entry::projection::start_application_for_slot(
                            &self.start_model,
                            self.start_category,
                            &self.start_query,
                            self.start_index_for_slot(slot),
                        )
                    })
                    .map(|app_id| StartAppDrag::press(app_id, event.action.x, event.action.y));
                Ok(false)
            }
            ActionPhase::Motion => {
                let Some(drag) = self.start_drag.as_mut() else {
                    return Ok(false);
                };
                Ok(drag.motion(event.action.x, event.action.y))
            }
            ActionPhase::Release => {
                let Some(drag) = self.start_drag.take() else {
                    return Ok(false);
                };
                if !drag.dragging {
                    return Ok(false);
                }
                if let Some(point) = root_pointer_for_surface(runtime, event) {
                    let surface_rect = runtime.surface_device_rect(event.surface).ok();
                    let outside_start = surface_rect.is_none_or(|rect| {
                        !flamewm_api::Rect::new(
                            rect.x.round() as i32,
                            rect.y.round() as i32,
                            rect.width.round() as i32,
                            rect.height.round() as i32,
                        )
                        .contains(point)
                    });
                    if outside_start && self.is_desktop_drop_target(point) {
                        // Off the event loop: catalog discovery and the copy touch the filesystem.
                        if let Err(error) = spawn_app_shortcut(drag.app_id.clone()) {
                            eprintln!(
                                "flamewm-shell: create desktop shortcut for {}: {error}",
                                drag.app_id.as_str()
                            );
                        }
                    }
                }
                self.close_transients(runtime)?;
                Ok(true)
            }
            ActionPhase::Hover => Ok(false),
        }
    }

    pub(super) fn is_desktop_drop_target(&self, point: flamewm_api::Point) -> bool {
        let snapshot = self.shell.snapshot();
        if !snapshot
            .displays
            .outputs
            .iter()
            .any(|output| output.connected && output.geometry.contains(point))
            || snapshot
                .panels
                .panels
                .iter()
                .any(|panel| panel.visible && panel.geometry.contains(point))
        {
            return false;
        }
        let active_workspace = snapshot
            .workspaces
            .as_ref()
            .map(|workspaces| workspaces.active_index);
        !snapshot.windows.iter().any(|window| {
            window.is_visible()
                && (window.sticky
                    || active_workspace.is_none_or(|active| {
                        usize::try_from(window.workspace.index).ok() == Some(active)
                    }))
                && window.outer_geometry.contains(point)
        })
    }

    /// Show the 2 px accent insertion marker before slot `target`, or hide it
    /// (`None`). Only the marker node is written, so the panel repaints once.
    pub(super) fn set_task_insert_marker(
        &mut self,
        runtime: &mut SurfaceRuntime,
        output: &flamewm_api::OutputId,
        target: Option<usize>,
        vertical: bool,
    ) {
        if let Some(panel) = self.surfaces.panel_for_output(output) {
            self.set_task_insert_marker_on_panel(runtime, panel, target, vertical);
        }
    }

    /// True when the taskbar on `output` runs down a Left/Right edge.
    pub(super) fn task_strip_is_vertical(&self, output: &flamewm_api::OutputId) -> bool {
        self.shell
            .snapshot()
            .panels
            .panels
            .iter()
            .any(|panel| &panel.output == output && !panel.edge.is_horizontal())
    }

    pub(super) fn cancel_task_drag(&mut self, runtime: &mut SurfaceRuntime) {
        let panel = self
            .task_press_target
            .as_ref()
            .and_then(|(_, target)| task_drag_cleanup_output(Some(target)))
            .and_then(|output| self.surfaces.panel_for_output(output));
        self.task_drag = None;
        self.task_press_target = None;
        if let Some(panel) = panel {
            self.set_task_insert_marker_on_panel(runtime, panel, None, false);
        }
    }

    pub(super) fn set_task_insert_marker_on_panel(
        &mut self,
        runtime: &mut SurfaceRuntime,
        panel: flamewm_ui_x11::SurfaceHandle,
        target: Option<usize>,
        vertical: bool,
    ) {
        use flamewm_shell::taskbar::tasks::drag::insertion_marker_left;
        let result = runtime.with_document(panel, |document| match target {
            Some(index) if vertical => {
                document.position("task-slot-insert", 6.0, insertion_marker_left(index))?;
                document.size("task-slot-insert", 32.0, 2.0)
            }
            Some(index) => {
                document.position("task-slot-insert", insertion_marker_left(index), 6.0)?;
                document.size("task-slot-insert", 2.0, 32.0)
            }
            None => document.size("task-slot-insert", 0.0, 0.0),
        });
        if let Err(error) = result {
            eprintln!("flamewm-shell: task insertion marker projection failed: {error:?}");
        }
    }
}
