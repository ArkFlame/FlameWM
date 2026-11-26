//! Monitor actions for the X11 WM: move the active window between `RandR` outputs and move input
//! focus between outputs. Geometry policy lives in `flamewm_window_core::monitor` so the Wayland
//! backend can reuse it.

use super::*;

impl<C: Connection> Wm<'_, C> {
    /// Output the current action should start from: the active window's output, else primary.
    fn current_output_index(&self) -> Option<usize> {
        if let Some(client) = self.active
            && let Some(rect) = self.clients.get(&client).map(|state| state.outer)
            && let Some(index) = select_output_for_rect(&self.output_work_areas, rect)
        {
            return Some(index);
        }
        self.output_work_areas
            .iter()
            .position(|output| output.primary)
            .or_else(|| (!self.output_work_areas.is_empty()).then_some(0))
    }

    /// Moves the active window to the adjacent output, keeping its relative position and its
    /// workspace. Maximized and snapped clients are re-laid out in the target work area by
    /// `reconcile_work_area`, the same path a `RandR` topology change uses.
    pub(super) fn move_window_to_monitor(&mut self, step: MonitorStep) -> Result<(), ReplyError> {
        let Some(client) = self.active else {
            return Ok(());
        };
        let Some(from) = self.current_output_index() else {
            return Ok(());
        };
        let Some(to) = cycle_output_index(&self.output_work_areas, from, step) else {
            return Ok(());
        };
        let Some((outer, floating)) = self
            .clients
            .get(&client)
            .map(|state| (state.outer, state.is_floating()))
        else {
            return Ok(());
        };
        let moved = flamewm_window_core::monitor::relocate_to_work_area(
            to_core_rect(outer),
            to_core_rect(self.output_work_areas[from].work_area),
            to_core_rect(self.output_work_areas[to].work_area),
        );
        let moved = from_core_rect(moved);
        if let Some(state) = self.clients.get_mut(&client) {
            state.set_outer(moved);
            state.placement.current = RootRect::from_legacy(moved);
            if floating {
                state.placement.floating_restore = state.placement.current;
            }
        }
        self.apply_frame_geometry(client)?;
        self.layout_input_children(client)?;
        self.paint_chrome(client)?;
        self.reconcile_work_area()?;
        self.focus(client)?;
        self.mark_windows();
        Ok(())
    }

    /// Moves input focus to the adjacent output, focusing its most recently used visible window.
    /// With no such window, warps the pointer to the target work area as mango does on `focusmon`.
    pub(super) fn focus_monitor(&mut self, step: MonitorStep) -> Result<(), ReplyError> {
        let Some(from) = self.current_output_index() else {
            return Ok(());
        };
        let Some(to) = cycle_output_index(&self.output_work_areas, from, step) else {
            return Ok(());
        };
        let target = self.output_work_areas[to].work_area;
        let candidate = self.mru.order().iter().copied().find(|&window| {
            self.clients.get(&window).is_some_and(|state| {
                state.kind == WindowKind::Normal
                    && !state.minimized
                    && state.visible_on(self.current_workspace)
                    && select_output_for_rect(&self.output_work_areas, state.outer) == Some(to)
            })
        });
        if let Some(window) = candidate {
            return self.focus(window);
        }
        let x = target.x.saturating_add((target.width / 2) as i32);
        let y = target.y.saturating_add((target.height / 2) as i32);
        self.conn.warp_pointer(
            x11rb::NONE,
            self.screen().root,
            0,
            0,
            0,
            0,
            x.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
            y.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
        )?;
        Ok(())
    }
}
