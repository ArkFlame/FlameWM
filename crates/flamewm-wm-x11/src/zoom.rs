//! Magnifier state on X11: the `zoomLevel` setting and the Super+Alt zoom shortcuts.
//!
//! X11 has no output-scaling path yet. The Wayland magnifier scales the frame in its composite
//! pass; the X11 WM has only the `RandR` gamma ramp (`blue_light`), which cannot scale or crop
//! pixels. So this module owns the state only: the level is committed from the settings sync
//! point and stepped by the shortcuts, and nothing is drawn at a level above 1.0 on X11.
//! The `RandR` CRTC transform is the candidate scanout mechanism; it is not implemented here.
//!
//! Precedent: `wm-wayland/src/zoom.rs` (same step and clamp, mirrored here because that module
//! is Smithay-facing and this crate must not depend on it). Choice only; nothing copied.

use std::cell::Cell;

use flamewm_api::settings::{
    SettingValue, SettingsSnapshot, ZOOM_LEVEL_MAX, ZOOM_LEVEL_MIN, parse_zoom_level,
};
use flamewm_api::shortcuts::{ACTION_ZOOM_IN, ACTION_ZOOM_OUT, ACTION_ZOOM_RESET};

/// Zoom change per shortcut press. Mirrors `wm-wayland` zoom.rs.
const STEP: f64 = 0.5;

thread_local! {
    /// Level committed from `zoomLevel` at the last settings sync; reset target for runtime zoom.
    static COMMITTED: Cell<f64> = const { Cell::new(ZOOM_LEVEL_MIN) };
    /// Runtime magnifier level, changed by the zoom shortcuts and reset by a settings sync.
    static LEVEL: Cell<f64> = const { Cell::new(ZOOM_LEVEL_MIN) };
}

/// The committed `zoomLevel`, clamped to 1.0..=8.0; a missing or non-numeric value is 1.0.
pub(crate) fn committed_from_snapshot(snapshot: &SettingsSnapshot) -> f64 {
    match snapshot.values.get("zoomLevel") {
        Some(SettingValue::Text(value)) => parse_zoom_level(value).unwrap_or(ZOOM_LEVEL_MIN),
        _ => ZOOM_LEVEL_MIN,
    }
}

/// Records the committed level from the settings sync point and resets the runtime level to it.
pub(crate) fn set_committed(level: f64) {
    COMMITTED.with(|slot| slot.set(level));
    LEVEL.with(|slot| slot.set(level));
}

/// The runtime magnifier level (read by tests; the shortcut path only steps it).
#[cfg(test)]
pub(crate) fn level() -> f64 {
    LEVEL.with(Cell::get)
}

/// The zoom level after a zoom shortcut, or `None` when `action` is not a zoom action.
pub(crate) fn after_action(zoom: f64, action: &str) -> Option<f64> {
    match action {
        ACTION_ZOOM_IN => Some((zoom + STEP).min(ZOOM_LEVEL_MAX)),
        ACTION_ZOOM_OUT => Some((zoom - STEP).max(ZOOM_LEVEL_MIN)),
        ACTION_ZOOM_RESET => Some(ZOOM_LEVEL_MIN),
        _ => None,
    }
}

/// Applies a zoom shortcut to the runtime level. Returns false for any other action.
pub(crate) fn dispatch(action: &str) -> bool {
    let Some(next) = LEVEL.with(|slot| {
        let next = after_action(slot.get(), action)?;
        slot.set(next);
        Some(next)
    }) else {
        return false;
    };
    eprintln!("flamewm: zoom level {next} (state only; X11 has no magnifier output path)");
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset_state() {
        set_committed(ZOOM_LEVEL_MIN);
    }

    #[test]
    fn zoom_shortcuts_step_clamp_and_reset() {
        assert_eq!(after_action(1.0, ACTION_ZOOM_IN), Some(1.5));
        assert_eq!(after_action(7.8, ACTION_ZOOM_IN), Some(8.0));
        assert_eq!(after_action(1.2, ACTION_ZOOM_OUT), Some(1.0));
        assert_eq!(after_action(3.0, ACTION_ZOOM_RESET), Some(1.0));
        assert_eq!(after_action(3.0, "WindowClose"), None);
    }

    #[test]
    fn dispatch_steps_the_runtime_level_and_ignores_other_actions() {
        reset_state();
        assert!(dispatch(ACTION_ZOOM_IN));
        assert!(dispatch(ACTION_ZOOM_IN));
        assert_eq!(level(), 2.0);
        assert!(dispatch(ACTION_ZOOM_RESET));
        assert_eq!(level(), 1.0);
        assert!(!dispatch("WindowClose"));
        assert_eq!(level(), 1.0);
    }

    #[test]
    fn settings_sync_resets_runtime_zoom_to_the_committed_level() {
        reset_state();
        set_committed(2.5);
        assert_eq!(level(), 2.5);
        assert!(dispatch(ACTION_ZOOM_IN));
        assert_eq!(level(), 3.0);
        set_committed(2.5);
        assert_eq!(level(), 2.5);
        reset_state();
    }

    #[test]
    fn committed_level_is_read_clamped_and_defaults_to_one() {
        let mut snapshot = SettingsSnapshot::new(0);
        assert_eq!(committed_from_snapshot(&snapshot), 1.0);
        snapshot
            .values
            .insert("zoomLevel".to_owned(), SettingValue::Text("99".to_owned()));
        assert_eq!(committed_from_snapshot(&snapshot), 8.0);
        snapshot.values.insert(
            "zoomLevel".to_owned(),
            SettingValue::Text("nope".to_owned()),
        );
        assert_eq!(committed_from_snapshot(&snapshot), 1.0);
    }
}
