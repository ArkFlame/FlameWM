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

pub(super) use flamewm_api::settings::{
    setting_bool as bool_setting, setting_integer as integer_setting,
    setting_percent as percent_setting, setting_text as text_setting,
};
