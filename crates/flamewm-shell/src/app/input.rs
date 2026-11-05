//! Pointer and task-slot input: pointer ownership, task clicks, icon-target capture and reorder.

use super::*;

impl ShellLoop {
    pub(crate) fn needs_output_pointer(&self) -> bool {
        self.shell.has_pending_controls()
    }

    pub(crate) fn set_output_pointer(&mut self, pointer: Option<(i32, i32)>) {
        self.pointer_position = pointer;
    }

    pub(crate) fn click_task_slot(
        &mut self,
        _runtime: &mut SurfaceRuntime,
        slot: usize,
    ) -> Result<(), UiBackendError> {
        let target = self
            .task_press_target
            .take()
            .filter(|(pressed_slot, _)| *pressed_slot == slot)
            .map(|(_, target)| target);
        let Some(target) = target else {
            return Ok(());
        };
        let Some(requests) = self.shell.task_click_requests_for_target(&target) else {
            return Ok(());
        };
        for request in requests {
            self.submit(request);
        }
        Ok(())
    }

    pub(crate) fn capture_task_target_for_output(
        &mut self,
        slot: usize,
        output: &flamewm_api::OutputId,
    ) {
        self.task_press_target = self
            .shell
            .task_target_for_slot_on_output(slot, output)
            .map(|target| (slot, target));
    }

    pub(crate) fn reorder_task_target(
        &mut self,
        target: &flamewm_shell::runtime::TaskSlotTarget,
        to: usize,
    ) -> Option<ControlRequest> {
        self.shell.task_reorder_request_for_target(target, to)
    }
}
