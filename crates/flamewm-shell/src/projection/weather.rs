//! Weather tray button and popup projection (A09 part 2).
//!
//! Pure views ([`tray_view`], [`popup_view`]) hold every rule; the projection
//! functions only write node visibility and text. Icons are chosen from
//! [`WeatherCondition::icon_name`], one node per condition.

use super::*;
use crate::weather_host::{LocationSearch, SearchStatus};
use flamewm_shell_core::weather::{TemperatureUnit, WeatherCondition, WeatherSettings, WeatherState};

/// Rows in the popup forecast list.
pub const WEATHER_DAY_ROWS: usize = 7;

/// Every condition, one icon node each (`tray-weather-<suffix>`, `weather-icon-<suffix>`).
pub const WEATHER_CONDITIONS: [WeatherCondition; 9] = [
    WeatherCondition::Clear,
    WeatherCondition::FewClouds,
    WeatherCondition::Overcast,
    WeatherCondition::Fog,
    WeatherCondition::Drizzle,
    WeatherCondition::Rain,
    WeatherCondition::Snow,
    WeatherCondition::Storm,
    WeatherCondition::Unknown,
];

/// Icon node suffix, e.g. `Clear` -> `clear`, `Unknown` -> `none-available`.
#[must_use]
pub fn icon_suffix(condition: WeatherCondition) -> &'static str {
    condition
        .icon_name()
        .strip_prefix("weather-")
        .unwrap_or("none-available")
}

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
    let mut parts = date.splitn(3, '-');
    let mut year: i32 = parts.next()?.parse().ok()?;
    let month: usize = parts.next()?.parse().ok()?;
    let day: i32 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&month) {
        return None;
    }
    const OFFSETS: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
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

/// Tray entry: visibility is the `taskbarShowWeather` setting alone, independent
/// of fetching. Neutral icon and `--` without data (including when disabled).
#[must_use]
pub fn tray_view(state: &WeatherState, units: TemperatureUnit, shown: bool) -> WeatherTrayView {
    match state {
        WeatherState::Ready(snapshot) | WeatherState::Offline(Some(snapshot)) => WeatherTrayView {
            visible: shown,
            condition: snapshot.condition,
            temperature: format!("{}\u{b0}", units.convert_c(snapshot.temperature_c)),
        },
        _ => WeatherTrayView {
            visible: shown,
            condition: WeatherCondition::Unknown,
            temperature: "--".to_owned(),
        },
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
            location: match s.location.country.as_deref() {
                Some(country) => format!("{}, {country}", s.location.name),
                None => s.location.name.clone(),
            },
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

/// Taskbar tray entry: visibility, one condition icon, and the temperature text under it.
pub fn project_weather_tray(
    document: &mut impl UiDocumentAccess,
    state: &WeatherState,
    units: TemperatureUnit,
    shown: bool,
) -> Result<(), String> {
    let view = tray_view(state, units, shown);
    document.visible("tray-weather", view.visible)?;
    for condition in WEATHER_CONDITIONS {
        document.visible(
            &format!("tray-weather-{}", icon_suffix(condition)),
            view.visible && condition == view.condition,
        )?;
    }
    document.text("tray-weather-temp", view.temperature)
}

/// Location picker rows shown in place of the forecast rows.
pub const WEATHER_RESULT_ROWS: usize = 5;

/// A snapshot shown from cache while the last refresh failed is dimmed.
#[must_use]
pub fn offline_dimmed(state: &WeatherState) -> bool {
    matches!(state, WeatherState::Offline(Some(_)))
}

/// The picker replaces the forecast rows while it has results or a status to show.
#[must_use]
pub fn picker_open(search: &LocationSearch) -> bool {
    !search.results.is_empty() || search.status != SearchStatus::Idle
}

fn search_status_text(status: SearchStatus) -> &'static str {
    match status {
        SearchStatus::Idle => "",
        SearchStatus::Searching => "Searching...",
        SearchStatus::NoMatches => "No matches",
        SearchStatus::Failed => "Search failed",
    }
}

/// Weather popup body for every state, with the Open-Meteo attribution.
/// `veil` is the surface colour used to dim an offline snapshot.
pub fn project_weather_popup(
    document: &mut impl UiDocumentAccess,
    state: &WeatherState,
    settings: &WeatherSettings,
    search: &LocationSearch,
    veil: UiColor,
) -> Result<(), String> {
    let view = popup_view(state, settings);
    let picker = picker_open(search);
    document.text("weather-location", view.location)?;
    document.text("weather-temp", view.temperature)?;
    document.text("weather-condition", condition_label(view.condition).to_owned())?;
    document.text("weather-state", view.status)?;
    for condition in WEATHER_CONDITIONS {
        document.visible(
            &format!("weather-icon-{}", icon_suffix(condition)),
            condition == view.condition,
        )?;
    }
    for row in 1..=WEATHER_DAY_ROWS {
        let day = view.days.get(row - 1);
        let shown = day.is_some() && !picker;
        document.text(
            &format!("weather-day-{row}-name"),
            day.map_or_else(String::new, |d| d.weekday.to_owned()),
        )?;
        document.visible(&format!("weather-day-{row}-name"), shown)?;
        document.text(
            &format!("weather-day-{row}-range"),
            day.map_or_else(String::new, |d| d.range.clone()),
        )?;
        document.visible(&format!("weather-day-{row}-range"), shown)?;
        for condition in WEATHER_CONDITIONS {
            document.visible(
                &format!("weather-day-{row}-icon-{}", icon_suffix(condition)),
                shown && day.is_some_and(|d| d.condition == condition),
            )?;
        }
    }
    document.visible("weather-offline-veil", offline_dimmed(state))?;
    document.background(
        "weather-offline-veil",
        UiColor { a: 160, ..veil },
    )?;
    document.text("weather-search-value", search.query.clone())?;
    document.visible("weather-search-placeholder", search.query.is_empty())?;
    for row in 1..=WEATHER_RESULT_ROWS {
        let location = search.results.get(row - 1).filter(|_| picker);
        document.text(
            &format!("weather-result-{row}"),
            location.map_or_else(String::new, |l| match l.country.as_deref() {
                Some(country) => format!("{}, {country}", l.name),
                None => l.name.clone(),
            }),
        )?;
        document.visible(&format!("weather-result-{row}"), location.is_some())?;
    }
    let status = if picker && search.results.is_empty() {
        search_status_text(search.status)
    } else {
        ""
    };
    document.text("weather-search-status", status.to_owned())?;
    document.visible("weather-search-status", !status.is_empty())?;
    document.text(
        "weather-attribution",
        crate::weather_provider::OPEN_METEO_ATTRIBUTION.to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_shell_core::weather::{
        DayForecast, WeatherError, WeatherLocation, WeatherSnapshot,
    };

    fn berlin() -> WeatherLocation {
        WeatherLocation::new("Berlin", 52.52, 13.41, Some("Germany")).unwrap()
    }

    fn settings(units: TemperatureUnit) -> WeatherSettings {
        WeatherSettings {
            enabled: true,
            location: Some(berlin()),
            units,
        }
    }

    fn snapshot(days: usize) -> WeatherSnapshot {
        WeatherSnapshot {
            location: berlin(),
            temperature_c: 12,
            condition: WeatherCondition::Overcast,
            days: (0..days)
                .map(|i| DayForecast {
                    date: format!("2026-10-{:02}", 16 + i),
                    condition: WeatherCondition::Rain,
                    min_c: 3,
                    max_c: 14,
                })
                .collect(),
            fetched_at: 1,
        }
    }

    use flamewm_ui_x11::test_support::RecordingUiDoc as Recorder;

    #[test]
    fn every_condition_icon_resolves_to_a_breeze_asset() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../ui/shell/assets/web/breeze");
        for condition in WEATHER_CONDITIONS {
            let file = dir.join(format!("{}.svg", condition.icon_name()));
            assert!(file.is_file(), "missing icon asset {}", file.display());
        }
    }

    #[test]
    fn tray_visibility_follows_setting_not_fetch_state() {
        for state in [
            WeatherState::Disabled,
            WeatherState::Unconfigured,
            WeatherState::Loading,
            WeatherState::Offline(None),
            WeatherState::Error(WeatherError::Transport),
        ] {
            let shown = tray_view(&state, TemperatureUnit::Celsius, true);
            assert!(shown.visible, "{state:?}");
            assert_eq!(shown.temperature, "--");
            assert_eq!(shown.condition, WeatherCondition::Unknown);
            assert!(!tray_view(&state, TemperatureUnit::Celsius, false).visible, "{state:?}");
        }
    }

    #[test]
    fn tray_shows_integer_temperature_for_ready_and_offline_snapshots() {
        let ready = WeatherState::Ready(snapshot(0));
        assert_eq!(tray_view(&ready, TemperatureUnit::Celsius, true).temperature, "12\u{b0}");
        assert_eq!(tray_view(&ready, TemperatureUnit::Fahrenheit, true).temperature, "54\u{b0}");
        let offline = WeatherState::Offline(Some(snapshot(0)));
        assert_eq!(
            tray_view(&offline, TemperatureUnit::Celsius, true).condition,
            WeatherCondition::Overcast
        );
    }

    #[test]
    fn tray_projection_places_temperature_and_one_icon() {
        let mut doc = Recorder::default();
        project_weather_tray(
            &mut doc,
            &WeatherState::Ready(snapshot(0)),
            TemperatureUnit::Celsius,
            true,
        )
        .unwrap();
        assert_eq!(doc.texts["tray-weather-temp"], "12\u{b0}");
        assert!(doc.visible["tray-weather"]);
        assert!(doc.visible["tray-weather-overcast"]);
        assert!(!doc.visible["tray-weather-clear"]);

        let mut disabled = Recorder::default();
        project_weather_tray(&mut disabled, &WeatherState::Disabled, TemperatureUnit::Celsius, true)
            .unwrap();
        assert!(disabled.visible["tray-weather"]);
        assert!(disabled.visible["tray-weather-none-available"]);
        assert_eq!(disabled.texts["tray-weather-temp"], "--");

        let mut off = Recorder::default();
        project_weather_tray(&mut off, &WeatherState::Disabled, TemperatureUnit::Celsius, false)
            .unwrap();
        assert!(!off.visible["tray-weather"]);
        assert!(!off.visible["tray-weather-overcast"]);
    }

    #[test]
    fn popup_states_are_explicit_and_never_invent_values() {
        let s = settings(TemperatureUnit::Celsius);
        let cases = [
            (WeatherState::Disabled, "Weather is off"),
            (WeatherState::Unconfigured, "Choose a location"),
            (WeatherState::Loading, "Loading..."),
            (WeatherState::Offline(None), "Offline"),
            (WeatherState::Error(WeatherError::Parse("x")), "Could not load weather"),
        ];
        for (state, status) in cases {
            let view = popup_view(&state, &s);
            assert_eq!(view.status, status, "{state:?}");
            assert_eq!(view.temperature, "--", "{state:?}");
            assert!(view.days.is_empty(), "{state:?}");
        }
        let ready = popup_view(&WeatherState::Ready(snapshot(3)), &s);
        assert_eq!(ready.status, "");
        assert_eq!(ready.location, "Berlin, Germany");
        assert_eq!(ready.days.len(), 3);
        let offline = popup_view(&WeatherState::Offline(Some(snapshot(1))), &s);
        assert_eq!(offline.status, "Offline");
        assert_eq!(offline.temperature, "12\u{b0}");
    }

    #[test]
    fn popup_caps_forecast_rows_at_seven_and_formats_ranges() {
        let view = popup_view(
            &WeatherState::Ready(snapshot(9)),
            &settings(TemperatureUnit::Fahrenheit),
        );
        assert_eq!(view.days.len(), WEATHER_DAY_ROWS);
        assert_eq!(view.days[0].range, "37 / 57");
        assert_eq!(view.days[0].weekday, "Fri");
    }

    #[test]
    fn popup_projection_hides_unused_rows_and_shows_attribution() {
        let mut doc = Recorder::default();
        project_weather_popup(
            &mut doc,
            &WeatherState::Ready(snapshot(2)),
            &settings(TemperatureUnit::Celsius),
            &LocationSearch::default(),
            UiColor { r: 0, g: 0, b: 0, a: 255 },
        )
        .unwrap();
        assert_eq!(doc.texts["weather-day-1-name"], "Fri");
        assert!(doc.visible["weather-day-1-icon-showers"]);
        assert_eq!(doc.texts["weather-day-3-name"], "");
        assert!(!doc.visible["weather-day-3-icon-showers"]);
        assert_eq!(
            doc.texts["weather-attribution"],
            crate::weather_provider::OPEN_METEO_ATTRIBUTION
        );
        assert_eq!(doc.texts["weather-state"], "");
    }

    #[test]
    fn picker_replaces_forecast_rows_and_offline_snapshot_is_veiled() {
        let mut search = LocationSearch::default();
        search.results = vec![berlin()];
        let veil = UiColor { r: 1, g: 2, b: 3, a: 255 };
        let mut doc = Recorder::default();
        project_weather_popup(
            &mut doc,
            &WeatherState::Offline(Some(snapshot(2))),
            &settings(TemperatureUnit::Celsius),
            &search,
            veil,
        )
        .unwrap();
        assert!(!doc.visible["weather-day-1-name"]);
        assert_eq!(doc.texts["weather-result-1"], "Berlin, Germany");
        assert!(doc.visible["weather-result-1"]);
        assert!(!doc.visible["weather-result-2"]);
        assert!(doc.visible["weather-offline-veil"]);
        assert_eq!(doc.backgrounds["weather-offline-veil"].a, 160);
        assert!(!doc.visible["weather-search-status"]);

        let mut searching = LocationSearch::default();
        searching.status = SearchStatus::Searching;
        let mut busy = Recorder::default();
        project_weather_popup(
            &mut busy,
            &WeatherState::Ready(snapshot(2)),
            &settings(TemperatureUnit::Celsius),
            &searching,
            veil,
        )
        .unwrap();
        assert_eq!(busy.texts["weather-search-status"], "Searching...");
        assert!(busy.visible["weather-search-status"]);
        assert!(!busy.visible["weather-offline-veil"]);
        assert!(!busy.visible["weather-day-1-name"]);
    }

    #[test]
    fn weekday_matches_known_dates() {
        assert_eq!(weekday_label("2026-10-16"), Some("Fri"));
        assert_eq!(weekday_label("2000-02-29"), Some("Tue"));
        assert_eq!(weekday_label("2026-13-01"), None);
        assert_eq!(weekday_label("nope"), None);
    }
}

#[cfg(test)]
mod panel_tray_tests {
    use super::*;
    use flamewm_shell_core::weather::{WeatherLocation, WeatherSnapshot};
    use flamewm_ui_x11::test_support::RecordingUiDoc;

    const PANEL_ARTIFACT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-panel.rwr"));

    fn ready_21c() -> WeatherState {
        WeatherState::Ready(WeatherSnapshot {
            location: WeatherLocation::new("Berlin", 52.52, 13.41, Some("Germany")).unwrap(),
            temperature_c: 21,
            condition: WeatherCondition::Clear,
            days: Vec::new(),
            fetched_at: 1,
        })
    }

    // REGRESSION: the tray temperature text must be written into the compiled
    // panel template and lay out with a non-zero rect inside the 30x44 tray button.
    // Viewport 1080x72 is the live 125% panel CSS viewport.
    // OBSERVABLE: text '--' without data, '21°' with data; span rect > 0 and inside the button.
    // MUTATION: drop the tray-weather-temp node from panel.html and the write is refused.
    #[test]
    fn tray_temperature_text_lays_out_inside_button_in_real_panel() {
        let cases = [
            (WeatherState::Disabled, "--"),
            (WeatherState::Unconfigured, "--"),
            (ready_21c(), "21\u{b0}"),
        ];
        for (state, expected) in cases {
            let mut recorder = RecordingUiDoc::default();
            project_weather_tray(&mut recorder, &state, TemperatureUnit::Celsius, true).unwrap();
            assert_eq!(recorder.texts["tray-weather-temp"], expected);

            let mut doc = flamewm_ui_x11::decode_document(PANEL_ARTIFACT).expect("panel decodes");
            project_weather_tray(&mut doc, &state, TemperatureUnit::Celsius, true)
                .unwrap_or_else(|error| panic!("tray projection refused: {error}"));
            let viewport = (1080.0, 72.0);
            let button = doc.layout_box_by_id("tray-weather", viewport).expect("button");
            let span = doc.layout_box_by_id("tray-weather-temp", viewport).expect("span");
            assert!(span.2 > 0.0 && span.3 > 0.0, "zero-size temp span {span:?}");
            let inside = span.0 >= button.0
                && span.1 >= button.1
                && span.0 + span.2 <= button.0 + button.2 + 0.5
                && span.1 + span.3 <= button.1 + button.3 + 0.5;
            assert!(inside, "temp span {span:?} outside button {button:?}");
        }
    }
}
