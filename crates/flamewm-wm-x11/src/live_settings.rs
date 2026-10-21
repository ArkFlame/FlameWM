//! Live WM copies of window-behaviour settings, refreshed at the settings sync point.
//!
//! Missing or non-boolean values keep the defaults from `flamewm-platform` settings.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_platform::chrome::parse_hex_rgb;

const DEFAULT_SNAP_PREVIEW_ENABLED: bool = true;
const DEFAULT_FOCUS_FOLLOWS_MOUSE: bool = false;
/// Neutral grey until a settings snapshot supplies `accent`.
const DEFAULT_ACCENT_RGB: (u8, u8, u8) = (0x7f, 0x87, 0x8e);

static SNAP_PREVIEW_ENABLED: AtomicBool = AtomicBool::new(DEFAULT_SNAP_PREVIEW_ENABLED);
static FOCUS_FOLLOWS_MOUSE: AtomicBool = AtomicBool::new(DEFAULT_FOCUS_FOLLOWS_MOUSE);
/// Live `accent` packed as `0x00RRGGBB`.
static ACCENT_RGB: AtomicU32 = AtomicU32::new(pack_rgb(DEFAULT_ACCENT_RGB));

const fn pack_rgb((r, g, b): (u8, u8, u8)) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

fn boolean_setting(snapshot: &SettingsSnapshot, key: &str, default: bool) -> bool {
    match snapshot.values.get(key) {
        Some(SettingValue::Boolean(value)) => *value,
        _ => default,
    }
}

/// Pure mapping from a settings snapshot to `(snapPreviewEnabled, focusFollowsMouse)`.
fn window_behaviour_from_snapshot(snapshot: &SettingsSnapshot) -> (bool, bool) {
    (
        boolean_setting(snapshot, "snapPreviewEnabled", DEFAULT_SNAP_PREVIEW_ENABLED),
        boolean_setting(snapshot, "focusFollowsMouse", DEFAULT_FOCUS_FOLLOWS_MOUSE),
    )
}

/// Pure mapping from a settings snapshot to the `accent` colour. Accepts `#RRGGBB` and bare
/// `RRGGBB`, the same forms the shell accepts; invalid or missing values keep the neutral default.
fn accent_from_snapshot(snapshot: &SettingsSnapshot) -> (u8, u8, u8) {
    match snapshot.values.get("accent") {
        Some(SettingValue::Text(value)) => {
            parse_hex_rgb(&format!("#{}", value.trim_start_matches('#')))
                .unwrap_or(DEFAULT_ACCENT_RGB)
        }
        _ => DEFAULT_ACCENT_RGB,
    }
}

/// Called from the single settings sync point on the WM thread.
pub(crate) fn sync(snapshot: &SettingsSnapshot) {
    let (snap_preview, focus_follows_mouse) = window_behaviour_from_snapshot(snapshot);
    SNAP_PREVIEW_ENABLED.store(snap_preview, Ordering::Relaxed);
    FOCUS_FOLLOWS_MOUSE.store(focus_follows_mouse, Ordering::Relaxed);
    ACCENT_RGB.store(pack_rgb(accent_from_snapshot(snapshot)), Ordering::Relaxed);
}

/// Live system accent as `(r, g, b)`; neutral default before the first snapshot.
pub(crate) fn accent_rgb() -> (u8, u8, u8) {
    let packed = ACCENT_RGB.load(Ordering::Relaxed);
    ((packed >> 16) as u8, (packed >> 8) as u8, packed as u8)
}

pub(crate) fn snap_preview_enabled() -> bool {
    SNAP_PREVIEW_ENABLED.load(Ordering::Relaxed)
}

pub(crate) fn focus_follows_mouse() -> bool {
    FOCUS_FOLLOWS_MOUSE.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(entries: &[(&str, SettingValue)]) -> SettingsSnapshot {
        let mut snapshot = SettingsSnapshot::new(1);
        for (key, value) in entries {
            snapshot.values.insert((*key).into(), value.clone());
        }
        snapshot
    }

    #[test]
    fn missing_keys_keep_defaults() {
        assert_eq!(
            window_behaviour_from_snapshot(&SettingsSnapshot::new(1)),
            (true, false)
        );
    }

    #[test]
    fn explicit_booleans_map_directly() {
        let mapped = window_behaviour_from_snapshot(&snapshot(&[
            ("snapPreviewEnabled", SettingValue::Boolean(false)),
            ("focusFollowsMouse", SettingValue::Boolean(true)),
        ]));
        assert_eq!(mapped, (false, true));
    }

    #[test]
    fn missing_accent_keeps_neutral_default() {
        assert_eq!(
            accent_from_snapshot(&SettingsSnapshot::new(1)),
            (0x7f, 0x87, 0x8e)
        );
    }

    #[test]
    fn accent_accepts_hash_and_bare_hex() {
        assert_eq!(
            accent_from_snapshot(&snapshot(&[(
                "accent",
                SettingValue::Text("#ef4048".into())
            )])),
            (0xef, 0x40, 0x48)
        );
        assert_eq!(
            accent_from_snapshot(&snapshot(&[(
                "accent",
                SettingValue::Text("123456".into())
            )])),
            (0x12, 0x34, 0x56)
        );
    }

    #[test]
    fn invalid_accent_falls_back_to_neutral_default() {
        assert_eq!(
            accent_from_snapshot(&snapshot(&[(
                "accent",
                SettingValue::Text("#GG0000".into())
            )])),
            (0x7f, 0x87, 0x8e)
        );
    }

    #[test]
    fn non_boolean_values_fall_back_to_defaults() {
        let mapped = window_behaviour_from_snapshot(&snapshot(&[
            ("snapPreviewEnabled", SettingValue::Integer(0)),
            ("focusFollowsMouse", SettingValue::Integer(1)),
        ]));
        assert_eq!(mapped, (true, false));
    }
}
