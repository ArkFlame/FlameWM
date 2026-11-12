//! Open/close transaction for the polkit authentication card. Placement and focus come from
//! `card_surface`; the card has no pointer grab, so an outside click cannot dismiss a prompt.

use flamewm_ui_x11::SurfaceRuntime;

use super::card_surface::unmap_card;
use super::ShellSurfaces;

/// Root node of the card in `ui/shell/polkit.html`.
const CARD_NODE_ID: &str = "polkit";

impl ShellSurfaces {
    /// Measures, places and maps the card on the primary output. No-op while it is open.
    pub fn open_polkit(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        if self.polkit_open {
            return Ok(());
        }
        self.polkit_output = Some(self.map_card(runtime, self.polkit, CARD_NODE_ID)?);
        self.polkit_open = true;
        Ok(())
    }

    /// Hides the card and releases its surface. No-op while it is closed.
    pub fn close_polkit(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        if !std::mem::take(&mut self.polkit_open) {
            return Ok(());
        }
        self.polkit_output = None;
        unmap_card(runtime, self.polkit)
    }

    #[must_use]
    pub fn is_polkit_open(&self) -> bool {
        self.polkit_open
    }
}
