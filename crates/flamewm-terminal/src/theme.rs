//! Maps the active terminal theme onto FlameWM's live settings snapshot. In `follow-flamewm` mode the background is the
//! FlameWM surface colour and the cursor and selection use the accent. Translucency comes from the terminal's own
//! `background_opacity` setting, with FlameWM's wallpaper and surface blur.
use std::path::Path;

use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_ui_core::context_menu::surface_fill;
use flamewm_ui_x11::backdrop::WindowBackdrop;

use crate::store::FOLLOW_FLAMEWM;
use crate::themes::{Palette, Theme, builtin_themes};

/// FlameWM default accent, the same value as flamewm-settings `DEFAULT_ACCENT`.
pub const DEFAULT_ACCENT: u32 = 0xEF4048;
/// Shared default for the surface blur (shell/desktop `surfaceBlurPercent`).
pub const DEFAULT_BLUR_PERCENT: u8 = 80;

fn text_setting<'a>(snapshot: &'a SettingsSnapshot, key: &str) -> Option<&'a str> {
    match snapshot.values.get(key) {
        Some(SettingValue::Text(value)) => Some(value.as_str()),
        _ => None,
    }
}

/// Alpha for the background: the opacity percent scaled to 0..=255.
pub fn surface_alpha(opacity_percent: u8) -> u8 {
    ((u16::from(opacity_percent.min(100)) * 255) / 100) as u8
}

fn accent(snapshot: &SettingsSnapshot) -> u32 {
    let Some(name) = text_setting(snapshot, "accent") else {
        return DEFAULT_ACCENT;
    };
    match name {
        "red" => DEFAULT_ACCENT,
        "blue" => 0x3dae_e9,
        "purple" => 0x9b59b6,
        "teal" => 0x1abc9c,
        "green" => 0x27ae60,
        "amber" => 0xf39c12,
        other => crate::themes::parse_hex(other).unwrap_or(DEFAULT_ACCENT),
    }
}

/// Palette for `follow-flamewm`: surface background, accent cursor, accent-tinted selection, and the Dark+ ANSI set.
pub fn follow_palette(snapshot: &SettingsSnapshot) -> Palette {
    let base = builtin_themes()[0].palette.clone();
    let surface = surface_fill(snapshot).rgb;
    let accent = accent(snapshot);
    let blend = |a: u32, b: u32, weight: u32| {
        let mix = |shift: u32| {
            (((a >> shift) & 0xff) * weight + ((b >> shift) & 0xff) * (100 - weight)) / 100
        };
        (mix(16) << 16) | (mix(8) << 8) | mix(0)
    };
    Palette {
        background: surface,
        foreground: 0xf1_f2_f3,
        cursor: accent,
        selection: blend(accent, surface, 40),
        ansi: base.ansi,
    }
}

/// Resolves the active theme name to a palette. `follow-flamewm` (or an unknown name) uses FlameWM colours or Dark+.
pub fn resolve_palette(
    active: &str,
    themes: &[Theme],
    snapshot: Option<&SettingsSnapshot>,
) -> Palette {
    if active == FOLLOW_FLAMEWM {
        return snapshot.map_or_else(|| builtin_themes()[0].palette.clone(), follow_palette);
    }
    themes
        .iter()
        .find(|theme| theme.name == active)
        .map(|theme| theme.palette.clone())
        .unwrap_or_else(|| builtin_themes()[0].palette.clone())
}

/// Backdrop for the window: shared wallpaper settings with the terminal opacity and blur. `None` at 100% opacity.
pub fn window_backdrop(
    snapshot: &SettingsSnapshot,
    packaged_root: &Path,
    opacity: u8,
) -> Option<WindowBackdrop> {
    if opacity >= 100 {
        return None;
    }
    let fit = text_setting(snapshot, "wallpaperFit")
        .unwrap_or("")
        .trim()
        .to_owned();
    let blur = match snapshot.values.get("surfaceBlurPercent") {
        Some(SettingValue::Percent(percent)) => (*percent).min(100),
        _ => DEFAULT_BLUR_PERCENT,
    };
    Some(WindowBackdrop {
        wallpaper: text_setting(snapshot, "wallpaper").unwrap_or("").to_owned(),
        fit: if fit.is_empty() {
            "fill".to_owned()
        } else {
            fit
        },
        packaged_root: packaged_root.to_path_buf(),
        opacity_percent: opacity.min(100),
        blur_percent: blur,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opacity_percent_scales_to_alpha() {
        assert_eq!(surface_alpha(100), 255);
        assert_eq!(surface_alpha(80), 204);
        let snapshot = SettingsSnapshot::new(1);
        assert!(
            window_backdrop(&snapshot, Path::new("/x"), 100).is_none(),
            "opaque surfaces install no backdrop"
        );
        assert_eq!(
            window_backdrop(&snapshot, Path::new("/x"), 80).map(|spec| spec.opacity_percent),
            Some(80)
        );
    }

    #[test]
    fn follow_mode_takes_surface_and_accent_from_flamewm() {
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot
            .values
            .insert("surfaceColor".into(), SettingValue::Text("#102030".into()));
        snapshot
            .values
            .insert("accent".into(), SettingValue::Text("#3DAEE9".into()));
        let palette = resolve_palette(FOLLOW_FLAMEWM, &[], Some(&snapshot));
        assert_eq!(palette.background, surface_fill(&snapshot).rgb);
        assert_eq!(palette.cursor, 0x3dae_e9);
    }

    #[test]
    fn named_theme_resolves_and_unknown_falls_back_to_dark_plus() {
        let themes = builtin_themes();
        assert_eq!(resolve_palette("Nord", &themes, None), themes[7].palette);
        assert_eq!(resolve_palette("Missing", &themes, None), themes[0].palette);
    }
}
