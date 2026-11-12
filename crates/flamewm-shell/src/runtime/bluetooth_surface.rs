//! Open/close transaction for the Bluetooth pairing card. Placement and focus come from
//! `card_surface`, shared with the polkit card.

use flamewm_ui_x11::SurfaceRuntime;

use super::card_surface::unmap_card;
use super::ShellSurfaces;

/// Root node of the card in `ui/shell/bluetooth.html`.
const CARD_NODE_ID: &str = "bluetooth";

impl ShellSurfaces {
    /// Places and maps the card while the agent has a prompt or notice. No-op while it is open.
    pub fn open_bluetooth(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        if self.bluetooth_open {
            return Ok(());
        }
        self.bluetooth_output = Some(self.map_card(runtime, self.bluetooth, CARD_NODE_ID)?);
        self.bluetooth_open = true;
        Ok(())
    }

    /// Hides the card and releases its surface. No-op while it is closed.
    pub fn close_bluetooth(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        if !std::mem::take(&mut self.bluetooth_open) {
            return Ok(());
        }
        self.bluetooth_output = None;
        unmap_card(runtime, self.bluetooth)
    }

    #[must_use]
    pub fn is_bluetooth_open(&self) -> bool {
        self.bluetooth_open
    }
}
