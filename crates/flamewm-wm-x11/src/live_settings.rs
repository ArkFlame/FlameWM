//! Live WM copies of window-behaviour settings, refreshed at the settings sync point.
//!
//! Missing or non-boolean values keep the defaults from `flamewm-platform` settings.

use std::sync::atomic::{AtomicBool, Ordering};

use flamewm_api::settings::{SettingValue, SettingsSnapshot};

const DEFAULT_SNAP_PREVIEW_ENABLED: bool = true;
const DEFAULT_FOCUS_FOLLOWS_MOUSE: bool = true;

static SNAP_PREVIEW_ENABLED: AtomicBool = AtomicBool::new(DEFAULT_SNAP_PREVIEW_ENABLED);
static FOCUS_FOLLOWS_MOUSE: AtomicBool = AtomicBool::new(DEFAULT_FOCUS_FOLLOWS_MOUSE);

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

/// Called from the single settings sync point on the WM thread.
pub(crate) fn sync(snapshot: &SettingsSnapshot) {
    let (snap_preview, focus_follows_mouse) = window_behaviour_from_snapshot(snapshot);
    SNAP_PREVIEW_ENABLED.store(snap_preview, Ordering::Relaxed);
    FOCUS_FOLLOWS_MOUSE.store(focus_follows_mouse, Ordering::Relaxed);
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
            (true, true)
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
    fn non_boolean_values_fall_back_to_defaults() {
        let mapped = window_behaviour_from_snapshot(&snapshot(&[
            ("snapPreviewEnabled", SettingValue::Integer(0)),
            ("focusFollowsMouse", SettingValue::Integer(1)),
        ]));
        assert_eq!(mapped, (true, true));
    }
}
