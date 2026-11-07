//! Taskbar clock and task-item projection.

use super::*;

/// Accent carries `accentOpacity` as alpha for every accent consumer.
/// Clock presentation keys; unknown or missing values fall back to the product defaults.
#[must_use]
pub fn clock_display(snapshot: &SettingsSnapshot) -> ClockDisplay {
    let defaults = ClockDisplay::default();
    let format = ClockFormat::parse(&text_setting(snapshot, "taskbarClockFormat", "24h"))
        .unwrap_or(defaults.format);
    ClockDisplay {
        format,
        show_seconds: bool_setting(snapshot, "taskbarShowSeconds", defaults.show_seconds),
        show_date: bool_setting(snapshot, "taskbarShowDate", defaults.show_date),
    }
}

/// Taskbar item visibility keys; a missing snapshot or key means its default.
#[must_use]
pub fn taskbar_items(snapshot: Option<&SettingsSnapshot>) -> TaskbarItems {
    let defaults = TaskbarItems::default();
    let Some(snapshot) = snapshot else {
        return defaults;
    };
    TaskbarItems {
        weather: bool_setting(snapshot, "taskbarShowWeather", defaults.weather),
        volume: bool_setting(snapshot, "taskbarShowVolume", defaults.volume),
        network: bool_setting(snapshot, "taskbarShowNetwork", defaults.network),
        assistant: bool_setting(snapshot, "assistant.enabled", defaults.assistant)
            && bool_setting(snapshot, "assistant.taskbarIcon", defaults.assistant),
        battery: bool_setting(snapshot, "taskbarShowBattery", defaults.battery),
        battery_only_when_relevant: bool_setting(
            snapshot,
            "taskbarBatteryOnlyWhenRelevant",
            defaults.battery_only_when_relevant,
        ),
        clock: bool_setting(snapshot, "taskbarShowClock", defaults.clock),
        workspaces: bool_setting(snapshot, "taskbarShowWorkspaces", defaults.workspaces),
        desktop_button: bool_setting(
            snapshot,
            "taskbarShowDesktopButton",
            defaults.desktop_button,
        ),
        activities: bool_setting(snapshot, "taskbarActivities", defaults.activities),
        separate: bool_setting(snapshot, "taskbarSeparate", defaults.separate),
        blur_percent: blur_px_to_percent(dock_material(snapshot).blur_px),
    }
}
