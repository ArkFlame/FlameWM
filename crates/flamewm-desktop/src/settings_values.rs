//! Typed readers over the settings snapshot and the accent-token projection for the desktop.

use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_ui_x11::{UiColor, UiDocumentAccess};

pub(crate) fn setting_text(snapshot: &SettingsSnapshot, key: &str) -> String {
    match snapshot.values.get(key) {
        Some(SettingValue::Text(value)) => value.clone(),
        _ => String::new(),
    }
}

pub(crate) fn setting_bool(snapshot: &SettingsSnapshot, key: &str, default: bool) -> bool {
    match snapshot.values.get(key) {
        Some(SettingValue::Boolean(value)) => *value,
        _ => default,
    }
}

/// True when the snapshot's `iconTheme` differs from the last applied value.
pub(crate) fn icon_theme_changed(previous: &str, snapshot: &SettingsSnapshot) -> bool {
    setting_text(snapshot, "iconTheme") != previous
}

pub(crate) fn setting_percent(snapshot: &SettingsSnapshot, key: &str, default: u8) -> u8 {
    match snapshot.values.get(key) {
        Some(SettingValue::Percent(value)) => *value,
        _ => default,
    }
}

fn setting_integer(snapshot: &SettingsSnapshot, key: &str, default: i64) -> i64 {
    match snapshot.values.get(key) {
        Some(SettingValue::Integer(value)) => *value,
        _ => default,
    }
}

pub(crate) fn desktop_entry_scale(snapshot: &SettingsSnapshot) -> u16 {
    setting_integer(snapshot, "DesktopEntryScalePercent", 100).clamp(50, 200) as u16
}

pub(crate) fn percent_alpha(percent: u8) -> u8 {
    ((u16::from(percent) * 255) / 100) as u8
}

/// Neutral grey until the settings snapshot supplies `accent` (mirrors the shell default).
const NEUTRAL_ACCENT: u32 = 0x007f_878e;
/// Accent-derived tokens of `ui/shell/flamewm.css`, with the alpha each keeps in its `:root`.
pub(crate) const ACCENT_TOKEN_ALPHAS: [(&str, u8); 9] = [
    ("--accent", 0xff),
    ("--accent-soft", 0x38),
    ("--accent-strong", 0xdd),
    ("--desktop-hover", 0x26),
    ("--desktop-hover-border", 0x4d),
    ("--desktop-selected", 0x47),
    ("--desktop-selected-border", 0xa6),
    ("--selection-fill", 0x33),
    ("--snap-fill", 0x33),
];

pub(crate) fn accent_color(snapshot: &SettingsSnapshot) -> UiColor {
    let hex = match snapshot.values.get("accent") {
        Some(SettingValue::Text(value)) => value.strip_prefix('#').unwrap_or(value),
        _ => "",
    };
    let rgb = if hex.len() == 6 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        u32::from_str_radix(hex, 16).unwrap_or(NEUTRAL_ACCENT)
    } else {
        NEUTRAL_ACCENT
    };
    UiColor {
        r: (rgb >> 16) as u8,
        g: (rgb >> 8) as u8,
        b: rgb as u8,
        a: 255,
    }
}

/// Accent-derived token values for the desktop; `accentOpacity` scales them like the shell does.
pub(crate) fn accent_token_values(snapshot: &SettingsSnapshot) -> Vec<(&'static str, UiColor)> {
    let accent = accent_color(snapshot);
    let opacity = percent_alpha(setting_percent(snapshot, "accentOpacity", 100));
    ACCENT_TOKEN_ALPHAS
        .into_iter()
        .map(|(name, alpha)| {
            let scaled = ((u16::from(alpha) * u16::from(opacity)) / 255) as u8;
            (
                name,
                UiColor {
                    a: scaled,
                    ..accent
                },
            )
        })
        .collect()
}

pub(crate) fn project_accent_tokens(
    document: &mut impl UiDocumentAccess,
    snapshot: &SettingsSnapshot,
) -> Result<(), String> {
    for (name, color) in accent_token_values(snapshot) {
        document.color_variable(name, color)?;
    }
    Ok(())
}
