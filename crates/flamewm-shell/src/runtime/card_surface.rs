//! Placement shared by the authentication and pairing cards. Shell-owned and placed like the
//! Launcher on the primary output, at a fixed 420 logical px width and its content height. No
//! pointer grab: an outside click must not dismiss a prompt an agent is waiting on.

use flamewm_api::OutputId;
use flamewm_ui_x11::{SurfaceHandle, SurfaceRuntime};

use super::launcher_surface::launcher_rect;
use super::{panel_ui_scale, ui_error, ShellSurfaces};
use crate::popup_controller;

/// Logical width of a card, the fixed width of a prompt dialog (KDE polkit-kde-agent precedent).
const CARD_WIDTH: f32 = 420.0;

impl ShellSurfaces {
    /// Measures `surface` at its root `node_id`, maps it on the primary output and takes X focus.
    /// Returns the output it opened on. The caller tracks its own open flag.
    pub(super) fn map_card(
        &self,
        runtime: &mut SurfaceRuntime,
        surface: SurfaceHandle,
        node_id: &str,
    ) -> Result<OutputId, String> {
        let Some(record) = self.panels.first() else {
            return Err(format!("no panel for the {node_id} card"));
        };
        let work_area = record.placement.work_area;
        let output = record.placement.output.id.clone();
        let scale = panel_ui_scale(record.placement.output.shell_scale_percent);
        runtime
            .apply_output_scale(surface, scale)
            .map_err(ui_error)?;
        let mut card_area = work_area;
        card_area.width = card_area.width.min((CARD_WIDTH * scale).round() as i32);
        let size = popup_controller::measure_node(runtime, surface, node_id, card_area)
            .map_err(|error| error.to_string())?;
        let rect = launcher_rect(work_area, size);
        let width = u32::try_from(rect.width).map_err(|_| format!("{node_id} width"))?;
        let height = u32::try_from(rect.height).map_err(|_| format!("{node_id} height"))?;
        runtime
            .move_resize(surface, rect.x, rect.y, width, height)
            .map_err(ui_error)?;
        runtime.show(surface).map_err(ui_error)?;
        // No pointer grab, so the WM never focuses this override-redirect card. Typed input needs
        // X focus, handed over the way a grabbed popup gets it.
        runtime.take_keyboard_focus(surface).map_err(ui_error)?;
        Ok(output)
    }
}

/// Releases focus and closes a card surface.
pub(super) fn unmap_card(
    runtime: &mut SurfaceRuntime,
    surface: SurfaceHandle,
) -> Result<(), String> {
    runtime.release_keyboard_focus(surface).map_err(ui_error)?;
    runtime.close_surface(surface).map_err(ui_error)
}
