//! Split from `lib.rs` (`consts`); see crate docs.

use super::*;

pub(crate) const DONATE_URI: &str = "https://paypal.me/LinsaFTW";
pub(crate) const SOURCE_URI: &str = "https://github.com/ArkFlame/flamewm";
pub(crate) const WEBSITE_URI: &str = "https://wm.arkflame.com";

/// Canonical product version: the workspace version inherited by this crate.
pub(crate) const APP_VERSION: &str = env!("FLAMEWM_SETTINGS_VERSION");
/// Authoritative accent for the native `FlameWM` preset.
pub(crate) const DEFAULT_ACCENT: &str = flamewm_api::settings::DEFAULT_ACCENT;
/// Nodes whose compiled colour is the accent and must follow live accent changes.
/// Every `toggle-on` id in ui/settings.html must be listed here (checked by a test).
pub(crate) const ACCENT_NODE_IDS: &[&str] = &[
    "nav-selection-accent",
    "weather-tint-on",
    "usage-claude-on",
    "usage-command-code-on",
    "workspace-osd-on",
    "reduce-transparency-on",
    "anim-global-on",
    "a11y-high-contrast-on",
    "a11y-reduced-motion-on",
    "a11y-sticky-keys-on",
    "a11y-slow-keys-on",
    "a11y-shake-locate-on",
    "anim-start-on",
    "anim-window-on",
    "anim-minimize-on",
    "anim-maximize-on",
    "lock-date-on",
    "lock-username-on",
    "sticky-toggle-on",
    "downloads-organize-on",
    "focus-mouse-on",
    "titlebars-on",
    "outlines-on",
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
    "taskbar-desktop-button-on",
    "custom-window-outline-on",
    "surface-custom-on",
    "glass-custom-on",
    "cursor-custom-on",
    "minimize-maximize-custom-on",
    "lock-background-custom-on",
    "lock-clock-custom-on",
    "screensaver-custom-on",
    "windows-custom-on",
    "panel-custom-on",
    "items-custom-on",
    "fonts-custom-on",
    "taskbar-activities-on",
    "taskbar-separate-on",
    "taskbar-enabled-on",
    "dock-enabled-on",
    "dock-running-on",
    "dock-trash-on",
    "dock-hover-scale-on",
    "dock-covered-on",
    "dock-bounce-on",
    "dock-autohide-on",
    "dock-reserve-on",
    "dock-surface-custom-on",
    "assistant-enabled-on",
    "assistant-taskbar-icon-on",
    "assistant-speech-to-text-on",
    "assistant-text-to-speech-on",
    "weather-enabled-on",
    "weather-auto-location-on",
    "time-auto-zone-on",
    "font-toggle-on",
    "font-shadow-toggle-on",
    "wallpaper-share-on",
    "night-shift-on",
    "night-shift-schedule-on",
];
/// Boolean ON/OFF twins: (setting key, ON id, OFF id, default when the key is absent).
pub(crate) const SETTING_TWINS: &[(&str, &str, &str, bool)] = &[
    (
        "highContrast",
        "a11y-high-contrast-on",
        "a11y-high-contrast-off",
        false,
    ),
    (
        "reducedMotion",
        "a11y-reduced-motion-on",
        "a11y-reduced-motion-off",
        false,
    ),
    (
        "stickyKeys",
        "a11y-sticky-keys-on",
        "a11y-sticky-keys-off",
        false,
    ),
    ("slowKeys", "a11y-slow-keys-on", "a11y-slow-keys-off", false),
    (
        "shakeToLocate",
        "a11y-shake-locate-on",
        "a11y-shake-locate-off",
        false,
    ),
    (
        "shareWallpaper",
        "wallpaper-share-on",
        "wallpaper-share-off",
        false,
    ),
    (
        "surfaceCustom",
        "surface-custom-on",
        "surface-custom-off",
        false,
    ),
    ("glassCustom", "glass-custom-on", "glass-custom-off", false),
    (
        "cursorCustom",
        "cursor-custom-on",
        "cursor-custom-off",
        false,
    ),
    (
        "minimizeMaximizeCustom",
        "minimize-maximize-custom-on",
        "minimize-maximize-custom-off",
        false,
    ),
    (
        "lockBackgroundCustom",
        "lock-background-custom-on",
        "lock-background-custom-off",
        false,
    ),
    (
        "lockClockCustom",
        "lock-clock-custom-on",
        "lock-clock-custom-off",
        false,
    ),
    (
        "screensaverCustom",
        "screensaver-custom-on",
        "screensaver-custom-off",
        false,
    ),
    (
        "windowsCustom",
        "windows-custom-on",
        "windows-custom-off",
        false,
    ),
    ("panelCustom", "panel-custom-on", "panel-custom-off", false),
    ("itemsCustom", "items-custom-on", "items-custom-off", false),
    ("fontsCustom", "fonts-custom-on", "fonts-custom-off", false),
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
        "taskbarShowDesktopButton",
        "taskbar-desktop-button-on",
        "taskbar-desktop-button-off",
        true,
    ),
    (
        "weatherEnabled",
        "weather-enabled-on",
        "weather-enabled-off",
        true,
    ),
    ("weatherTint", "weather-tint-on", "weather-tint-off", true),
    ("usage.claude", "usage-claude-on", "usage-claude-off", true),
    (
        "usage.commandCode",
        "usage-command-code-on",
        "usage-command-code-off",
        true,
    ),
    (
        "workspaceOsd",
        "workspace-osd-on",
        "workspace-osd-off",
        false,
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
    ("outlines", "outlines-on", "outlines-off", true),
    (
        "customWindowOutline",
        "custom-window-outline-on",
        "custom-window-outline-off",
        false,
    ),
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
        false,
    ),
    (
        "taskbar.enabled",
        "taskbar-enabled-on",
        "taskbar-enabled-off",
        true,
    ),
    ("dock.enabled", "dock-enabled-on", "dock-enabled-off", false),
    (
        "dock.showRunning",
        "dock-running-on",
        "dock-running-off",
        true,
    ),
    ("dock.showTrash", "dock-trash-on", "dock-trash-off", true),
    (
        "dock.hoverScale",
        "dock-hover-scale-on",
        "dock-hover-scale-off",
        false,
    ),
    (
        "dock.autoHideCovered",
        "dock-covered-on",
        "dock-covered-off",
        false,
    ),
    (
        "dock.launchBounce",
        "dock-bounce-on",
        "dock-bounce-off",
        false,
    ),
    (
        "dock.autoHide",
        "dock-autohide-on",
        "dock-autohide-off",
        false,
    ),
    (
        "dock.reserveSpace",
        "dock-reserve-on",
        "dock-reserve-off",
        false,
    ),
    (
        "dock.surfaceCustom",
        "dock-surface-custom-on",
        "dock-surface-custom-off",
        false,
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
    ("nightShift", "night-shift-on", "night-shift-off", false),
    (
        "nightShiftSchedule",
        "night-shift-schedule-on",
        "night-shift-schedule-off",
        false,
    ),
];
/// Progressive-disclosure switch groups with a UI body: (group, switch key, body node id). The body
/// holds every member control and is hidden while its switch is off (`docs/SETTINGS_PROGRESSIVE_DISCLOSURE.md`).
pub(crate) const SWITCH_BODIES: &[(&str, &str, &str)] = &[
    ("cursor", "cursorCustom", "cursor-custom-body"),
    ("surface", "surfaceCustom", "surface-custom-body"),
    ("glass", "glassCustom", "glass-custom-body"),
    (
        "outline",
        "customWindowOutline",
        "custom-window-outline-body",
    ),
    (
        "minimizeMaximize",
        "minimizeMaximizeCustom",
        "minimize-maximize-custom-body",
    ),
    (
        "lockBackground",
        "lockBackgroundCustom",
        "lock-background-custom-body",
    ),
    ("lockClock", "lockClockCustom", "lock-clock-custom-body"),
    (
        "screensaver",
        "screensaverCustom",
        "screensaver-custom-body",
    ),
    ("windows", "windowsCustom", "windows-custom-body"),
    ("panel", "panelCustom", "panel-custom-body"),
    ("items", "itemsCustom", "items-custom-body"),
    ("fonts", "fontsCustom", "fonts-custom-body"),
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
