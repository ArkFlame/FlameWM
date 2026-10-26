//! Automatic location and time zone from one IP lookup (opt-in: `autoLocationEnabled`).
//!
//! The lookup runs on a worker thread through the same [`CurlFetcher`] seam as the forecast.
//! Providers are tried in order; the first valid answer wins. Results are validated before they
//! reach settings. The time zone is applied to this process only (the `TZ` variable); the system
//! zone and `/etc` are never touched.

use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_platform::settings::is_valid_time_zone_name;
use flamewm_shell_core::weather::{HttpGet, WeatherError, WeatherLocation, WeatherSettings};

use crate::weather_provider::{json, CurlFetcher};

pub const AUTO_LOCATION_KEY: &str = "autoLocationEnabled";
pub const AUTO_TIME_ZONE_ENABLED_KEY: &str = "autoTimeZoneEnabled";
pub const AUTO_TIME_ZONE_KEY: &str = "autoTimeZone";
/// Lookup cadence while enabled: startup or enable, network-up, then at most every 6 hours.
pub const AUTO_LOOKUP_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const ZONEINFO_ROOT: &str = "/usr/share/zoneinfo";

type ProviderParser = fn(&str) -> Result<IpFix, WeatherError>;

/// Providers in fallback order. `ipwho.is` carries the country name; `ipinfo.io` the code only.
const PROVIDERS: [(&str, ProviderParser); 2] = [
    ("https://ipwho.is/", parse_ipwho),
    ("https://ipinfo.io/json", parse_ipinfo),
];

/// One validated IP-geolocation answer.
#[derive(Debug, Clone, PartialEq)]
pub struct IpFix {
    pub location: WeatherLocation,
    pub time_zone: Option<String>,
}

fn number_field(value: &json::Json) -> Result<f64, WeatherError> {
    match value {
        json::Json::Num(n) if n.is_finite() => Ok(*n),
        _ => Err(WeatherError::Parse("wrong type")),
    }
}

fn text_field(value: &json::Json) -> Result<&str, WeatherError> {
    match value {
        json::Json::Str(s) => Ok(s),
        _ => Err(WeatherError::Parse("wrong type")),
    }
}

fn zone(text: &str) -> Option<String> {
    is_valid_time_zone_name(text).then(|| text.to_owned())
}

/// Parses `ipwho.is` (`success`, `latitude`, `longitude`, `city`, `country`, `timezone.id`).
pub fn parse_ipwho(body: &str) -> Result<IpFix, WeatherError> {
    let root = json::parse(body)?;
    if root.field("success")? != &json::Json::Bool(true) {
        return Err(WeatherError::Parse("lookup refused"));
    }
    let latitude = number_field(root.field("latitude")?)?;
    let longitude = number_field(root.field("longitude")?)?;
    let name = text_field(root.field("city")?)?;
    let country = text_field(root.field("country")?)?;
    let time_zone = match root.optional("timezone")? {
        Some(tz) => zone(text_field(tz.field("id")?)?),
        None => None,
    };
    Ok(IpFix {
        location: WeatherLocation::new(name, latitude, longitude, Some(country))?,
        time_zone,
    })
}

/// Parses `ipinfo.io/json` (`city`, `country`, `loc` as `"lat,lon"`, `timezone`).
pub fn parse_ipinfo(body: &str) -> Result<IpFix, WeatherError> {
    let root = json::parse(body)?;
    let name = text_field(root.field("city")?)?;
    let country = text_field(root.field("country")?)?;
    let loc = text_field(root.field("loc")?)?;
    let (lat, lon) = loc
        .split_once(',')
        .ok_or(WeatherError::Parse("malformed loc"))?;
    let latitude = lat
        .parse::<f64>()
        .map_err(|_| WeatherError::Parse("malformed loc"))?;
    let longitude = lon
        .parse::<f64>()
        .map_err(|_| WeatherError::Parse("malformed loc"))?;
    let time_zone = match root.optional("timezone")? {
        Some(tz) => zone(text_field(tz)?),
        None => None,
    };
    Ok(IpFix {
        location: WeatherLocation::new(name, latitude, longitude, Some(country))?,
        time_zone,
    })
}

/// Tries each provider in order and returns the first valid answer. When all fail, the
/// last error is returned. Only the response body is parsed; URLs are never logged.
pub fn lookup_with(http: &impl HttpGet) -> Result<IpFix, WeatherError> {
    let mut last = WeatherError::Transport;
    for (url, parse) in PROVIDERS {
        match http.get(url).and_then(|body| parse(&body)) {
            Ok(fix) => return Ok(fix),
            Err(error) => last = error,
        }
    }
    Err(last)
}

/// Worker-thread lookup on the production curl seam. A failed thread start reports transport.
#[must_use]
pub fn spawn_ip_lookup() -> mpsc::Receiver<Result<IpFix, WeatherError>> {
    let (tx, rx) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("flamewm-auto-location".to_owned())
        .spawn(move || {
            let _ = tx.send(lookup_with(&CurlFetcher));
        });
    rx
}

type FixReceiver = Arc<Mutex<mpsc::Receiver<Result<IpFix, WeatherError>>>>;

/// Lookup cadence. Returns a finished lookup when one settles; starts a new one when due.
#[derive(Debug, Default)]
pub struct AutoLookup {
    last_started: Option<Instant>,
    inflight: Option<FixReceiver>,
    was_enabled: bool,
    was_online: bool,
}

impl AutoLookup {
    /// The next instant the shell loop must wake to poll an in-flight lookup; the worker
    /// has no fd, so without this the result waits for an unrelated event.
    #[must_use]
    pub fn poll_deadline(&self, now: Instant) -> Option<Instant> {
        self.inflight
            .as_ref()
            .map(|_| now + crate::weather_host::WEATHER_POLL_INTERVAL)
    }

    /// Due on enable, on a network-up edge, or when the last start is 6 hours old. Disabled
    /// drops any in-flight lookup. Never blocks.
    pub fn tick(
        &mut self,
        now: Instant,
        enabled: bool,
        online: bool,
        start: impl FnOnce() -> mpsc::Receiver<Result<IpFix, WeatherError>>,
    ) -> Option<Result<IpFix, WeatherError>> {
        let enable_edge = enabled && !self.was_enabled;
        let online_edge = online && !self.was_online;
        self.was_enabled = enabled;
        self.was_online = online;
        if !enabled {
            self.inflight = None;
            self.last_started = None;
            return None;
        }
        let polled = match self
            .inflight
            .as_ref()
            .map(|rx| rx.lock().map(|g| g.try_recv()))
        {
            None => None,
            Some(Ok(Ok(result))) => Some(result),
            Some(Ok(Err(mpsc::TryRecvError::Empty))) => None,
            Some(_) => Some(Err(WeatherError::Transport)),
        };
        let finished = polled.is_some();
        if finished {
            self.inflight = None;
        }
        let due = enable_edge
            || online_edge
            || self
                .last_started
                .is_none_or(|started| now.duration_since(started) >= AUTO_LOOKUP_INTERVAL);
        if due && self.inflight.is_none() {
            self.last_started = Some(now);
            self.inflight = Some(Arc::new(Mutex::new(start())));
        }
        polled
    }
}

/// Settings change that writes the auto fix as the weather location. The current enabled and
/// units values are not part of it.
#[must_use]
pub fn location_change(
    current: &WeatherSettings,
    fix: &IpFix,
) -> Option<flamewm_api::settings::SettingsChange> {
    if current.location.as_ref() == Some(&fix.location) {
        return None;
    }
    let candidate = WeatherSettings {
        location: Some(fix.location.clone()),
        ..current.clone()
    };
    candidate
        .to_changes()
        .into_iter()
        .find(|change| change.key == WeatherSettings::LOCATION_KEY)
}

#[must_use]
pub fn flag(snapshot: &SettingsSnapshot, key: &str) -> bool {
    snapshot.values.get(key) == Some(&SettingValue::Boolean(true))
}

/// The zone to apply to this process: the cached automatic zone while its switch is on.
#[must_use]
pub fn desired_time_zone(snapshot: &SettingsSnapshot) -> Option<String> {
    if !flag(snapshot, AUTO_TIME_ZONE_ENABLED_KEY) {
        return None;
    }
    match snapshot.values.get(AUTO_TIME_ZONE_KEY) {
        Some(SettingValue::Text(zone)) if is_valid_time_zone_name(zone) => Some(zone.clone()),
        _ => None,
    }
}

/// Sets this process's zone to `name` after checking the zone file under `root`. Never writes
/// system files. Returns false when the name is invalid or the zone file is missing. The shell's
/// own clock picks it up; the lock and desktop processes do not inherit it (they need their own).
#[must_use]
/// `set_env` receives the zone name for the process `TZ` variable; production passes
/// `set_var`, tests record the value instead of touching the process environment.
pub fn apply_time_zone_in(root: &std::path::Path, name: &str, set_env: impl FnOnce(&str)) -> bool {
    if !is_valid_time_zone_name(name) || !root.join(name).is_file() {
        return false;
    }
    set_env(name);
    true
}

/// Applies `name` using the system zoneinfo database. Edition 2021: `set_var` is safe, and
/// chrono's Local re-reads TZ at most once per second (chrono 0.4.45 local/unix.rs), so the
/// taskbar clock and calendar follow without tzset.
#[must_use]
pub fn apply_time_zone(name: &str) -> bool {
    apply_time_zone_in(std::path::Path::new(ZONEINFO_ROOT), name, |zone| {
        std::env::set_var("TZ", zone);
    })
}

/// Clears this process's override so the system zone applies again.
pub fn restore_system_time_zone() {
    std::env::remove_var("TZ");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::weather_host::test_support::answer as finished;
    use std::collections::BTreeMap;

    struct Canned(BTreeMap<&'static str, Result<String, WeatherError>>);

    impl HttpGet for Canned {
        fn get(&self, url: &str) -> Result<String, WeatherError> {
            self.0
                .get(url)
                .cloned()
                .unwrap_or(Err(WeatherError::Transport))
        }
    }

    const IPWHO: &str = r#"{"ip":"1.2.3.4","success":true,"city":"Buenos Aires","country":"Argentina","latitude":-34.6131499,"longitude":-58.3772348,"timezone":{"id":"America/Argentina/Buenos_Aires","abbr":"-03"}}"#;
    const IPINFO: &str = r#"{"ip":"1.2.3.4","city":"Buenos Aires","country":"AR","loc":"-34.6131,-58.3772","timezone":"America/Argentina/Buenos_Aires"}"#;

    #[test]
    fn ipwho_fixture_parses_location_and_zone() {
        let fix = parse_ipwho(IPWHO).expect("fixture");
        assert_eq!(fix.location.name, "Buenos Aires");
        assert_eq!(fix.location.country.as_deref(), Some("Argentina"));
        assert!((fix.location.latitude + 34.613_149_9).abs() < 1e-9);
        assert_eq!(
            fix.time_zone.as_deref(),
            Some("America/Argentina/Buenos_Aires")
        );
    }

    #[test]
    fn ipinfo_fixture_parses_loc_pair_and_zone() {
        let fix = parse_ipinfo(IPINFO).expect("fixture");
        assert_eq!(fix.location.country.as_deref(), Some("AR"));
        assert!((fix.location.longitude + 58.3772).abs() < 1e-9);
        assert_eq!(
            fix.time_zone.as_deref(),
            Some("America/Argentina/Buenos_Aires")
        );
    }

    #[test]
    fn invalid_answers_are_rejected_and_bad_zone_is_dropped() {
        assert!(parse_ipwho(&IPWHO.replace("\"success\":true", "\"success\":false")).is_err());
        assert!(parse_ipwho(&IPWHO.replace("-34.6131499", "91")).is_err());
        assert!(parse_ipwho(&IPWHO.replace("-58.3772348", "181")).is_err());
        assert!(parse_ipwho(&IPWHO.replace("-34.6131499", "\"x\"")).is_err());
        assert!(parse_ipinfo(&IPINFO.replace("-34.6131,-58.3772", "nope")).is_err());
        assert!(parse_ipinfo(&IPINFO.replace("-34.6131,-58.3772", "-34.6,-58.3,9")).is_err());
        let bad_zone = IPWHO.replace("America/Argentina/Buenos_Aires", "../../etc/passwd");
        assert_eq!(
            parse_ipwho(&bad_zone)
                .expect("location still valid")
                .time_zone,
            None
        );
        assert!(parse_ipinfo("not json").is_err());
    }

    #[test]
    fn lookup_falls_back_to_next_provider_on_failure() {
        let http = Canned(BTreeMap::from([
            ("https://ipwho.is/", Err(WeatherError::Transport)),
            ("https://ipinfo.io/json", Ok(IPINFO.to_owned())),
        ]));
        assert_eq!(
            lookup_with(&http)
                .expect("fallback")
                .location
                .country
                .as_deref(),
            Some("AR")
        );

        let both_down = Canned(BTreeMap::new());
        assert_eq!(lookup_with(&both_down), Err(WeatherError::Transport));

        let first_wins = Canned(BTreeMap::from([
            ("https://ipwho.is/", Ok(IPWHO.to_owned())),
            ("https://ipinfo.io/json", Ok(IPINFO.to_owned())),
        ]));
        assert_eq!(
            lookup_with(&first_wins)
                .expect("first")
                .location
                .country
                .as_deref(),
            Some("Argentina")
        );
    }

    fn fix() -> IpFix {
        parse_ipinfo(IPINFO).unwrap()
    }

    #[test]
    fn cadence_starts_on_enable_then_waits_six_hours_or_network_edge() {
        let mut host = AutoLookup::default();
        let t0 = Instant::now();
        let mut starts = 0;
        let start = |starts: &mut u32| {
            *starts += 1;
            finished(Ok(fix()))
        };
        // Disabled: nothing starts.
        assert!(host.tick(t0, false, true, || start(&mut starts)).is_none());
        assert_eq!(starts, 0);
        // Enable: one start, result delivered on the next poll.
        assert!(host.tick(t0, true, true, || start(&mut starts)).is_none());
        assert_eq!(starts, 1);
        assert!(host.tick(t0, true, true, || start(&mut starts)).is_some());
        // Still within six hours and online: no new start.
        let soon = t0 + Duration::from_secs(60);
        assert!(host.tick(soon, true, true, || start(&mut starts)).is_none());
        assert_eq!(starts, 1);
        // Offline then online again: network-up edge starts a lookup.
        assert!(host
            .tick(soon, true, false, || start(&mut starts))
            .is_none());
        assert!(host.tick(soon, true, true, || start(&mut starts)).is_none());
        assert_eq!(starts, 2);
        // Six hours after the last start: the settled result lands and the next lookup starts.
        let later = soon + AUTO_LOOKUP_INTERVAL;
        assert!(host
            .tick(later, true, true, || start(&mut starts))
            .is_some());
        assert_eq!(starts, 3);
    }

    #[test]
    fn inflight_lookup_keeps_the_loop_polling_until_it_settles() {
        let mut host = AutoLookup::default();
        let t0 = Instant::now();
        assert_eq!(host.poll_deadline(t0), None);
        let (tx, pending) = mpsc::channel();
        let mut pending = Some(pending);
        host.tick(t0, true, true, || pending.take().expect("one lookup"));
        assert!(host.poll_deadline(t0).is_some(), "lookup in flight");
        tx.send(Ok(fix())).unwrap();
        assert!(host
            .tick(t0, true, true, || unreachable!("lookup in flight"))
            .is_some());
        assert_eq!(
            host.poll_deadline(t0),
            None,
            "idle once the fix is delivered"
        );
    }

    #[test]
    fn disable_drops_inflight_lookup() {
        let mut host = AutoLookup::default();
        let t0 = Instant::now();
        let (_tx, pending) = mpsc::channel();
        host.tick(t0, true, true, || pending);
        assert!(host.tick(t0, false, true, || finished(Ok(fix()))).is_none());
        assert!(host.inflight.is_none());
    }

    #[test]
    fn location_change_skips_identical_fix_and_keeps_switches() {
        let current = WeatherSettings {
            enabled: true,
            location: None,
            units: flamewm_shell_core::weather::TemperatureUnit::Celsius,
        };
        let change = location_change(&current, &fix()).expect("location differs");
        assert_eq!(change.key, WeatherSettings::LOCATION_KEY);
        let same = WeatherSettings {
            location: Some(fix().location),
            ..current
        };
        assert!(location_change(&same, &fix()).is_none());
    }

    #[test]
    fn time_zone_apply_uses_only_the_given_root_and_checks_existence() {
        let dir = std::env::temp_dir().join(format!("flamewm-tz-test-{}", std::process::id()));
        let zone_dir = dir.join("Test");
        std::fs::create_dir_all(&zone_dir).unwrap();
        std::fs::write(zone_dir.join("Zone"), b"TZif").unwrap();
        let mut applied = None;
        assert!(apply_time_zone_in(&dir, "Test/Zone", |zone| applied = Some(zone.to_owned())));
        assert_eq!(applied.as_deref(), Some("Test/Zone"));
        assert!(!apply_time_zone_in(&dir, "Test/Missing", |_| unreachable!(
            "missing zone"
        )));
        assert!(!apply_time_zone_in(&dir, "../Test/Zone", |_| unreachable!(
            "invalid name"
        )));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn desired_zone_requires_switch_and_valid_cache() {
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot.values.insert(
            AUTO_TIME_ZONE_KEY.to_owned(),
            SettingValue::Text("Asia/Tokyo".to_owned()),
        );
        assert_eq!(desired_time_zone(&snapshot), None);
        snapshot.values.insert(
            AUTO_TIME_ZONE_ENABLED_KEY.to_owned(),
            SettingValue::Boolean(true),
        );
        assert_eq!(desired_time_zone(&snapshot).as_deref(), Some("Asia/Tokyo"));
        snapshot.values.insert(
            AUTO_TIME_ZONE_KEY.to_owned(),
            SettingValue::Text("/etc".to_owned()),
        );
        assert_eq!(desired_time_zone(&snapshot), None);
    }
}
