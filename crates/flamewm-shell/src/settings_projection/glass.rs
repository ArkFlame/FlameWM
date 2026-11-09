//! Glass and backdrop material percent projection for panels and Start.

use super::*;

/// Surface `(opacity, blur)` percents for the taskbar pseudo-transparency backdrop.
#[must_use]
pub fn taskbar_backdrop_percents(snapshot: &SettingsSnapshot) -> (u8, u8) {
    flamewm_ui_core::surface::dock_backdrop_percents(snapshot)
}

/// Dock pseudo-transparency `(opacity, blur)`: the Dock material while `dock.surfaceCustom` is off,
/// otherwise the Dock's own `surfaceOpacity` / `surfaceBlurPercent` overrides.
#[must_use]
pub fn dock_backdrop_percents(snapshot: &SettingsSnapshot) -> (u8, u8) {
    if flamewm_api::settings_groups::dock_surface_custom(snapshot) {
        start_backdrop_percents(&flamewm_api::settings_groups::dock_effective_snapshot(
            snapshot,
        ))
    } else {
        taskbar_backdrop_percents(snapshot)
    }
}

/// Resolved taskbar/Dock material for the current appearance settings. The Dock role never
/// reads the content glass tint, the contrast zone or `surfaceOpacity`.
#[must_use]
pub fn dock_material(snapshot: &SettingsSnapshot) -> flamewm_ui_core::glass::ResolvedMaterial {
    flamewm_ui_core::glass::resolve(
        flamewm_ui_core::glass::MaterialRole::Dock,
        glass_appearance(snapshot),
        100,
        flamewm_ui_core::glass::RendererCaps {
            backdrop_blur: true,
        },
    )
}

/// Maps the persisted glass settings to the resolver input. Absent or invalid keys use defaults.
#[must_use]
pub fn glass_appearance(snapshot: &SettingsSnapshot) -> flamewm_ui_core::glass::Appearance {
    flamewm_ui_core::glass::appearance_from_settings(snapshot)
}

/// Blur px at 1x to the `surfaceBlurPercent` scale (`px / 40 * 100`), capped at 100.
pub(super) fn blur_px_to_percent(blur_px: u16) -> u8 {
    let percent = (u32::from(blur_px) * 100 + 20) / 40;
    u8::try_from(percent.min(100)).unwrap_or(100)
}

/// Start popup `(opacity, blur)` percents for its pseudo-transparency backdrop: the shared
/// `surfaceOpacity` / `surfaceBlurPercent` pair.
#[must_use]
pub fn start_backdrop_percents(snapshot: &SettingsSnapshot) -> (u8, u8) {
    (
        percent_setting(snapshot, "surfaceOpacity", DEFAULT_SURFACE_OPACITY),
        percent_setting(snapshot, "surfaceBlurPercent", DEFAULT_SURFACE_BLUR_PERCENT),
    )
}
