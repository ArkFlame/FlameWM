//! Settings readers: per-output shell scales, chrome, edge snap, and media volume cap.

use super::*;

/// Live media-key volume cap: the shared `audioRaiseMaximum` toggle, off unless explicitly set.
pub(super) fn volume_cap(snapshot: &flamewm_api::settings::SettingsSnapshot) -> u8 {
    use flamewm_api::settings::SettingValue;
    let raise = matches!(
        snapshot
            .values
            .get(flamewm_platform::system::AUDIO_RAISE_MAXIMUM_KEY),
        Some(SettingValue::Boolean(true))
    );
    flamewm_platform::system::volume_cap_percent(raise)
}

/// The per-output shell scales saved in the `FlameWM` settings file.
///
/// Entries that are not a supported scale are dropped rather than reported, so a corrupt or
/// hand-edited value cannot present a scale the rest of the product would reject.
pub(super) fn saved_shell_scales() -> BTreeMap<String, u16> {
    shell_scales_at(&settings_path())
}

/// `titlebars` and `windowMargin` from a committed settings snapshot.
///
/// Read the way X11 `live_settings` reads them: a missing or ill-typed value keeps the product
/// default, and a margin outside the supported range falls back rather than being clamped silently.
pub(super) fn chrome_settings(snapshot: &flamewm_api::settings::SettingsSnapshot) -> (bool, i32) {
    use flamewm_api::settings::SettingValue;
    use flamewm_platform::settings::ProductSettings;

    /// Upper bound `ProductSettings::validate` enforces for `windowMargin`.
    const MAX_WINDOW_MARGIN: i64 = 32;

    let defaults = ProductSettings::default();
    let titlebars = match snapshot.values.get("titlebars") {
        Some(SettingValue::Boolean(value)) => *value,
        _ => defaults.titlebars,
    };
    let window_margin = match snapshot.values.get("windowMargin") {
        Some(SettingValue::Integer(value)) if (0..=MAX_WINDOW_MARGIN).contains(value) => {
            i32::try_from(*value).unwrap_or(i32::from(defaults.window_margin))
        }
        _ => i32::from(defaults.window_margin),
    };
    (titlebars, window_margin)
}

/// `(edgeSnap.enabled, edgeSnap.thresholdPx)` from a committed snapshot. Mirrors X11
/// `live_settings`: a missing or out-of-range threshold keeps the default.
pub(super) fn edge_snap_settings(
    snapshot: &flamewm_api::settings::SettingsSnapshot,
) -> (bool, i32) {
    use flamewm_api::settings::SettingValue;

    let enabled = match snapshot.values.get("edgeSnap.enabled") {
        Some(SettingValue::Boolean(value)) => *value,
        _ => true,
    };
    let threshold = match snapshot.values.get("edgeSnap.thresholdPx") {
        Some(SettingValue::Integer(value)) if (4..=64).contains(value) => *value as i32,
        _ => flamewm_window_core::EDGE_THRESHOLD,
    };
    (enabled, threshold)
}

/// Reads per-output scales from `path`. Split from `settings_path` so tests read a real file.
pub(super) fn shell_scales_at(path: &std::path::Path) -> BTreeMap<String, u16> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| shell_scales_from(&text))
        .unwrap_or_default()
}

/// Parses settings text into per-output scales.
///
/// `parse_snapshot` validates every `scale.*` entry against the same 100..=200 in 5% steps rule
/// Settings enforces, and rejects the whole file when one is out of range. A rejected file
/// therefore yields no scales at all, which is why this needs no filter of its own.
pub(super) fn shell_scales_from(text: &str) -> BTreeMap<String, u16> {
    flamewm_platform::settings::ConfigStore::default()
        .parse_snapshot(text)
        .map(|settings| settings.per_output_scale)
        .unwrap_or_default()
}
