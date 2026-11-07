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

/// `(showContentWhileDragging, snapPreviewEnabled)` saved in the settings file. Read at startup
/// like the other saved window settings: the live sync runs only on a bus turn, so a value that is
/// not applied here stays at its compiled-in default until something else talks to the bus.
pub fn saved_drag_feedback() -> (bool, bool) {
    drag_feedback_at(&crate::control::settings_path())
}

/// Reads the drag-feedback pair from `path`. Split from `settings_path` so the file is really read
/// in tests.
pub fn drag_feedback_at(path: &std::path::Path) -> (bool, bool) {
    std::fs::read_to_string(path)
        .ok()
        .map_or_else(default_drag_feedback, |text| drag_feedback_from(&text))
}

/// Parses the drag-feedback pair out of settings text, through the same group rules Settings uses,
/// so `showContentWhileDragging` counts only while its `windowsCustom` switch is on.
pub(super) fn drag_feedback_from(text: &str) -> (bool, bool) {
    flamewm_platform::settings::ConfigStore::default()
        .parse_snapshot(text)
        .map_or_else(
            |_| default_drag_feedback(),
            |settings| {
                let settings = settings.effective();
                (settings.show_content_while_dragging, settings.snap_preview_enabled)
            },
        )
}

fn default_drag_feedback() -> (bool, bool) {
    use flamewm_platform::settings::ProductSettings;

    let defaults = ProductSettings::default();
    (defaults.show_content_while_dragging, defaults.snap_preview_enabled)
}

#[cfg(test)]
mod drag_feedback_tests {
    use super::drag_feedback_from;

    #[test]
    fn saved_content_drag_applies_at_startup_without_a_bus_turn() {
        // The switch is migrated on when a member is off default, so a bare saved value counts.
        assert_eq!(drag_feedback_from("showContentWhileDragging=true\n"), (true, true));
    }

    #[test]
    fn explicit_windows_switch_off_gates_the_saved_content_drag() {
        assert_eq!(
            drag_feedback_from("windowsCustom=false\nshowContentWhileDragging=true\n"),
            (false, true)
        );
    }

    #[test]
    fn snap_preview_off_is_read_and_defaults_stay_outline_and_preview() {
        assert_eq!(drag_feedback_from("windowsCustom=true\nsnapPreviewEnabled=false\n"), (false, false));
        assert_eq!(drag_feedback_from(""), (false, true));
    }
}
