//! Split from `lib.rs` (`consts`); see crate docs.

use super::*;

pub(crate) const DONATE_URI: &str = "https://paypal.me/LinsaFTW";
pub(crate) const SOURCE_URI: &str = "https://github.com/ArkFlame/flamewm";
pub(crate) const WEBSITE_URI: &str = "https://wm.arkflame.com";

/// Canonical product version: the workspace version inherited by this crate.
pub(crate) const APP_VERSION: &str = env!("FLAMEWM_SETTINGS_VERSION");
/// Authoritative accent for the native `FlameWM` preset.
pub(crate) const DEFAULT_ACCENT: &str = "#539BFF";
/// Nodes whose compiled colour is the accent and must follow live accent changes.
/// Every `toggle-on` id in ui/settings.html must be listed here (checked by a test).
pub(crate) const ACCENT_NODE_IDS: &[&str] = &[
    "nav-selection-accent",
    "accent-custom-on",
    "reduce-transparency-on",
    "anim-global-on",
    "anim-start-on",
    "anim-window-on",
    "anim-minimize-on",
    "anim-maximize-on",
    "lock-date-on",
    "lock-username-on",
    "lock-suspend-on",
    "sticky-toggle-on",
    "downloads-organize-on",
    "focus-mouse-on",
    "titlebars-on",
    "snap-preview-on",
    "show-content-on",
    "taskbar-seconds-on",
    "taskbar-date-on",
    "taskbar-weather-on",
    "taskbar-volume-on",
    "taskbar-network-on",
    "taskbar-battery-on",
    "taskbar-battery-relevant-on",
    "taskbar-clock-on",
    "taskbar-workspaces-on",
    "taskbar-activities-on",
    "taskbar-separate-on",
    "assistant-enabled-on",
    "assistant-taskbar-icon-on",
    "assistant-speech-to-text-on",
    "assistant-text-to-speech-on",
    "weather-enabled-on",
    "weather-auto-location-on",
    "time-auto-zone-on",
    "font-toggle-on",
    "wallpaper-share-on",
];
/// Boolean ON/OFF twins: (setting key, ON id, OFF id, default when the key is absent).
pub(crate) const SETTING_TWINS: &[(&str, &str, &str, bool)] = &[
    (
        "shareWallpaper",
        "wallpaper-share-on",
        "wallpaper-share-off",
        false,
    ),
    (
        "accentCustom",
        "accent-custom-on",
        "accent-custom-off",
        false,
    ),
    (
        "taskbarShowWeather",
        "taskbar-weather-on",
        "taskbar-weather-off",
        true,
    ),
    (
        "taskbarShowVolume",
        "taskbar-volume-on",
        "taskbar-volume-off",
        true,
    ),
    (
        "taskbarShowNetwork",
        "taskbar-network-on",
        "taskbar-network-off",
        true,
    ),
    (
        "organizeDownloads",
        "downloads-organize-on",
        "downloads-organize-off",
        true,
    ),
    (
        "taskbarShowBattery",
        "taskbar-battery-on",
        "taskbar-battery-off",
        true,
    ),
    (
        "taskbarBatteryOnlyWhenRelevant",
        "taskbar-battery-relevant-on",
        "taskbar-battery-relevant-off",
        false,
    ),
    (
        "taskbarShowClock",
        "taskbar-clock-on",
        "taskbar-clock-off",
        true,
    ),
    (
        "taskbarShowWorkspaces",
        "taskbar-workspaces-on",
        "taskbar-workspaces-off",
        true,
    ),
    (
        "weatherEnabled",
        "weather-enabled-on",
        "weather-enabled-off",
        true,
    ),
    (
        "autoLocationEnabled",
        "weather-auto-location-on",
        "weather-auto-location-off",
        false,
    ),
    (
        "autoTimeZoneEnabled",
        "time-auto-zone-on",
        "time-auto-zone-off",
        false,
    ),
    ("titlebars", "titlebars-on", "titlebars-off", true),
    (
        "taskbarActivities",
        "taskbar-activities-on",
        "taskbar-activities-off",
        true,
    ),
    (
        "taskbarSeparate",
        "taskbar-separate-on",
        "taskbar-separate-off",
        true,
    ),
    (
        "assistant.enabled",
        "assistant-enabled-on",
        "assistant-enabled-off",
        true,
    ),
    (
        "assistant.taskbarIcon",
        "assistant-taskbar-icon-on",
        "assistant-taskbar-icon-off",
        true,
    ),
    (
        "assistant.speechToText",
        "assistant-speech-to-text-on",
        "assistant-speech-to-text-off",
        false,
    ),
    (
        "assistant.textToSpeech",
        "assistant-text-to-speech-on",
        "assistant-text-to-speech-off",
        false,
    ),
];
/// Inline status line for failed requests. Optional in the compiled markup.
pub(crate) const ERROR_NODE: &str = "settings-error";
/// Version line on the About page. Optional in the compiled markup.
pub(crate) const ABOUT_VERSION_NODE: &str = "about-version";
/// Output rows per page on the Displays page and the Taskbar output list. The
/// Taskbar rows are optional in the compiled markup.
pub(crate) const OUTPUT_PAGE_SLOTS: usize = 4;

/// Process-local monotonic milliseconds for the in-process `ControlTransport` path.
/// The D-Bus wire adapter replaces this value with the reactor's monotonic clock.
pub(crate) fn monotonic_now_ms() -> u64 {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

