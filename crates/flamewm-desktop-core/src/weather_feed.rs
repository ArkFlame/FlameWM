//! Desktop weather widget projection over the shell-published forecast feed.
//!
//! The shell owns fetching, refresh and the weather settings. The widget decodes the same
//! [`WeatherFeed`] payload the tray reads and projects it through the shared `weather_view`
//! rules, so temperature, units, status text and attribution cannot drift from the tray.

use flamewm_api::weather::WeatherFeedSnapshot;
use flamewm_shell_core::weather::{
    OPEN_METEO_ATTRIBUTION, WeatherCondition, WeatherFeed, WeatherState,
};
use flamewm_shell_core::weather_view::{
    condition_label, popup_view, range_label, tray_view, weekday_label,
};

/// Days shown in the widget strip (its fixed slot count).
pub const WEATHER_STRIP_DAYS: usize = 6;
/// Shown in place of the forecast when no feed was ever published.
pub const NO_DATA_TEXT: &str = "No data";
/// Location line when no location is configured and nothing else names the widget.
const DEFAULT_TITLE: &str = "Weather";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeatherStripDay {
    pub weekday: &'static str,
    /// Daily high in the feed's units, e.g. `68°`.
    pub high: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeatherWidgetView {
    pub location: String,
    pub temperature: String,
    pub condition: WeatherCondition,
    /// The popup status when one applies (off, choose a location, loading, offline...),
    /// otherwise the current condition label.
    pub condition_text: String,
    pub range: String,
    pub days: Vec<WeatherStripDay>,
    /// Open-Meteo attribution, present exactly when forecast data is shown.
    pub attribution: Option<&'static str>,
}

impl WeatherWidgetView {
    fn unavailable() -> Self {
        Self {
            location: DEFAULT_TITLE.to_owned(),
            temperature: "--".to_owned(),
            condition: WeatherCondition::Unknown,
            condition_text: NO_DATA_TEXT.to_owned(),
            range: "H:-- L:--".to_owned(),
            days: Vec::new(),
            attribution: None,
        }
    }
}

/// Projects the shell's feed. An empty or undecodable payload is the "no data" state.
#[must_use]
pub fn weather_widget_view(feed: &WeatherFeedSnapshot) -> WeatherWidgetView {
    let Some(decoded) = WeatherFeed::decode(&feed.payload) else {
        return WeatherWidgetView::unavailable();
    };
    let units = decoded.settings.units;
    let popup = popup_view(&decoded.state, &decoded.settings);
    let tray = tray_view(&decoded.state, units, true);
    let days = match &decoded.state {
        WeatherState::Ready(snapshot) | WeatherState::Offline(Some(snapshot)) => snapshot
            .days
            .iter()
            .take(WEATHER_STRIP_DAYS)
            .map(|day| WeatherStripDay {
                weekday: weekday_label(&day.date).unwrap_or("---"),
                high: format!("{}\u{b0}", units.convert_c(day.max_c)),
            })
            .collect(),
        _ => Vec::new(),
    };
    let has_data = matches!(
        decoded.state,
        WeatherState::Ready(_) | WeatherState::Offline(Some(_))
    );
    WeatherWidgetView {
        location: if popup.location.is_empty() {
            DEFAULT_TITLE.to_owned()
        } else {
            popup.location
        },
        temperature: tray.temperature,
        condition: popup.condition,
        condition_text: if popup.status.is_empty() {
            condition_label(popup.condition).to_owned()
        } else {
            popup.status
        },
        range: range_label(&decoded.state, units),
        days,
        attribution: has_data.then_some(OPEN_METEO_ATTRIBUTION),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_shell_core::weather::{
        DayForecast, OPEN_METEO_ATTRIBUTION, TemperatureUnit, WeatherLocation, WeatherSettings,
        WeatherSnapshot,
    };

    fn feed_of(
        enabled: bool,
        units: TemperatureUnit,
        state: WeatherState,
        named: bool,
    ) -> WeatherFeedSnapshot {
        let settings = WeatherSettings {
            enabled,
            location: named
                .then(|| WeatherLocation::new("Oslo", 59.9, 10.7, Some("Norway")).unwrap()),
            units,
        };
        let payload = WeatherFeed { settings, state }.encode();
        WeatherFeedSnapshot {
            revision: 1,
            payload,
        }
    }

    fn ready() -> WeatherState {
        WeatherState::Ready(WeatherSnapshot {
            location: WeatherLocation::new("Oslo", 59.9, 10.7, Some("Norway")).unwrap(),
            temperature_c: 20,
            condition: WeatherCondition::Clear,
            days: vec![
                DayForecast {
                    date: "2026-10-28".to_owned(),
                    condition: WeatherCondition::Clear,
                    min_c: 5,
                    max_c: 20,
                },
                DayForecast {
                    date: "2026-10-29".to_owned(),
                    condition: WeatherCondition::Rain,
                    min_c: 3,
                    max_c: 12,
                },
            ],
            fetched_at: 0,
        })
    }

    #[test]
    fn missing_or_garbage_payload_is_no_data() {
        for payload in ["", "garbage"] {
            let view = weather_widget_view(&WeatherFeedSnapshot {
                revision: 0,
                payload: payload.to_owned(),
            });
            assert_eq!(view.condition_text, NO_DATA_TEXT);
            assert_eq!(view.temperature, "--");
            assert!(view.attribution.is_none());
            assert!(view.days.is_empty());
        }
    }

    #[test]
    fn unset_location_and_disabled_use_the_tray_states() {
        let unset = weather_widget_view(&feed_of(
            true,
            TemperatureUnit::Celsius,
            WeatherState::Unconfigured,
            false,
        ));
        assert_eq!(unset.condition_text, "Choose a location");
        assert_eq!(unset.location, DEFAULT_TITLE);
        let off = weather_widget_view(&feed_of(
            false,
            TemperatureUnit::Celsius,
            WeatherState::Disabled,
            true,
        ));
        assert_eq!(off.condition_text, "Weather is off");
        assert_eq!(off.temperature, "--");
    }

    #[test]
    fn ready_feed_matches_tray_temperature_and_unit() {
        let feed = feed_of(true, TemperatureUnit::Fahrenheit, ready(), true);
        let view = weather_widget_view(&feed);
        let tray = tray_view(&ready(), TemperatureUnit::Fahrenheit, true);
        assert_eq!(view.temperature, tray.temperature);
        assert_eq!(view.temperature, "68\u{b0}");
        assert_eq!(view.location, "Oslo, Norway");
        assert_eq!(view.condition_text, "Clear");
        assert_eq!(view.range, "H:68\u{b0} L:41\u{b0}");
        assert_eq!(view.attribution, Some(OPEN_METEO_ATTRIBUTION));
    }

    #[test]
    fn strip_lists_daily_highs_in_feed_units() {
        let view = weather_widget_view(&feed_of(true, TemperatureUnit::Celsius, ready(), true));
        assert_eq!(view.days.len(), 2);
        assert_eq!(view.days[0].weekday, "Wed");
        assert_eq!(view.days[0].high, "20\u{b0}");
        assert_eq!(view.days[1].high, "12\u{b0}");
    }

    #[test]
    fn offline_keeps_last_data_and_says_offline() {
        let WeatherState::Ready(snapshot) = ready() else {
            unreachable!()
        };
        let view = weather_widget_view(&feed_of(
            true,
            TemperatureUnit::Celsius,
            WeatherState::Offline(Some(snapshot)),
            true,
        ));
        assert_eq!(view.condition_text, "Offline");
        assert_eq!(view.temperature, "20\u{b0}");
        assert_eq!(view.attribution, Some(OPEN_METEO_ATTRIBUTION));
    }

    #[test]
    fn loading_without_snapshot_has_no_forecast_rows() {
        let view = weather_widget_view(&feed_of(
            true,
            TemperatureUnit::Celsius,
            WeatherState::Loading,
            true,
        ));
        assert_eq!(view.condition_text, "Loading...");
        assert_eq!(view.range, "H:-- L:--");
        assert!(view.days.is_empty());
        assert!(view.attribution.is_none());
    }
}
