//! Weather scheduler (main shell) and location search state (quick-control host).
//!
//! The main shell owns [`WeatherHost`]: it fetches the forecast, applies the
//! result, and pushes a [`WeatherFeed`] to the popup host over the existing
//! parent->host stdin channel. The popup host owns [`LocationSearch`], which
//! runs the geocoding query for the picker. Both run network work on a worker
//! thread; the UI thread only polls a channel while a request is in flight.

use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use flamewm_shell_core::weather::{
    WeatherError, WeatherFeed, WeatherLocation, WeatherProvider, WeatherSettings, WeatherState,
};

use crate::weather_provider::{CurlFetcher, OpenMeteo};

/// Minimum spacing between periodic weather fetches (opening the popup always fetches).
pub const WEATHER_REFRESH_INTERVAL: Duration = Duration::from_secs(30 * 60);
/// Loop cadence while a fetch is in flight; bounded by the curl timeout.
pub const WEATHER_POLL_INTERVAL: Duration = Duration::from_millis(100);

type WeatherResult = Result<flamewm_shell_core::weather::WeatherSnapshot, WeatherError>;

/// Weather refresh scheduler. It never fetches while disabled or without a location,
/// fetches once when the popup opens, then at most every [`WEATHER_REFRESH_INTERVAL`]
/// while enabled. Network work runs on a worker thread; the caller only polls the channel.
#[derive(Debug, Clone)]
pub struct WeatherHost {
    pub settings: WeatherSettings,
    pub state: WeatherState,
    fetch_on_open: bool,
    last_started: Option<Instant>,
    /// Shared so the scheduler stays `Clone`; only the owning thread polls it.
    inflight: Option<Arc<Mutex<mpsc::Receiver<WeatherResult>>>>,
}

impl WeatherHost {
    #[must_use]
    pub fn new() -> Self {
        Self {
            settings: WeatherSettings::default(),
            state: WeatherState::Disabled,
            fetch_on_open: false,
            last_started: None,
            inflight: None,
        }
    }

    /// The view pushed to the popup host.
    #[must_use]
    pub fn feed(&self) -> WeatherFeed {
        WeatherFeed {
            settings: self.settings.clone(),
            state: self.state.clone(),
        }
    }

    /// Applies settings and resets the visible state to match (disabled or unconfigured).
    /// Enabling or choosing a location fetches once on the next tick, so the tray
    /// shows data without the popup open. A location change drops the old city's
    /// snapshot and any in-flight fetch for it.
    pub fn apply_settings(&mut self, settings: WeatherSettings) {
        let location_changed = self.settings.location != settings.location;
        let became_active = settings.enabled
            && settings.location.is_some()
            && (!self.settings.enabled || location_changed);
        if !settings.enabled || location_changed {
            self.inflight = None;
        }
        if location_changed {
            self.state = WeatherState::Disabled;
        }
        if became_active {
            self.fetch_on_open = true;
        }
        self.settings = settings;
        let state = std::mem::replace(&mut self.state, WeatherState::Disabled);
        self.state = state.begin_refresh(&self.settings);
    }

    /// Called when the weather popup opens: fetch on the next tick.
    pub fn request_open_fetch(&mut self) {
        self.fetch_on_open = true;
    }

    #[must_use]
    pub fn fetch_due(&self, now: Instant) -> bool {
        if !self.settings.enabled || self.settings.location.is_none() || self.inflight.is_some() {
            return false;
        }
        self.fetch_on_open
            || self
                .last_started
                .is_some_and(|started| now.duration_since(started) >= WEATHER_REFRESH_INTERVAL)
    }

    /// The next instant the shell loop must wake to poll an in-flight fetch.
    /// The worker's result has no fd, so without this the loop sleeps until an
    /// unrelated event and the popup stays on "Loading...".
    #[must_use]
    pub fn poll_deadline(&self, now: Instant) -> Option<Instant> {
        self.inflight.as_ref().map(|_| now + WEATHER_POLL_INTERVAL)
    }

    /// Advances the scheduler: applies a finished fetch, then starts one if due.
    /// Returns true when the visible state changed.
    pub fn tick(
        &mut self,
        now: Instant,
        fetch: impl FnOnce(WeatherLocation) -> mpsc::Receiver<WeatherResult>,
    ) -> bool {
        let mut changed = false;
        let polled: Option<WeatherResult> = match self.inflight.as_ref() {
            None => None,
            Some(rx) => match rx.lock().map(|guard| guard.try_recv()) {
                Ok(Ok(result)) => Some(result),
                Ok(Err(mpsc::TryRecvError::Empty)) => None,
                Ok(Err(mpsc::TryRecvError::Disconnected)) | Err(_) => {
                    Some(Err(WeatherError::Transport))
                }
            },
        };
        if let Some(result) = polled {
            self.inflight = None;
            let state = std::mem::replace(&mut self.state, WeatherState::Disabled);
            self.state = state.finish_refresh(result);
            changed = true;
        }
        if self.fetch_due(now) {
            if let Some(location) = self.settings.location.clone() {
                let state = std::mem::replace(&mut self.state, WeatherState::Disabled);
                self.state = state.begin_refresh(&self.settings);
                self.last_started = Some(now);
                self.fetch_on_open = false;
                self.inflight = Some(Arc::new(Mutex::new(fetch(location))));
                changed = true;
            }
        }
        changed
    }
}

impl Default for WeatherHost {
    fn default() -> Self {
        Self::new()
    }
}

/// Production fetch: Open-Meteo through the curl fetcher, on a worker thread.
/// If the thread cannot start, the dropped sender reports a transport error.
pub fn spawn_weather_fetch(location: WeatherLocation) -> mpsc::Receiver<WeatherResult> {
    let (tx, rx) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("flamewm-weather".to_owned())
        .spawn(move || {
            let _ = tx.send(OpenMeteo(CurlFetcher).forecast(&location));
        });
    rx
}

type SearchResult = Result<Vec<WeatherLocation>, WeatherError>;

/// Production geocoding search on a worker thread (same fetcher as the forecast).
pub fn spawn_location_search(query: String) -> mpsc::Receiver<SearchResult> {
    let (tx, rx) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("flamewm-weather-search".to_owned())
        .spawn(move || {
            let _ = tx.send(OpenMeteo(CurlFetcher).search(&query));
        });
    rx
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchStatus {
    #[default]
    Idle,
    Searching,
    NoMatches,
    Failed,
    /// The chosen location could not be saved; the picker stays open to retry.
    SaveFailed,
}

/// Location picker state in the popup host. The query lives here; the
/// results come from one geocoding request at a time.
#[derive(Debug, Clone, Default)]
pub struct LocationSearch {
    pub query: String,
    pub focused: bool,
    pub results: Vec<WeatherLocation>,
    pub status: SearchStatus,
    inflight: Option<Arc<Mutex<mpsc::Receiver<SearchResult>>>>,
}

impl LocationSearch {
    /// Starts a search for the trimmed query. Blank queries are ignored.
    pub fn submit(&mut self, spawn: impl FnOnce(String) -> mpsc::Receiver<SearchResult>) -> bool {
        let query = self.query.trim().to_owned();
        if query.is_empty() {
            return false;
        }
        self.results.clear();
        self.status = SearchStatus::Searching;
        self.inflight = Some(Arc::new(Mutex::new(spawn(query))));
        true
    }

    /// True while a geocoding request is outstanding and must be polled.
    #[must_use]
    pub fn in_flight(&self) -> bool {
        self.inflight.is_some()
    }

    /// Applies a finished search. Returns true when the visible state changed.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = self.inflight.as_ref() else {
            return false;
        };
        let result = match rx.lock().map(|guard| guard.try_recv()) {
            Ok(Ok(result)) => result,
            Ok(Err(mpsc::TryRecvError::Empty)) => return false,
            Ok(Err(mpsc::TryRecvError::Disconnected)) | Err(_) => Err(WeatherError::Transport),
        };
        self.inflight = None;
        match result {
            Ok(found) if found.is_empty() => self.status = SearchStatus::NoMatches,
            Ok(found) => {
                self.results = found;
                self.status = SearchStatus::Idle;
            }
            Err(_) => self.status = SearchStatus::Failed,
        }
        true
    }

    /// Ends the picker session after a location is chosen or the query is cleared.
    pub fn reset(&mut self) {
        self.query.clear();
        self.results.clear();
        self.status = SearchStatus::Idle;
        self.inflight = None;
    }
}

/// Shared weather test fakes; shared by the sibling `weather_auto` tests.
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::mpsc;

    /// A worker that already finished with `result`; no socket, no thread.
    pub(crate) fn answer<T>(result: T) -> mpsc::Receiver<T> {
        let (tx, rx) = mpsc::channel();
        tx.send(result).unwrap();
        rx
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::answer;
    use super::*;
    use flamewm_shell_core::weather::{TemperatureUnit, WeatherCondition, WeatherSnapshot};
    use std::cell::Cell;

    fn location() -> WeatherLocation {
        WeatherLocation::new("Berlin", 52.52, 13.41, None).unwrap()
    }

    fn settings(enabled: bool, with_location: bool) -> WeatherSettings {
        WeatherSettings {
            enabled,
            location: with_location.then(location),
            units: TemperatureUnit::Celsius,
        }
    }

    /// Stands in for the network: a ready fixture, no socket, no thread.
    fn fixture_rx() -> mpsc::Receiver<WeatherResult> {
        answer(Ok(WeatherSnapshot {
            location: location(),
            temperature_c: 9,
            condition: WeatherCondition::Clear,
            days: Vec::new(),
            fetched_at: 1,
        }))
    }

    fn search_fixture(result: SearchResult) -> impl FnOnce(String) -> mpsc::Receiver<SearchResult> {
        move |_| answer(result)
    }

    #[test]
    fn disabled_or_unconfigured_never_fetches() {
        for s in [
            settings(false, true),
            settings(true, false),
            settings(false, false),
        ] {
            let mut host = WeatherHost::new();
            host.apply_settings(s);
            host.request_open_fetch();
            let calls = Cell::new(0);
            let start = Instant::now();
            for step in 0..5 {
                host.tick(start + WEATHER_REFRESH_INTERVAL * step, |_| {
                    calls.set(calls.get() + 1);
                    fixture_rx()
                });
            }
            assert_eq!(calls.get(), 0);
        }
        let mut host = WeatherHost::new();
        host.apply_settings(settings(false, true));
        assert_eq!(host.state, WeatherState::Disabled);
    }

    #[test]
    fn in_flight_fetch_keeps_the_loop_polling_until_the_result_lands() {
        let mut host = WeatherHost::new();
        host.apply_settings(settings(true, true));
        let start = Instant::now();
        assert_eq!(host.poll_deadline(start), None, "nothing in flight yet");
        // Worker that has not answered yet: the sender stays alive, so try_recv is Empty.
        let (tx, rx) = mpsc::channel::<WeatherResult>();
        let mut pending = Some(rx);
        host.tick(start, |_| pending.take().expect("one fetch"));
        assert_eq!(
            host.poll_deadline(start),
            Some(start + WEATHER_POLL_INTERVAL)
        );
        host.tick(start + WEATHER_POLL_INTERVAL, |_| {
            unreachable!("fetch in flight")
        });
        assert_eq!(host.state, WeatherState::Loading);
        assert!(host.poll_deadline(start).is_some());
        tx.send(Ok(WeatherSnapshot {
            location: location(),
            temperature_c: 9,
            condition: WeatherCondition::Clear,
            days: Vec::new(),
            fetched_at: 1,
        }))
        .unwrap();
        assert!(host.tick(start + WEATHER_POLL_INTERVAL, |_| unreachable!()));
        assert!(matches!(host.state, WeatherState::Ready(_)));
        assert_eq!(
            host.poll_deadline(start),
            None,
            "idle again after the result"
        );
    }

    #[test]
    fn enabled_with_location_fetches_on_open_then_every_thirty_minutes() {
        let mut host = WeatherHost::new();
        host.apply_settings(settings(true, true));
        assert_eq!(host.state, WeatherState::Loading);
        let calls = Cell::new(0);
        let start = Instant::now();
        let fetch = |_: WeatherLocation| {
            calls.set(calls.get() + 1);
            fixture_rx()
        };
        host.tick(start, fetch);
        assert_eq!(
            calls.get(),
            1,
            "enabling with a location fetches once for the tray"
        );
        host.tick(start, fetch);
        assert!(matches!(host.state, WeatherState::Ready(_)));
        assert_eq!(calls.get(), 1);

        host.request_open_fetch();
        host.tick(start, fetch);
        assert_eq!(calls.get(), 2, "opening the popup fetches");
        host.tick(start, fetch);
        assert_eq!(calls.get(), 2);

        host.tick(start + Duration::from_secs(29 * 60), fetch);
        assert_eq!(calls.get(), 2, "still inside the 30 minute window");
        host.tick(start + WEATHER_REFRESH_INTERVAL, fetch);
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn disabling_drops_the_inflight_fetch_and_stops_refreshing() {
        let mut host = WeatherHost::new();
        host.apply_settings(settings(true, true));
        let calls = Cell::new(0);
        let start = Instant::now();
        host.request_open_fetch();
        host.tick(start, |_| {
            calls.set(calls.get() + 1);
            fixture_rx()
        });
        host.apply_settings(settings(false, true));
        assert_eq!(host.state, WeatherState::Disabled);
        host.tick(start + WEATHER_REFRESH_INTERVAL * 2, |_| {
            calls.set(calls.get() + 1);
            fixture_rx()
        });
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn feed_mirrors_settings_and_state() {
        let mut host = WeatherHost::new();
        host.apply_settings(settings(true, true));
        let feed = host.feed();
        assert!(feed.settings.enabled);
        assert_eq!(feed.state, WeatherState::Loading);
    }

    #[test]
    fn location_search_ignores_blank_and_applies_results() {
        let mut search = LocationSearch::default();
        search.query = "   ".to_owned();
        assert!(!search.submit(search_fixture(Ok(Vec::new()))));
        assert_eq!(search.status, SearchStatus::Idle);

        search.query = " Berlin ".to_owned();
        assert!(search.submit(search_fixture(Ok(vec![location()]))));
        assert_eq!(search.status, SearchStatus::Searching);
        assert!(search.poll());
        assert_eq!(search.results, vec![location()]);
        assert_eq!(search.status, SearchStatus::Idle);
        assert!(!search.poll(), "a settled search polls quietly");
    }

    #[test]
    fn location_search_in_flight_until_its_result_is_polled() {
        let mut search = LocationSearch::default();
        assert!(!search.in_flight());
        search.query = "Berlin".to_owned();
        let (tx, rx) = mpsc::channel::<SearchResult>();
        let mut pending = Some(rx);
        assert!(search.submit(|_| pending.take().expect("one search")));
        assert!(
            search.in_flight(),
            "helper loop must keep polling while a search runs"
        );
        assert!(!search.poll(), "no result yet");
        assert!(search.in_flight());
        tx.send(Ok(vec![location()])).unwrap();
        assert!(search.poll());
        assert!(!search.in_flight());
    }

    #[test]
    fn location_search_reports_empty_and_failed_results() {
        let mut search = LocationSearch::default();
        search.query = "zzz".to_owned();
        search.submit(search_fixture(Ok(Vec::new())));
        search.poll();
        assert_eq!(search.status, SearchStatus::NoMatches);
        assert!(search.results.is_empty());

        search.submit(search_fixture(Err(WeatherError::Transport)));
        search.poll();
        assert_eq!(search.status, SearchStatus::Failed);
    }

    #[test]
    fn dropped_search_worker_reports_transport_failure() {
        let mut search = LocationSearch::default();
        search.query = "Oslo".to_owned();
        search.submit(|_| mpsc::channel().1);
        // The sender was dropped immediately: the worker vanished without a result.
        assert!(search.poll());
        assert_eq!(search.status, SearchStatus::Failed);
    }

    #[test]
    fn reset_clears_the_picker_session() {
        let mut search = LocationSearch::default();
        search.query = "Berlin".to_owned();
        search.submit(search_fixture(Ok(vec![location()])));
        search.poll();
        search.reset();
        assert!(search.query.is_empty());
        assert!(search.results.is_empty());
        assert_eq!(search.status, SearchStatus::Idle);
    }
}

#[cfg(test)]
mod flow_tests {
    use super::test_support::answer;
    use super::*;
    use crate::projection::{project_weather_popup, project_weather_tray};
    use flamewm_shell_core::weather::{
        DayForecast, TemperatureUnit, WeatherCondition, WeatherSnapshot,
    };
    use flamewm_ui_x11::test_support::RecordingUiDoc;
    use flamewm_ui_x11::UiColor;
    use std::cell::Cell;

    const VEIL: UiColor = UiColor {
        r: 0,
        g: 0,
        b: 0,
        a: 255,
    };

    fn berlin() -> WeatherLocation {
        WeatherLocation::new("Berlin", 52.52, 13.41, Some("Germany")).unwrap()
    }

    fn enabled_settings() -> WeatherSettings {
        WeatherSettings {
            enabled: true,
            location: Some(berlin()),
            units: TemperatureUnit::Celsius,
        }
    }

    fn week_snapshot() -> WeatherSnapshot {
        WeatherSnapshot {
            location: berlin(),
            temperature_c: 21,
            condition: WeatherCondition::Clear,
            days: (0..7)
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

    /// Starts one fetch answered by `result`, then polls it on the next tick.
    fn refresh(host: &mut WeatherHost, now: Instant, result: WeatherResult) {
        host.tick(now, |_| answer(result));
        host.tick(now, |_| unreachable!("a fetch is already in flight"));
    }

    fn tray_of(host: &WeatherHost) -> RecordingUiDoc {
        let feed = host.feed();
        let mut doc = RecordingUiDoc::default();
        project_weather_tray(&mut doc, &feed.state, feed.settings.units, true).unwrap();
        doc
    }

    fn popup_of(host: &WeatherHost) -> RecordingUiDoc {
        let feed = host.feed();
        let mut doc = RecordingUiDoc::default();
        project_weather_popup(
            &mut doc,
            &feed.state,
            &feed.settings,
            &LocationSearch::default(),
            VEIL,
        )
        .unwrap();
        doc
    }

    #[test]
    fn no_http_before_a_location_is_chosen() {
        let mut host = WeatherHost::new();
        host.apply_settings(WeatherSettings {
            enabled: true,
            location: None,
            units: TemperatureUnit::Celsius,
        });
        host.request_open_fetch();
        let calls = Cell::new(0);
        host.tick(Instant::now(), |_| {
            calls.set(calls.get() + 1);
            answer(Ok(week_snapshot()))
        });
        assert_eq!(calls.get(), 0);
        assert_eq!(host.state, WeatherState::Unconfigured);
        assert_eq!(tray_of(&host).texts["tray-weather-temp"], "--");
    }

    #[test]
    fn picked_location_fetches_then_shows_tray_text_and_seven_rows() {
        let mut host = WeatherHost::new();
        let calls = Cell::new(0);
        let now = Instant::now();
        host.apply_settings(enabled_settings());
        host.tick(now, |_| {
            calls.set(calls.get() + 1);
            answer(Ok(week_snapshot()))
        });
        assert_eq!(calls.get(), 1, "enabling with a location fetches once");
        assert_eq!(host.state, WeatherState::Loading);
        host.tick(now, |_| unreachable!("a fetch is already in flight"));
        assert!(matches!(host.state, WeatherState::Ready(_)));
        assert_eq!(calls.get(), 1);

        let tray = tray_of(&host);
        assert_eq!(tray.texts["tray-weather-temp"], "21\u{b0}");
        assert!(tray.visible["tray-weather-clear"]);
        let popup = popup_of(&host);
        assert_eq!(popup.texts["weather-temp"], "21\u{b0}");
        assert!(popup.visible["weather-day-7-name"]);
        assert!(!popup.visible.contains_key("weather-day-8-name"));
    }

    #[test]
    fn failed_refresh_keeps_last_data_offline_and_failed_first_load_shows_error() {
        let mut host = WeatherHost::new();
        let now = Instant::now();
        host.apply_settings(enabled_settings());
        refresh(&mut host, now, Ok(week_snapshot()));
        host.request_open_fetch();
        refresh(&mut host, now, Err(WeatherError::Transport));
        assert!(matches!(host.state, WeatherState::Offline(Some(_))));
        assert_eq!(tray_of(&host).texts["tray-weather-temp"], "21\u{b0}");
        assert_eq!(popup_of(&host).texts["weather-state"], "Offline");

        let mut failed = WeatherHost::new();
        failed.apply_settings(enabled_settings());
        refresh(&mut failed, now, Err(WeatherError::Transport));
        assert_eq!(failed.state, WeatherState::Error(WeatherError::Transport));
        assert_eq!(tray_of(&failed).texts["tray-weather-temp"], "--");
        assert_eq!(
            popup_of(&failed).texts["weather-state"],
            "Could not load weather"
        );
    }

    #[test]
    fn changing_location_drops_the_previous_city_snapshot() {
        let mut host = WeatherHost::new();
        let now = Instant::now();
        host.apply_settings(enabled_settings());
        refresh(&mut host, now, Ok(week_snapshot()));
        let mut other = enabled_settings();
        other.location = Some(WeatherLocation::new("Paris", 48.85, 2.35, None).unwrap());
        host.apply_settings(other);
        assert_eq!(host.state, WeatherState::Loading);
        assert_eq!(tray_of(&host).texts["tray-weather-temp"], "--");
    }
}
