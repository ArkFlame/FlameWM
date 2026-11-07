//! Input settings readers: focus-follows-mouse and model (titlebar, edge snap) settings.

/// `focusFollowsMouse` saved in the settings file.
///
/// Mouse-driven focus is opt-in, so a missing or unreadable file keeps the X11 default (off).
pub fn saved_focus_follows_mouse() -> bool {
    focus_follows_mouse_at(&crate::control::settings_path())
}

/// Reads `focusFollowsMouse` from `path`. Split from `settings_path` so the file is really read in
/// tests; the path itself comes from the environment, which is resolved in one place.
pub fn focus_follows_mouse_at(path: &std::path::Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .is_some_and(|text| focus_follows_mouse_from(&text))
}

/// Parses `focusFollowsMouse` out of settings text; an unreadable file means the default, off.
pub(super) fn focus_follows_mouse_from(text: &str) -> bool {
    flamewm_platform::settings::ConfigStore::default()
        .parse_snapshot(text)
        .is_ok_and(|settings| settings.effective().focus_follows_mouse)
}

/// `titlebars` and `windowMargin` saved in the settings file, for the model built at startup.
///
/// A missing or unreadable file keeps the product defaults, so an unset settings file lays windows
/// out exactly as an unconfigured model does.
pub fn saved_model_settings() -> (bool, i32) {
    model_settings_at(&crate::control::settings_path())
}

/// Reads `titlebars` and `windowMargin` from `path`. Split from `settings_path` so the file is
/// really read in tests.
pub fn model_settings_at(path: &std::path::Path) -> (bool, i32) {
    std::fs::read_to_string(path)
        .ok()
        .map_or_else(default_model_settings, |text| model_settings_from(&text))
}

/// Parses `titlebars` and `windowMargin` out of settings text.
pub(super) fn model_settings_from(text: &str) -> (bool, i32) {
    flamewm_platform::settings::ConfigStore::default()
        .parse_snapshot(text)
        .map_or_else(
            |_| default_model_settings(),
            |settings| {
                let settings = settings.effective();
                (settings.titlebars, i32::from(settings.window_margin))
            },
        )
}

/// The product defaults, used when the file is absent or rejected.
pub(super) fn default_model_settings() -> (bool, i32) {
    use flamewm_platform::settings::ProductSettings;

    let defaults = ProductSettings::default();
    (defaults.titlebars, i32::from(defaults.window_margin))
}
