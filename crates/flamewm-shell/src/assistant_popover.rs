//! Taskbar assistant button. A click launches `flamewm-assistant --popover X Y W H`, where
//! `X Y W H` is the button's device rect. The assistant owns the popover window (override-redirect,
//! no titlebar); a second click on the button arrives as a launch and the assistant's toggle logic
//! stops the open popover, the same way Super+C does.

use crate::popup_controller::resolve_source_rect;
use flamewm_ui_x11::{SurfaceHandle, SurfaceRuntime};

/// Panel action id of the taskbar assistant button (`ui/shell/panel.html`).
pub const ACTION: &str = "assistant.open";
/// Panel node id of the taskbar assistant button; the popover is anchored to its rect.
pub const NODE_ID: &str = "tray-assistant";
/// Program launched for the popover; the same binary serves Super+C and Settings.
pub const PROGRAM: &str = "flamewm-assistant";

/// Arguments for `flamewm-assistant --popover`, from the button rect.
#[must_use]
pub fn launch_args(anchor: flamewm_api::Rect) -> [String; 5] {
    [
        "--popover".to_owned(),
        anchor.x.to_string(),
        anchor.y.to_string(),
        anchor.width.to_string(),
        anchor.height.to_string(),
    ]
}

/// Resolves the button rect on the panel and spawns the popover next to it.
pub fn launch(runtime: &SurfaceRuntime, panel: SurfaceHandle) -> Result<(), String> {
    let anchor = resolve_source_rect(runtime, panel, NODE_ID).map_err(|error| error.to_string())?;
    std::process::Command::new(PROGRAM)
        .args(launch_args(anchor))
        .spawn()
        .map(drop)
        .map_err(|error| format!("could not open the assistant: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_args_carry_the_button_rect_in_order() {
        let anchor = flamewm_api::Rect::new(1200, 1040, 30, 40);
        assert_eq!(
            launch_args(anchor),
            ["--popover", "1200", "1040", "30", "40"].map(str::to_owned)
        );
    }
}
