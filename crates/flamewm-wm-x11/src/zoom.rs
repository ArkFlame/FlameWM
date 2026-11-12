//! Magnifier state on X11: the `zoomLevel` setting and the Super+Alt zoom shortcuts.
//!
//! This module owns the level: it is committed from the settings sync point and stepped by the
//! shortcuts. The output side is `zoom_transform`, which sends the level to the server as a `RandR`
//! CRTC transform (the X11 analogue of the Wayland composite scale).
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
    /// Committed `zoomFollowMode`, read by the output path each turn.
    static FOLLOW: Cell<FollowMode> = const { Cell::new(FollowMode::Proportional) };
}

/// How the magnified viewport tracks the pointer (`zoomFollowMode`), named as in `KWin`.
/// Mirrors `wm-wayland` zoom.rs `FollowMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum FollowMode {
    /// The pointer's place across the output maps to the viewport's place across the frame.
    #[default]
    Proportional,
    /// The pointer stays at the viewport centre, clamped at the output edges.
    Centred,
    /// The viewport holds still until the pointer reaches its edge, then moves just far enough.
    Push,
}

impl FollowMode {
    /// Parses a `zoomFollowMode` value; unknown text is `None`.
    pub(crate) fn from_setting(text: &str) -> Option<Self> {
        match text {
            "proportional" => Some(Self::Proportional),
            "centred" => Some(Self::Centred),
            "push" => Some(Self::Push),
            _ => None,
        }
    }
}

/// The sampled part of the frame, in physical pixels with a top-left origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Viewport {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) w: f64,
    pub(crate) h: f64,
}

/// The source rectangle for `zoom` following `pointer`, in physical pixels of a `size` output.
/// `previous` is the last origin: `Push` holds it, and `None` centres on the pointer. At zoom 1 it
/// is the whole output, and it never leaves the output.
pub(crate) fn viewport(
    mode: FollowMode,
    size: (f64, f64),
    zoom: f64,
    pointer: (f64, f64),
    previous: Option<(f64, f64)>,
) -> Viewport {
    let zoom = zoom.clamp(ZOOM_LEVEL_MIN, ZOOM_LEVEL_MAX);
    let (w, h) = (size.0 / zoom, size.1 / zoom);
    let centred = (pointer.0 - w / 2.0, pointer.1 - h / 2.0);
    let (x, y) = match (mode, previous) {
        (FollowMode::Proportional, _) => (
            pointer.0 / size.0 * (size.0 - w),
            pointer.1 / size.1 * (size.1 - h),
        ),
        (FollowMode::Push, Some((px, py))) => (push(px, pointer.0, w), push(py, pointer.1, h)),
        (FollowMode::Centred | FollowMode::Push, _) => centred,
    };
    Viewport {
        x: x.clamp(0.0, size.0 - w),
        y: y.clamp(0.0, size.1 - h),
        w,
        h,
    }
}

/// The least move of `origin` that keeps `at` inside `[origin, origin + len]`.
fn push(origin: f64, at: f64, len: f64) -> f64 {
    if at < origin {
        at
    } else if at > origin + len {
        at - len
    } else {
        origin
    }
}

/// The committed follow mode; the output path reads it on each turn.
pub(crate) fn follow() -> FollowMode {
    FOLLOW.with(Cell::get)
}

/// The committed `zoomFollowMode`; missing or unknown text is `Proportional`.
pub(crate) fn follow_from_snapshot(snapshot: &SettingsSnapshot) -> FollowMode {
    match snapshot.values.get("zoomFollowMode") {
        Some(SettingValue::Text(value)) => FollowMode::from_setting(value).unwrap_or_default(),
        _ => FollowMode::default(),
    }
}

/// Records the committed follow mode from the settings sync point.
pub(crate) fn set_follow(mode: FollowMode) {
    FOLLOW.with(|slot| slot.set(mode));
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

/// The runtime magnifier level, read by the output path and by tests.
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
    eprintln!("flamewm: zoom level {next}");
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

    const OUTPUT: (f64, f64) = (800.0, 600.0);

    #[test]
    fn follow_mode_parses_the_three_settings_only() {
        assert_eq!(
            FollowMode::from_setting("centred"),
            Some(FollowMode::Centred)
        );
        assert_eq!(FollowMode::from_setting("push"), Some(FollowMode::Push));
        assert_eq!(
            FollowMode::from_setting("proportional"),
            Some(FollowMode::Proportional)
        );
        assert_eq!(FollowMode::from_setting("diagonal"), None);
    }

    #[test]
    fn unit_zoom_samples_the_whole_output_in_every_mode() {
        for mode in [
            FollowMode::Proportional,
            FollowMode::Centred,
            FollowMode::Push,
        ] {
            let v = viewport(mode, OUTPUT, 1.0, (123.0, 456.0), Some((50.0, 50.0)));
            assert_eq!(
                v,
                Viewport {
                    x: 0.0,
                    y: 0.0,
                    w: 800.0,
                    h: 600.0
                },
                "{mode:?}"
            );
        }
    }

    #[test]
    fn centred_keeps_the_pointer_at_the_viewport_centre_and_clamps() {
        let v = viewport(FollowMode::Centred, OUTPUT, 2.0, (400.0, 300.0), None);
        assert_eq!(
            v,
            Viewport {
                x: 200.0,
                y: 150.0,
                w: 400.0,
                h: 300.0
            }
        );
        let corner = viewport(FollowMode::Centred, OUTPUT, 2.0, (0.0, 0.0), None);
        assert_eq!((corner.x, corner.y), (0.0, 0.0));
    }

    #[test]
    fn proportional_maps_the_pointer_across_the_whole_frame() {
        let left = viewport(FollowMode::Proportional, OUTPUT, 2.0, (0.0, 0.0), None);
        let right = viewport(FollowMode::Proportional, OUTPUT, 2.0, (800.0, 600.0), None);
        assert_eq!((left.x, left.y), (0.0, 0.0));
        assert_eq!((right.x, right.y), (400.0, 300.0));
    }

    #[test]
    fn push_holds_until_the_pointer_reaches_an_edge() {
        let held = viewport(
            FollowMode::Push,
            OUTPUT,
            2.0,
            (250.0, 200.0),
            Some((200.0, 150.0)),
        );
        assert_eq!((held.x, held.y), (200.0, 150.0));
        let pushed = viewport(
            FollowMode::Push,
            OUTPUT,
            2.0,
            (700.0, 500.0),
            Some((200.0, 150.0)),
        );
        assert_eq!((pushed.x, pushed.y), (300.0, 200.0));
    }
}
