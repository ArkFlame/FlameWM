//! Open/close transaction for the polkit authentication card. Shell-owned and placed like the
//! Launcher on the primary output. No pointer grab: an outside click must not dismiss a prompt the
//! agent is waiting on.

use flamewm_ui_x11::SurfaceRuntime;

use super::launcher_surface::launcher_rect;
use super::{panel_ui_scale, ui_error, ShellSurfaces};
use crate::popup_controller;

/// Root node of the card in `ui/shell/polkit.html`.
const CARD_NODE_ID: &str = "polkit";

impl ShellSurfaces {
    /// Measures, places and maps the card on the primary output. No-op while it is open.
    pub fn open_polkit(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        if self.polkit_open {
            return Ok(());
        }
        let Some(record) = self.panels.first() else {
            return Err("no panel for the polkit card".to_owned());
        };
        let work_area = record.placement.work_area;
        let scale = panel_ui_scale(record.placement.output.shell_scale_percent);
        runtime
            .apply_output_scale(self.polkit, scale)
            .map_err(ui_error)?;
        let size = popup_controller::measure_node(runtime, self.polkit, CARD_NODE_ID, work_area)
            .map_err(|error| error.to_string())?;
        let rect = launcher_rect(work_area, size);
        let width = u32::try_from(rect.width).map_err(|_| "polkit width".to_owned())?;
        let height = u32::try_from(rect.height).map_err(|_| "polkit height".to_owned())?;
        runtime
            .move_resize(self.polkit, rect.x, rect.y, width, height)
            .map_err(ui_error)?;
        runtime.show(self.polkit).map_err(ui_error)?;
        self.polkit_open = true;
        Ok(())
    }

    /// Hides the card and releases its surface. No-op while it is closed.
    pub fn close_polkit(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        if !std::mem::take(&mut self.polkit_open) {
            return Ok(());
        }
        runtime.close_surface(self.polkit).map_err(ui_error)
    }

    #[must_use]
    pub fn is_polkit_open(&self) -> bool {
        self.polkit_open
    }
}
