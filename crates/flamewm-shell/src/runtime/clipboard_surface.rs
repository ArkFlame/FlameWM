//! Open/close transaction for the clipboard history popup. Placement and map/grab are the
//! Launcher's top-centre transaction (`launcher_surface::open_top_centre`); this module owns
//! only the clipboard surface's own open state.

use flamewm_api::{OutputId, Rect};
use flamewm_ui_x11::SurfaceRuntime;

use super::{ui_error, ShellSurfaces};
use crate::popup_role::PopupRole;

impl ShellSurfaces {
    /// Places, maps and grabs the clipboard popup on `output`.
    pub fn open_clipboard_on(
        &mut self,
        runtime: &mut SurfaceRuntime,
        output: &OutputId,
    ) -> Result<Rect, String> {
        let rect = self.open_top_centre(runtime, self.clipboard, PopupRole::Clipboard, output)?;
        self.clipboard_output = Some(output.clone());
        Ok(rect)
    }

    /// Hides the clipboard popup. A no-op when it is closed.
    pub fn close_clipboard(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        if self.clipboard_output.take().is_none() {
            return Ok(());
        }
        runtime.close_surface(self.clipboard).map_err(ui_error)
    }

    #[must_use]
    pub fn is_clipboard_open(&self) -> bool {
        self.clipboard_output.is_some()
    }
}
