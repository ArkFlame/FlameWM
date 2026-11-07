//! Typed settings readers with defaults for shell projections.

use super::*;

/// Flame-owned icon theme (`iconTheme`); unset resolves to the product default.
#[must_use]
pub fn icon_theme_name(snapshot: Option<&SettingsSnapshot>) -> String {
    let default = flamewm_platform::settings::ProductSettings::default().icon_theme;
    snapshot.map_or_else(
        || default.clone(),
        |snapshot| text_setting(snapshot, "iconTheme", &default),
    )
}

/// Start Favorites from the settings snapshot; absent or non-text values mean no favorites.
#[must_use]
pub fn start_favorites(snapshot: Option<&SettingsSnapshot>) -> Vec<String> {
    match snapshot.and_then(|settings| {
        settings
            .values
            .get(flamewm_platform::settings::START_FAVORITES_KEY)
    }) {
        Some(SettingValue::Text(text)) => flamewm_shell_core::start_menu::decode_favorites(text),
        _ => Vec::new(),
    }
}

pub(super) fn text_setting(snapshot: &SettingsSnapshot, key: &str, default: &str) -> String {
    match snapshot.values.get(key) {
        Some(SettingValue::Text(value)) if !value.trim().is_empty() => value.clone(),
        _ => default.to_owned(),
    }
}

pub(super) fn bool_setting(snapshot: &SettingsSnapshot, key: &str, default: bool) -> bool {
    match snapshot.values.get(key) {
        Some(SettingValue::Boolean(value)) => *value,
        _ => default,
    }
}

pub(super) fn integer_setting(snapshot: &SettingsSnapshot, key: &str, default: i64) -> i64 {
    match snapshot.values.get(key) {
        Some(SettingValue::Integer(value)) => *value,
        _ => default,
    }
}

pub(super) fn percent_setting(snapshot: &SettingsSnapshot, key: &str, default: u8) -> u8 {
    match snapshot.values.get(key) {
        Some(SettingValue::Percent(value)) => *value,
        _ => default,
    }
}
