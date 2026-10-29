//! Shared weather presentation rules: the taskbar tray, the quick-control popup and the
//! desktop widget all read these pure views, so their text and states cannot drift.
//! No UI types here; projections only write the strings and visibility these return.

use crate::weather::{TemperatureUnit, WeatherCondition, WeatherSettings, WeatherState};

/// Rows in the popup forecast list.
pub const WEATHER_DAY_ROWS: usize = 7;

#[must_use]
pub fn condition_label(condition: WeatherCondition) -> &'static str {
    match condition {
        WeatherCondition::Clear => "Clear",
        WeatherCondition::FewClouds => "Partly cloudy",
        WeatherCondition::Overcast => "Overcast",
        WeatherCondition::Fog => "Fog",
        WeatherCondition::Drizzle => "Drizzle",
        WeatherCondition::Rain => "Rain",
        WeatherCondition::Snow => "Snow",
        WeatherCondition::Storm => "Storm",
        WeatherCondition::Unknown => "Unknown",
    }
}

/// Weekday abbreviation for an ISO `YYYY-MM-DD` date (Sakamoto's method).
#[must_use]
pub fn weekday_label(date: &str) -> Option<&'static str> {
    const OFFSETS: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let mut parts = date.splitn(3, '-');
    let mut year: i32 = parts.next()?.parse().ok()?;
    let month: usize = parts.next()?.parse().ok()?;
    let day: i32 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&month) {
        return None;
    }
    if month < 3 {
        year -= 1;
    }
    let index =
        (year + year / 4 - year / 100 + year / 400 + OFFSETS[month - 1] + day).rem_euclid(7);
    Some(["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][index as usize])
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeatherTrayView {
    pub visible: bool,
    pub condition: WeatherCondition,
    pub temperature: String,
}

/// Tray entry: shown when `taskbarShowWeather` is on and weather is enabled. Enabled
/// without a location shows the neutral icon and `--` (the popup offers the picker).
#[must_use]
pub fn tray_view(state: &WeatherState, units: TemperatureUnit, shown: bool) -> WeatherTrayView {
    let visible = shown && !matches!(state, WeatherState::Disabled);
    match state {
        WeatherState::Ready(snapshot) | WeatherState::Offline(Some(snapshot)) => WeatherTrayView {
            visible,
            condition: snapshot.condition,
            temperature: format!("{}\u{b0}", units.convert_c(snapshot.temperature_c)),
        },
        _ => WeatherTrayView {
            visible,
            condition: WeatherCondition::Unknown,
            temperature: "--".to_owned(),
        },
    }
}

/// Today's high and low in `units`, or `H:-- L:--` when no forecast is held.
#[must_use]
pub fn range_label(state: &WeatherState, units: TemperatureUnit) -> String {
    match state {
        WeatherState::Ready(snapshot) | WeatherState::Offline(Some(snapshot)) => {
            snapshot.days.first().map_or_else(
                || "H:-- L:--".to_owned(),
                |day| {
                    format!(
                        "H:{}\u{b0} L:{}\u{b0}",
                        units.convert_c(day.max_c),
                        units.convert_c(day.min_c)
                    )
                },
            )
        }
        _ => "H:-- L:--".to_owned(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeatherDayRow {
    pub weekday: &'static str,
    pub condition: WeatherCondition,
    pub range: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeatherPopupView {
    pub location: String,
    pub condition: WeatherCondition,
    pub temperature: String,
    /// Empty when data is shown; otherwise the explicit state line.
    pub status: String,
    pub days: Vec<WeatherDayRow>,
}

/// Popup content for every state. Data is never invented: without a snapshot the
/// temperature is `--` and no forecast rows are shown.
#[must_use]
pub fn popup_view(state: &WeatherState, settings: &WeatherSettings) -> WeatherPopupView {
    let configured_name = settings
        .location
        .as_ref()
        .map_or_else(String::new, |l| l.name.clone());
    let (snapshot, status) = match state {
        WeatherState::Disabled => (None, "Weather is off"),
        WeatherState::Unconfigured => (None, "Choose a location"),
        WeatherState::Loading => (None, "Loading..."),
        WeatherState::Ready(s) => (Some(s), ""),
        WeatherState::Offline(Some(s)) => (Some(s), "Offline"),
        WeatherState::Offline(None) => (None, "Offline"),
        WeatherState::Error(_) => (None, "Could not load weather"),
    };
    match snapshot {
        Some(s) => WeatherPopupView {
            location: place_name(&s.location.name, s.location.country.as_deref()),
            condition: s.condition,
            temperature: format!("{}\u{b0}", settings.units.convert_c(s.temperature_c)),
            status: status.to_owned(),
            days: s
                .days
                .iter()
                .take(WEATHER_DAY_ROWS)
                .map(|day| WeatherDayRow {
                    weekday: weekday_label(&day.date).unwrap_or("---"),
                    condition: day.condition,
                    range: format!(
                        "{} / {}",
                        settings.units.convert_c(day.min_c),
                        settings.units.convert_c(day.max_c)
                    ),
                })
                .collect(),
        },
        None => WeatherPopupView {
            location: configured_name,
            condition: WeatherCondition::Unknown,
            temperature: "--".to_owned(),
            status: status.to_owned(),
            days: Vec::new(),
        },
    }
}

/// `Name, Country`, or just the name when the country is unknown.
#[must_use]
pub fn place_name(name: &str, country: Option<&str>) -> String {
    country.map_or_else(|| name.to_owned(), |country| format!("{name}, {country}"))
}
