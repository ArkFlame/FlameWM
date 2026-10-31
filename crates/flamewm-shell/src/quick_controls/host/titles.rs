/// `WM_NAME` of each helper-hosted quick popup, and of the main-hosted media popup.
pub const AUDIO_POPUP_TITLE: &str = "FlameWM audio";

pub const NETWORK_POPUP_TITLE: &str = "FlameWM network";

pub const CALENDAR_POPUP_TITLE: &str = "FlameWM calendar";

pub const BATTERY_POPUP_TITLE: &str = "FlameWM battery";

pub const WEATHER_POPUP_TITLE: &str = "FlameWM weather";

pub const MEDIA_POPUP_TITLE: &str = "FlameWM media";

/// Every quick-popup title. A press whose top-level window carries one of these
/// titles lands inside a popup and is not an outside dismissal.
pub const QUICK_POPUP_TITLES: [&str; 6] = [
    AUDIO_POPUP_TITLE,
    NETWORK_POPUP_TITLE,
    CALENDAR_POPUP_TITLE,
    BATTERY_POPUP_TITLE,
    WEATHER_POPUP_TITLE,
    MEDIA_POPUP_TITLE,
];

/// True when `title` names a quick popup window.
#[must_use]
pub fn is_quick_popup_title(title: &str) -> bool {
    QUICK_POPUP_TITLES.contains(&title)
}
