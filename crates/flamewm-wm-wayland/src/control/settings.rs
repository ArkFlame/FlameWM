//! Settings readers: per-output shell scales, chrome, edge snap, and media volume cap.

use super::*;

/// `showContentWhileDragging`: true moves the live window during a drag; false (the default, as on
/// X11) keeps the window in place and draws an outline of where it would land.
pub(super) fn show_content_while_dragging(
    snapshot: &flamewm_api::settings::SettingsSnapshot,
) -> bool {
    use flamewm_api::settings::SettingValue;
    matches!(
        snapshot.values.get("showContentWhileDragging"),
        Some(SettingValue::Boolean(true))
    )
}

/// `snapPreviewEnabled`: the snap-zone ring while a move targets a zone. On by default, as on X11
/// `live_settings`; only an explicit `false` hides it.
pub(super) fn snap_preview_enabled(snapshot: &flamewm_api::settings::SettingsSnapshot) -> bool {
    use flamewm_api::settings::SettingValue;
    match snapshot.values.get("snapPreviewEnabled") {
        Some(SettingValue::Boolean(value)) => *value,
        _ => true,
    }
}

/// `zoomLevel`: the magnifier factor, clamped to 1.0..=8.0; a missing or non-numeric value is 1.0.
pub(super) fn zoom_level(snapshot: &flamewm_api::settings::SettingsSnapshot) -> f64 {
    use flamewm_api::settings::{SettingValue, ZOOM_LEVEL_MIN, parse_zoom_level};
    match snapshot.values.get("zoomLevel") {
        Some(SettingValue::Text(value)) => parse_zoom_level(value).unwrap_or(ZOOM_LEVEL_MIN),
        _ => ZOOM_LEVEL_MIN,
    }
}

/// `colourFilter`: the text value names a filter; a missing or non-text value keeps `None`.
pub(super) fn colour_filter(
    snapshot: &flamewm_api::settings::SettingsSnapshot,
) -> crate::colour_filter::ColourFilter {
    use flamewm_api::settings::SettingValue;
    match snapshot.values.get("colourFilter") {
        Some(SettingValue::Text(value)) => crate::colour_filter::ColourFilter::from_setting(value),
        _ => crate::colour_filter::ColourFilter::None,
    }
}

#[cfg(test)]
mod colour_filter_tests {
    use crate::colour_filter::ColourFilter;
    use flamewm_api::settings::{SettingValue, SettingsSnapshot};
    use std::collections::BTreeMap;

    fn snapshot(value: Option<SettingValue>) -> SettingsSnapshot {
        let mut values = BTreeMap::new();
        if let Some(value) = value {
            values.insert("colourFilter".to_owned(), value);
        }
        SettingsSnapshot {
            revision: 1,
            values,
        }
    }

    #[test]
    fn missing_or_ill_typed_value_is_no_filter() {
        assert_eq!(super::colour_filter(&snapshot(None)), ColourFilter::None);
        assert_eq!(
            super::colour_filter(&snapshot(Some(SettingValue::Boolean(true)))),
            ColourFilter::None
        );
    }

    #[test]
    fn text_value_selects_the_named_filter() {
        assert_eq!(
            super::colour_filter(&snapshot(Some(SettingValue::Text("deuteranopia".into())))),
            ColourFilter::Deuteranopia
        );
        assert_eq!(
            super::colour_filter(&snapshot(Some(SettingValue::Text("bogus".into())))),
            ColourFilter::None
        );
    }
}

#[cfg(test)]
mod snap_preview_tests {
    use flamewm_api::settings::{SettingValue, SettingsSnapshot};
    use std::collections::BTreeMap;

    fn snapshot(value: Option<SettingValue>) -> SettingsSnapshot {
        let mut values = BTreeMap::new();
        if let Some(value) = value {
            values.insert("snapPreviewEnabled".to_owned(), value);
        }
        SettingsSnapshot {
            revision: 1,
            values,
        }
    }

    #[test]
    fn preview_is_on_by_default_and_only_false_hides_it() {
        assert!(super::snap_preview_enabled(&snapshot(None)));
        assert!(super::snap_preview_enabled(&snapshot(Some(
            SettingValue::Boolean(true)
        ))));
        assert!(!super::snap_preview_enabled(&snapshot(Some(
            SettingValue::Boolean(false)
        ))));
    }
}

#[cfg(test)]
mod content_drag_tests {
    use flamewm_api::settings::{SettingValue, SettingsSnapshot};
    use std::collections::BTreeMap;

    fn snapshot(value: Option<SettingValue>) -> SettingsSnapshot {
        let mut values = BTreeMap::new();
        if let Some(value) = value {
            values.insert("showContentWhileDragging".to_owned(), value);
        }
        SettingsSnapshot {
            revision: 1,
            values,
        }
    }

    #[test]
    fn outline_is_the_default_and_content_needs_true() {
        assert!(!super::show_content_while_dragging(&snapshot(None)));
        assert!(!super::show_content_while_dragging(&snapshot(Some(
            SettingValue::Boolean(false)
        ))));
        assert!(!super::show_content_while_dragging(&snapshot(Some(
            SettingValue::Text("true".into())
        ))));
        assert!(super::show_content_while_dragging(&snapshot(Some(
            SettingValue::Boolean(true)
        ))));
    }
}

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

/// The saved scale of the nested output in percent (100 when none is saved), read at startup.
pub(crate) fn nested_output_scale_percent() -> u16 {
    saved_shell_scales()
        .get(OUTPUT_NAME)
        .copied()
        .unwrap_or(100)
}

/// The saved scale of the output at `index` in the display snapshot (see `output_layout`).
#[cfg_attr(not(feature = "drm"), allow(dead_code))] // only the drm backend builds outputs per index
pub(crate) fn output_scale_percent(index: usize) -> u16 {
    crate::output_layout::output_scale_percent(&saved_shell_scales(), index)
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
