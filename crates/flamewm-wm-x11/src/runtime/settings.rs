use super::*;

pub fn persisted_cursor_theme() -> Option<String> {
    persisted_settings().map(|settings| settings.cursor_theme)
}

/// Persisted `appearance` from the settings file at startup. `None` when the
/// file is missing or unparsable.
#[must_use]
pub fn persisted_appearance() -> Option<AppearanceMode> {
    persisted_settings().map(|settings| settings.appearance)
}

pub(super) fn persisted_settings() -> Option<flamewm_platform::settings::ProductSettings> {
    let (path, _) = settings_path();
    let text = std::fs::read_to_string(path).ok()?;
    flamewm_platform::settings::ConfigStore::default()
        .parse_snapshot(&text)
        .ok()
        .map(|settings| settings.effective())
}

/// Single settings sync point: applies the WM-owned snapshot effects, then
/// notifies the optional observer with the same snapshot.
pub(super) fn publish_settings_snapshot(
    observer: &SharedSettingsObserver,
    snapshot: &SettingsSnapshot,
) {
    sync_snap_preview_opacity(snapshot);
    crate::live_settings::sync(snapshot);
    sync_window_decoration(snapshot);
    sync_idle_lock_timeout(snapshot);
    // The filter itself is applied on the WM loop turn; this only records the committed values.
    crate::blue_light::set_config(flamewm_api::blue_light::BlueLightConfig::from_snapshot(
        snapshot,
    ));
    crate::blue_light::set_invert(flamewm_api::blue_light::gamma_inverts(snapshot));
    crate::zoom::set_committed(crate::zoom::committed_from_snapshot(snapshot));
    crate::zoom::set_follow(crate::zoom::follow_from_snapshot(snapshot));
    if let Some(observer) = observer.borrow_mut().as_mut() {
        observer(snapshot);
    }
}

pub(super) fn sync_window_decoration(snapshot: &SettingsSnapshot) {
    crate::chrome::set_live_chrome_tokens(crate::chrome::chrome_tokens_from_settings(snapshot));
    crate::animation::set_live_settings(crate::animation::settings_from_snapshot(snapshot));
}

pub(super) fn sync_idle_lock_timeout(snapshot: &SettingsSnapshot) {
    // Missing or out-of-range values fall back to manual lock only (0).
    let minutes = match snapshot.values.get("lockTimeoutMinutes") {
        Some(SettingValue::Integer(minutes)) => u16::try_from(*minutes).unwrap_or(0),
        _ => 0,
    };
    idle::set_live_lock_timeout(minutes);
}

pub(super) fn sync_snap_preview_opacity(snapshot: &SettingsSnapshot) {
    if let Some(SettingValue::Percent(opacity)) =
        snapshot.values.get("WindowSnapPreviewFillOpacity")
    {
        crate::snap_preview::set_live_opacity(*opacity);
    }
}

/// Drain one `WmChangeSet` once per reactor turn: refresh only required
/// snapshots, emit one snapshot signal per changed domain plus legacy
/// revision-only compat. MANAGE/MINIMIZE/RESTORE/CLOSE -> Windows+Panels,
/// WORKSPACE SWITCH -> Workspaces+Panels. `work_area`-only turns emit nothing.
/// Resolves the product settings file. Precedence: `FLAMEWM_SETTINGS_PATH`,
/// then `FLAMEWM_CONFIG_DIR/settings.toml`, then the XDG default
/// (`XDG_CONFIG_HOME` or `$HOME/.config`, `/flamewm/settings.toml`). The
/// last fallback never depends on the current working directory.
pub(crate) fn settings_path() -> (PathBuf, &'static str) {
    if let Some(path) = env::var_os("FLAMEWM_SETTINGS_PATH").filter(|value| !value.is_empty()) {
        return (PathBuf::from(path), "FLAMEWM_SETTINGS_PATH");
    }
    if let Some(dir) = env::var_os("FLAMEWM_CONFIG_DIR").filter(|value| !value.is_empty()) {
        return (
            PathBuf::from(dir).join("settings.toml"),
            "FLAMEWM_CONFIG_DIR",
        );
    }
    let config_home = env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("/root/.config"));
    (
        config_home.join("flamewm").join("settings.toml"),
        "xdg-default",
    )
}
