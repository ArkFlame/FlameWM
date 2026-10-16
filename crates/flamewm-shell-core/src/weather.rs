//! Taskbar weather data layer (A09 part 1). View-independent: no UI here.
//!
//! Providers implement [`WeatherProvider`]. The Open-Meteo provider lives in
//! flamewm-shell (weather_provider.rs). LICENSING: Open-Meteo's free API is for noncommercial use
//! only and its data requires attribution ([`OPEN_METEO_ATTRIBUTION`]). Check
//! the current terms before shipping commercially, and show the attribution
//! wherever forecast data is displayed.
//!
//! Network access is isolated behind [`HttpGet`]; no network code lives here.
//!
//! No timers or polling live here. The caller decides when to refresh, and
//! must not refresh while [`WeatherSettings::enabled`] is false.

use flamewm_api::settings::{SettingValue, SettingsChange, SettingsSnapshot};

const NAME_MAX_CHARS: usize = 80;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeatherError {
    InvalidLocation(&'static str),
    InvalidQuery,
    Transport,
    TooLarge,
    Parse(&'static str),
}

#[derive(Debug, Clone, PartialEq)]
pub struct WeatherLocation {
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
    pub country: Option<String>,
}

impl WeatherLocation {
    pub fn new(
        name: &str,
        latitude: f64,
        longitude: f64,
        country: Option<&str>,
    ) -> Result<Self, WeatherError> {
        if !(-90.0..=90.0).contains(&latitude) {
            return Err(WeatherError::InvalidLocation("latitude"));
        }
        if !(-180.0..=180.0).contains(&longitude) {
            return Err(WeatherError::InvalidLocation("longitude"));
        }
        let country = country.map(|c| clean_label(c, "country")).transpose()?;
        Ok(Self {
            name: clean_label(name, "name")?,
            latitude,
            longitude,
            country,
        })
    }
}

fn clean_label(raw: &str, what: &'static str) -> Result<String, WeatherError> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.chars().count() > NAME_MAX_CHARS
        || trimmed.chars().any(char::is_control)
    {
        return Err(WeatherError::InvalidLocation(what));
    }
    Ok(trimmed.to_owned())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeatherCondition {
    Clear,
    FewClouds,
    Overcast,
    Fog,
    Drizzle,
    Rain,
    Snow,
    Storm,
    Unknown,
}

impl WeatherCondition {
    /// WMO weather interpretation codes, as used by Open-Meteo.
    #[must_use]
    pub fn from_wmo_code(code: u8) -> Self {
        match code {
            0 => Self::Clear,
            1 | 2 => Self::FewClouds,
            3 => Self::Overcast,
            45 | 48 => Self::Fog,
            51 | 53 | 55 | 56 | 57 => Self::Drizzle,
            61 | 63 | 65 | 66 | 67 | 80 | 81 | 82 => Self::Rain,
            71 | 73 | 75 | 77 | 85 | 86 => Self::Snow,
            95 | 96 | 99 => Self::Storm,
            _ => Self::Unknown,
        }
    }

    #[must_use]
    pub fn icon_name(self) -> &'static str {
        match self {
            Self::Clear => "weather-clear",
            Self::FewClouds => "weather-few-clouds",
            Self::Overcast => "weather-overcast",
            Self::Fog => "weather-fog",
            Self::Drizzle => "weather-showers-scattered",
            Self::Rain => "weather-showers",
            Self::Snow => "weather-snow",
            Self::Storm => "weather-storm",
            Self::Unknown => "weather-none-available",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DayForecast {
    /// ISO date, YYYY-MM-DD, in the location's local time zone.
    pub date: String,
    pub condition: WeatherCondition,
    pub min_c: i32,
    pub max_c: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WeatherSnapshot {
    pub location: WeatherLocation,
    /// Whole degrees Celsius; convert with [`TemperatureUnit::convert_c`].
    pub temperature_c: i32,
    pub condition: WeatherCondition,
    pub days: Vec<DayForecast>,
    /// Unix seconds when the data was fetched.
    pub fetched_at: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WeatherState {
    Disabled,
    Unconfigured,
    Loading,
    Ready(WeatherSnapshot),
    Offline(Option<WeatherSnapshot>),
    Error(WeatherError),
}

impl WeatherState {
    /// Transition at the start of a caller-driven refresh.
    #[must_use]
    pub fn begin_refresh(self, settings: &WeatherSettings) -> Self {
        match settings {
            _ if !settings.enabled => Self::Disabled,
            _ if settings.location.is_none() => Self::Unconfigured,
            _ => match self {
                Self::Ready(_) | Self::Offline(Some(_)) => self,
                _ => Self::Loading,
            },
        }
    }

    /// Transition when a fetch completes. A failure keeps the last good
    /// snapshot as `Offline`; with no snapshot it becomes `Error`.
    #[must_use]
    pub fn finish_refresh(self, result: Result<WeatherSnapshot, WeatherError>) -> Self {
        let last = match self {
            Self::Ready(s) | Self::Offline(Some(s)) => Some(s),
            _ => None,
        };
        match (result, last) {
            (Ok(snapshot), _) => Self::Ready(snapshot),
            (Err(_), Some(last)) => Self::Offline(Some(last)),
            (Err(error), None) => Self::Error(error),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TemperatureUnit {
    #[default]
    Celsius,
    Fahrenheit,
}

impl TemperatureUnit {
    #[must_use]
    pub fn convert_c(self, celsius: i32) -> i32 {
        match self {
            Self::Celsius => celsius,
            Self::Fahrenheit => (f64::from(celsius) * 9.0 / 5.0 + 32.0).round() as i32,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct WeatherSettings {
    pub enabled: bool,
    pub location: Option<WeatherLocation>,
    pub units: TemperatureUnit,
}

impl WeatherSettings {
    pub const SETTINGS_KEY: &str = "weatherEnabled";
    pub const LOCATION_KEY: &str = "weatherLocation";
    pub const UNITS_KEY: &str = "weatherUnits";

    #[must_use]
    pub fn from_snapshot(snapshot: &SettingsSnapshot) -> Self {
        Self {
            enabled: matches!(
                snapshot.values.get(Self::SETTINGS_KEY),
                Some(SettingValue::Boolean(true))
            ),
            location: stored_text(snapshot, Self::LOCATION_KEY).and_then(decode_location),
            units: match stored_text(snapshot, Self::UNITS_KEY) {
                Some("fahrenheit") => TemperatureUnit::Fahrenheit,
                _ => TemperatureUnit::Celsius,
            },
        }
    }

    /// Changes to persist through the settings transaction. A `None` location
    /// clears the stored key.
    #[must_use]
    pub fn to_changes(&self) -> Vec<SettingsChange> {
        let units = match self.units {
            TemperatureUnit::Celsius => "celsius",
            TemperatureUnit::Fahrenheit => "fahrenheit",
        };
        let location = self
            .location
            .as_ref()
            .map(|l| SettingValue::Text(encode_location(l)));
        [
            (
                Self::SETTINGS_KEY,
                Some(SettingValue::Boolean(self.enabled)),
            ),
            (Self::LOCATION_KEY, location),
            (Self::UNITS_KEY, Some(SettingValue::Text(units.to_owned()))),
        ]
        .into_iter()
        .map(|(key, value)| SettingsChange {
            key: key.to_owned(),
            value,
        })
        .collect()
    }
}

fn stored_text<'a>(snapshot: &'a SettingsSnapshot, key: &str) -> Option<&'a str> {
    match snapshot.values.get(key) {
        Some(SettingValue::Text(text)) => Some(text.as_str()),
        _ => None,
    }
}

/// Encoded as `latitude<TAB>longitude<TAB>country<TAB>name`. Names and
/// countries never contain control characters, so tabs cannot collide.
fn encode_location(l: &WeatherLocation) -> String {
    let country = l.country.as_deref().unwrap_or("");
    format!("{}\t{}\t{country}\t{}", l.latitude, l.longitude, l.name)
}

fn decode_location(text: &str) -> Option<WeatherLocation> {
    let mut parts = text.splitn(4, '\t');
    let latitude = parts.next()?.parse::<f64>().ok()?;
    let longitude = parts.next()?.parse::<f64>().ok()?;
    let country = parts.next().filter(|c| !c.is_empty());
    WeatherLocation::new(parts.next()?, latitude, longitude, country).ok()
}

pub trait HttpGet {
    fn get(&self, url: &str) -> Result<String, WeatherError>;
}

pub trait WeatherProvider {
    fn forecast(&self, location: &WeatherLocation) -> Result<WeatherSnapshot, WeatherError>;
    fn search(&self, query: &str) -> Result<Vec<WeatherLocation>, WeatherError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn berlin() -> WeatherLocation {
        WeatherLocation::new("Berlin", 52.52, 13.41, Some("Germany")).unwrap()
    }

    fn snapshot() -> WeatherSnapshot {
        WeatherSnapshot {
            location: berlin(),
            temperature_c: 12,
            condition: WeatherCondition::Overcast,
            days: Vec::new(),
            fetched_at: 1,
        }
    }

    fn apply(changes: Vec<SettingsChange>) -> SettingsSnapshot {
        let mut snapshot = SettingsSnapshot::new(1);
        for change in changes {
            match change.value {
                Some(value) => {
                    snapshot.values.insert(change.key, value);
                }
                None => {
                    snapshot.values.remove(&change.key);
                }
            }
        }
        snapshot
    }

    #[test]
    fn wmo_table_maps_codes_to_conditions_and_icons() {
        let table: &[(u8, WeatherCondition, &str)] = &[
            (0, WeatherCondition::Clear, "weather-clear"),
            (1, WeatherCondition::FewClouds, "weather-few-clouds"),
            (2, WeatherCondition::FewClouds, "weather-few-clouds"),
            (3, WeatherCondition::Overcast, "weather-overcast"),
            (45, WeatherCondition::Fog, "weather-fog"),
            (48, WeatherCondition::Fog, "weather-fog"),
            (53, WeatherCondition::Drizzle, "weather-showers-scattered"),
            (61, WeatherCondition::Rain, "weather-showers"),
            (82, WeatherCondition::Rain, "weather-showers"),
            (73, WeatherCondition::Snow, "weather-snow"),
            (86, WeatherCondition::Snow, "weather-snow"),
            (95, WeatherCondition::Storm, "weather-storm"),
            (99, WeatherCondition::Storm, "weather-storm"),
            (4, WeatherCondition::Unknown, "weather-none-available"),
            (98, WeatherCondition::Unknown, "weather-none-available"),
        ];
        for (code, condition, icon) in table {
            assert_eq!(
                WeatherCondition::from_wmo_code(*code),
                *condition,
                "code {code}"
            );
            assert_eq!(condition.icon_name(), *icon, "code {code}");
        }
    }

    #[test]
    fn location_validation_bounds() {
        assert!(WeatherLocation::new("Berlin", 90.0, -180.0, None).is_ok());
        assert!(WeatherLocation::new("  Berlin  ", -90.0, 180.0, Some("DE")).is_ok());
        assert_eq!(
            WeatherLocation::new("Berlin", 90.5, 0.0, None),
            Err(WeatherError::InvalidLocation("latitude"))
        );
        assert_eq!(
            WeatherLocation::new("Berlin", f64::NAN, 0.0, None),
            Err(WeatherError::InvalidLocation("latitude"))
        );
        assert_eq!(
            WeatherLocation::new("Berlin", 0.0, 180.5, None),
            Err(WeatherError::InvalidLocation("longitude"))
        );
        assert_eq!(
            WeatherLocation::new("Berlin", 0.0, f64::INFINITY, None),
            Err(WeatherError::InvalidLocation("longitude"))
        );
        assert_eq!(
            WeatherLocation::new("   ", 0.0, 0.0, None),
            Err(WeatherError::InvalidLocation("name"))
        );
        let long = "x".repeat(NAME_MAX_CHARS + 1);
        assert_eq!(
            WeatherLocation::new(&long, 0.0, 0.0, None),
            Err(WeatherError::InvalidLocation("name"))
        );
        assert_eq!(
            WeatherLocation::new("Ber\tlin", 0.0, 0.0, None),
            Err(WeatherError::InvalidLocation("name"))
        );
        assert_eq!(
            WeatherLocation::new("Berlin", 0.0, 0.0, Some("")),
            Err(WeatherError::InvalidLocation("country"))
        );
    }

    #[test]
    fn temperature_unit_conversion() {
        assert_eq!(TemperatureUnit::Celsius.convert_c(-7), -7);
        assert_eq!(TemperatureUnit::Fahrenheit.convert_c(0), 32);
        assert_eq!(TemperatureUnit::Fahrenheit.convert_c(100), 212);
        assert_eq!(TemperatureUnit::Fahrenheit.convert_c(-40), -40);
        assert_eq!(TemperatureUnit::Fahrenheit.convert_c(37), 99);
        assert_eq!(TemperatureUnit::Fahrenheit.convert_c(-17), 1);
    }

    #[test]
    fn settings_round_trip_through_snapshot() {
        let settings = WeatherSettings {
            enabled: true,
            location: Some(WeatherLocation::new("Sao\u{e3}o", -23.55, -46.63, None).unwrap()),
            units: TemperatureUnit::Fahrenheit,
        };
        let stored = apply(settings.to_changes());
        assert_eq!(WeatherSettings::from_snapshot(&stored), settings);

        let cleared = WeatherSettings {
            location: None,
            ..settings.clone()
        };
        let stored = apply(cleared.to_changes());
        assert_eq!(WeatherSettings::from_snapshot(&stored), cleared);
    }

    #[test]
    fn settings_defaults_and_invalid_stored_values() {
        let empty = WeatherSettings::from_snapshot(&SettingsSnapshot::new(0));
        assert_eq!(empty, WeatherSettings::default());
        assert!(!empty.enabled);
        assert_eq!(empty.units, TemperatureUnit::Celsius);

        let mut snapshot = SettingsSnapshot::new(0);
        snapshot.values.insert(
            WeatherSettings::SETTINGS_KEY.to_owned(),
            SettingValue::Text("yes".to_owned()),
        );
        snapshot.values.insert(
            WeatherSettings::LOCATION_KEY.to_owned(),
            SettingValue::Text("999\t0\t\tBad".to_owned()),
        );
        snapshot.values.insert(
            WeatherSettings::UNITS_KEY.to_owned(),
            SettingValue::Text("kelvin".to_owned()),
        );
        assert_eq!(
            WeatherSettings::from_snapshot(&snapshot),
            WeatherSettings::default()
        );
    }

    #[test]
    fn state_transitions() {
        let disabled = WeatherSettings::default();
        let unconfigured = WeatherSettings {
            enabled: true,
            ..Default::default()
        };
        let configured = WeatherSettings {
            enabled: true,
            location: Some(berlin()),
            units: TemperatureUnit::Celsius,
        };
        assert_eq!(
            WeatherState::Loading.begin_refresh(&disabled),
            WeatherState::Disabled
        );
        assert_eq!(
            WeatherState::Ready(snapshot()).begin_refresh(&disabled),
            WeatherState::Disabled
        );
        assert_eq!(
            WeatherState::Loading.begin_refresh(&unconfigured),
            WeatherState::Unconfigured
        );

        let loading = WeatherState::Disabled.begin_refresh(&configured);
        assert_eq!(loading, WeatherState::Loading);

        let snapshot = snapshot();
        let ready = loading.clone().finish_refresh(Ok(snapshot.clone()));
        assert_eq!(ready, WeatherState::Ready(snapshot.clone()));
        assert_eq!(
            ready.clone().begin_refresh(&configured),
            WeatherState::Ready(snapshot.clone())
        );

        let offline = ready.finish_refresh(Err(WeatherError::Transport));
        assert_eq!(offline, WeatherState::Offline(Some(snapshot.clone())));
        assert_eq!(
            offline.clone().begin_refresh(&configured),
            WeatherState::Offline(Some(snapshot.clone()))
        );
        assert_eq!(
            offline.finish_refresh(Ok(snapshot.clone())),
            WeatherState::Ready(snapshot)
        );

        assert_eq!(
            WeatherState::Loading.finish_refresh(Err(WeatherError::TooLarge)),
            WeatherState::Error(WeatherError::TooLarge)
        );
        assert_eq!(
            WeatherState::Error(WeatherError::Transport).begin_refresh(&configured),
            WeatherState::Loading
        );
    }
}
