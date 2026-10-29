//! Live WM copies of window-behaviour settings, refreshed at the settings sync point.
//!
//! Missing or non-boolean values keep the defaults from `flamewm-platform` settings.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_platform::chrome::parse_hex_rgb;

const DEFAULT_SNAP_PREVIEW_ENABLED: bool = true;
const DEFAULT_FOCUS_FOLLOWS_MOUSE: bool = false;
/// XP style: moves and resizes show an outline and apply the real geometry on release.
const DEFAULT_SHOW_CONTENT_WHILE_DRAGGING: bool = false;
/// `titlebars`: Server decoration draws the Flame titlebar band.
const DEFAULT_TITLEBARS: bool = true;
/// `taskbarActivities`: the shell shows running windows, so they may be minimized.
const DEFAULT_TASKBAR_ACTIVITIES: bool = true;
/// Neutral grey until a settings snapshot supplies `accent`.
const DEFAULT_ACCENT_RGB: (u8, u8, u8) = (0x7f, 0x87, 0x8e);
const DEFAULT_WINDOW_MARGIN: u32 = 2;
const MAX_WINDOW_MARGIN: u32 = 32;

static SNAP_PREVIEW_ENABLED: AtomicBool = AtomicBool::new(DEFAULT_SNAP_PREVIEW_ENABLED);
static FOCUS_FOLLOWS_MOUSE: AtomicBool = AtomicBool::new(DEFAULT_FOCUS_FOLLOWS_MOUSE);
static SHOW_CONTENT_WHILE_DRAGGING: AtomicBool =
    AtomicBool::new(DEFAULT_SHOW_CONTENT_WHILE_DRAGGING);
static TITLEBARS: AtomicBool = AtomicBool::new(DEFAULT_TITLEBARS);
static TASKBAR_ACTIVITIES: AtomicBool = AtomicBool::new(DEFAULT_TASKBAR_ACTIVITIES);
/// `shareWallpaper`: one wallpaper spans the outputs instead of one copy per output.
static SHARE_WALLPAPER: AtomicBool = AtomicBool::new(false);
/// `surfaceSaturationPercent`: backdrop saturation behind translucent titlebars (100 is identity).
static SATURATION_PERCENT: AtomicU32 =
    AtomicU32::new(flamewm_ui_core::glass::DARK.saturation_percent as u32);
/// Live `accent` packed as `0x00RRGGBB`.
static ACCENT_RGB: AtomicU32 = AtomicU32::new(pack_rgb(DEFAULT_ACCENT_RGB));
static WINDOW_MARGIN: AtomicU32 = AtomicU32::new(DEFAULT_WINDOW_MARGIN);
/// Shared `audioRaiseMaximum`: false until a snapshot says otherwise, matching the shell default.
static AUDIO_RAISE_MAXIMUM: AtomicBool = AtomicBool::new(false);

/// Live volume cap for the media keys, from the shared "raise maximum volume" value.
pub(crate) fn volume_cap_percent() -> u8 {
    flamewm_platform::system::volume_cap_percent(AUDIO_RAISE_MAXIMUM.load(Ordering::Relaxed))
}

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

/// Pure mapping from a settings snapshot to `showContentWhileDragging`.
fn show_content_from_snapshot(snapshot: &SettingsSnapshot) -> bool {
    boolean_setting(
        snapshot,
        "showContentWhileDragging",
        DEFAULT_SHOW_CONTENT_WHILE_DRAGGING,
    )
}

/// Pure mapping from a settings snapshot to the `defaultTerminal` override; empty when unset.
fn terminal_from_snapshot(snapshot: &SettingsSnapshot) -> String {
    match snapshot.values.get("defaultTerminal") {
        Some(SettingValue::Text(value)) => value.clone(),
        _ => String::new(),
    }
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

fn window_margin_from_snapshot(snapshot: &SettingsSnapshot) -> u32 {
    match snapshot.values.get("windowMargin") {
        Some(SettingValue::Integer(value)) => u32::try_from(*value)
            .ok()
            .filter(|margin| *margin <= MAX_WINDOW_MARGIN)
            .unwrap_or(DEFAULT_WINDOW_MARGIN),
        _ => DEFAULT_WINDOW_MARGIN,
    }
}

/// Called from the single settings sync point on the WM thread.
pub(crate) fn sync(snapshot: &SettingsSnapshot) {
    let (snap_preview, focus_follows_mouse) = window_behaviour_from_snapshot(snapshot);
    SNAP_PREVIEW_ENABLED.store(snap_preview, Ordering::Relaxed);
    FOCUS_FOLLOWS_MOUSE.store(focus_follows_mouse, Ordering::Relaxed);
    SHOW_CONTENT_WHILE_DRAGGING.store(show_content_from_snapshot(snapshot), Ordering::Relaxed);
    TITLEBARS.store(
        boolean_setting(snapshot, "titlebars", DEFAULT_TITLEBARS),
        Ordering::Relaxed,
    );
    TASKBAR_ACTIVITIES.store(
        boolean_setting(snapshot, "taskbarActivities", DEFAULT_TASKBAR_ACTIVITIES),
        Ordering::Relaxed,
    );
    SHARE_WALLPAPER.store(
        boolean_setting(snapshot, "shareWallpaper", false),
        Ordering::Relaxed,
    );
    SATURATION_PERCENT.store(
        flamewm_ui_core::surface::glass_material(snapshot).saturation_percent,
        Ordering::Relaxed,
    );
    ACCENT_RGB.store(pack_rgb(accent_from_snapshot(snapshot)), Ordering::Relaxed);
    WINDOW_MARGIN.store(window_margin_from_snapshot(snapshot), Ordering::Relaxed);
    AUDIO_RAISE_MAXIMUM.store(
        boolean_setting(
            snapshot,
            flamewm_platform::system::AUDIO_RAISE_MAXIMUM_KEY,
            false,
        ),
        Ordering::Relaxed,
    );
    flamewm_render_x11::set_render_acceleration(
        match snapshot
            .values
            .get(flamewm_render_x11::RENDER_ACCELERATION_SETTING)
        {
            Some(SettingValue::Text(value)) => {
                flamewm_render_x11::RenderAcceleration::from_setting(value)
            }
            _ => flamewm_render_x11::RenderAcceleration::Cpu,
        },
    );
    flamewm_applications::terminal::set_configured_terminal(&terminal_from_snapshot(snapshot));
}

/// Live system accent as `(r, g, b)`; neutral default before the first snapshot.
pub(crate) fn accent_rgb() -> (u8, u8, u8) {
    let packed = ACCENT_RGB.load(Ordering::Relaxed);
    ((packed >> 16) as u8, (packed >> 8) as u8, packed as u8)
}

pub(crate) fn window_margin() -> i32 {
    WINDOW_MARGIN.load(Ordering::Relaxed) as i32
}

pub(crate) fn snap_preview_enabled() -> bool {
    SNAP_PREVIEW_ENABLED.load(Ordering::Relaxed)
}

pub(crate) fn focus_follows_mouse() -> bool {
    FOCUS_FOLLOWS_MOUSE.load(Ordering::Relaxed)
}

/// `surfaceSaturationPercent` (Dark preset default); 100 leaves the backdrop unchanged.
pub(crate) fn saturation_percent() -> u32 {
    SATURATION_PERCENT.load(Ordering::Relaxed)
}

/// `shareWallpaper` (default `false`): whether the desktop shares one wallpaper over all outputs.
pub(crate) fn share_wallpaper() -> bool {
    SHARE_WALLPAPER.load(Ordering::Relaxed)
}

/// `false` (default) draws an outline during interactive move/resize.
pub(crate) fn show_content_while_dragging() -> bool {
    SHOW_CONTENT_WHILE_DRAGGING.load(Ordering::Relaxed)
}

/// `titlebars` (default `true`): Server frames include the titlebar band.
pub(crate) fn titlebars() -> bool {
    TITLEBARS.load(Ordering::Relaxed)
}

/// `taskbarActivities` (default `true`): the shell lists running windows, so minimize is allowed.
pub(crate) fn taskbar_activities() -> bool {
    TASKBAR_ACTIVITIES.load(Ordering::Relaxed)
}

#[cfg(test)]
mod show_content_tests {
    use super::*;

    #[test]
    fn window_margin_defaults_to_two_and_rejects_out_of_range_values() {
        assert_eq!(window_margin_from_snapshot(&SettingsSnapshot::new(1)), 2);
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot
            .values
            .insert("windowMargin".into(), SettingValue::Integer(8));
        assert_eq!(window_margin_from_snapshot(&snapshot), 8);
        snapshot
            .values
            .insert("windowMargin".into(), SettingValue::Integer(99));
        assert_eq!(window_margin_from_snapshot(&snapshot), 2);
        snapshot
            .values
            .insert("windowMargin".into(), SettingValue::Integer(-1));
        assert_eq!(window_margin_from_snapshot(&snapshot), 2);
    }

    #[test]
    fn show_content_defaults_off_and_maps_booleans() {
        assert!(!show_content_from_snapshot(&SettingsSnapshot::new(1)));
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot.values.insert(
            "showContentWhileDragging".into(),
            SettingValue::Boolean(true),
        );
        assert!(show_content_from_snapshot(&snapshot));
        snapshot.values.insert(
            "showContentWhileDragging".into(),
            SettingValue::Text("yes".into()),
        );
        assert!(!show_content_from_snapshot(&snapshot));
    }
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
    fn raise_maximum_drives_the_media_key_cap_live() {
        let key = flamewm_platform::system::AUDIO_RAISE_MAXIMUM_KEY;
        let raise = |value| {
            AUDIO_RAISE_MAXIMUM.store(
                boolean_setting(&snapshot(&[(key, SettingValue::Boolean(value))]), key, false),
                Ordering::Relaxed,
            );
            volume_cap_percent()
        };
        assert_eq!(raise(true), 150);
        assert_eq!(raise(false), 100);
        AUDIO_RAISE_MAXIMUM.store(false, Ordering::Relaxed);
        assert_eq!(volume_cap_percent(), 100);
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
    fn default_terminal_maps_text_and_defaults_empty() {
        assert_eq!(terminal_from_snapshot(&SettingsSnapshot::new(1)), "");
        let configured = snapshot(&[("defaultTerminal", SettingValue::Text("kitty".into()))]);
        assert_eq!(terminal_from_snapshot(&configured), "kitty");
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

    #[test]
    fn titlebars_and_activities_default_on_and_map_booleans() {
        // Pure mapping only: `sync` writes process globals that parallel WM tests read.
        let empty = SettingsSnapshot::new(1);
        assert!(boolean_setting(&empty, "titlebars", DEFAULT_TITLEBARS));
        assert!(boolean_setting(
            &empty,
            "taskbarActivities",
            DEFAULT_TASKBAR_ACTIVITIES
        ));
        let off = snapshot(&[
            ("titlebars", SettingValue::Boolean(false)),
            ("taskbarActivities", SettingValue::Boolean(false)),
        ]);
        assert!(!boolean_setting(&off, "titlebars", DEFAULT_TITLEBARS));
        assert!(!boolean_setting(
            &off,
            "taskbarActivities",
            DEFAULT_TASKBAR_ACTIVITIES
        ));
        let junk = snapshot(&[("titlebars", SettingValue::Integer(0))]);
        assert!(boolean_setting(&junk, "titlebars", DEFAULT_TITLEBARS));
    }
}
