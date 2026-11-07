//! Menu, popup and surface background colours and corner radius.

use super::*;

/// Shared surface background for the audio, Wi-Fi and calendar popups: the `surfaceColor` /
/// `surfaceOpacity` pair on the popup root. Layers inside stay transparent.
pub fn project_menu_background(
    document: &mut impl UiDocumentAccess,
    snapshot: &SettingsSnapshot,
    root: &str,
) -> Result<(), String> {
    document.background(root, surface_background(snapshot))?;
    project_surface_outline(document, root, snapshot)?;
    document.corner_radius(root, popup_corner_radius(snapshot))?;
    // Popups own their accent too: the `--accent` token drives the battery bar, and the
    // Wi-Fi/audio track override reads the global task accent, which must match this snapshot.
    crate::projection::set_task_accent(accent_color(snapshot));
    project_accent_tokens(document, snapshot)
}

/// Popup and menu corner radius from the `decoration` setting: Rounded 12, Squared 0.
#[must_use]
pub fn popup_corner_radius(snapshot: &SettingsSnapshot) -> f32 {
    let rounded = text_setting(snapshot, "decoration", "rounded") != "squared";
    shape_radius(rounded) as f32
}

/// Start menu background colour: the shared surface pair, same as [`surface_background`].
#[must_use]
pub fn menu_background(snapshot: &SettingsSnapshot) -> UiColor {
    surface_background(snapshot)
}

/// Shared surface colour with the `surfaceOpacity` alpha (taskbar, popups, desktop menus).
/// The mapping is owned by `flamewm_ui_core::context_menu::surface_fill`.
#[must_use]
pub fn surface_background(snapshot: &SettingsSnapshot) -> UiColor {
    shared_surface_background(snapshot)
}
