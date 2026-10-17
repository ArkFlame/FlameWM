//! Open-Meteo provider and CurlFetcher for the weather data layer.
//!
//! LICENSING: Open-Meteo's free API is for noncommercial use only and its data
//! requires attribution ([`OPEN_METEO_ATTRIBUTION`]). Check the current terms
//! before shipping commercially, and show the attribution wherever forecast data
//! is displayed.
//!
//! [`CurlFetcher`] runs the system curl binary directly (no shell) with bounded
//! time and response size. URLs are never logged, and stderr is discarded.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use flamewm_shell_core::weather::{
    DayForecast, HttpGet, WeatherCondition, WeatherError, WeatherLocation, WeatherProvider,
    WeatherSnapshot,
};

pub const OPEN_METEO_ATTRIBUTION: &str = "Weather data by Open-Meteo.com (CC BY 4.0)";
pub const MAX_RESPONSE_BYTES: usize = 256 * 1024;
pub const MAX_FORECAST_DAYS: usize = 7;
const SEARCH_RESULTS: usize = 5;
const MAX_JSON_DEPTH: usize = 32;
const QUERY_MAX_CHARS: usize = 100;
const FORECAST_BASE: &str = "https://api.open-meteo.com/v1/forecast";
const GEOCODE_BASE: &str = "https://geocoding-api.open-meteo.com/v1/search";

/// Open-Meteo provider. See the module docs for licensing.
#[derive(Debug, Clone)]
pub struct OpenMeteo<H: HttpGet>(pub H);

impl<H: HttpGet> WeatherProvider for OpenMeteo<H> {
    fn forecast(&self, location: &WeatherLocation) -> Result<WeatherSnapshot, WeatherError> {
        let body = self.0.get(&forecast_url(location))?;
        parse_forecast(&body, location.clone(), now_unix())
    }

    fn search(&self, query: &str) -> Result<Vec<WeatherLocation>, WeatherError> {
        parse_search(&self.0.get(&search_url(validate_query(query)?))?)
    }
}

fn validate_query(raw: &str) -> Result<&str, WeatherError> {
    let query = raw.trim();
    if query.is_empty()
        || query.chars().count() > QUERY_MAX_CHARS
        || query.chars().any(char::is_control)
    {
        return Err(WeatherError::InvalidQuery);
    }
    Ok(query)
}

#[must_use]
pub fn forecast_url(location: &WeatherLocation) -> String {
    format!(
        "{FORECAST_BASE}?latitude={}&longitude={}&current=temperature_2m,weather_code\
         &daily=weather_code,temperature_2m_max,temperature_2m_min&forecast_days={MAX_FORECAST_DAYS}&timezone=auto",
        location.latitude, location.longitude
    )
}

#[must_use]
pub fn search_url(query: &str) -> String {
    let encoded: String = query
        .bytes()
        .map(|b| match b {
            b if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') => {
                char::from(b).to_string()
            }
            b => format!("%{b:02X}"),
        })
        .collect();
    format!("{GEOCODE_BASE}?name={encoded}&count={SEARCH_RESULTS}")
}

pub fn parse_forecast(
    body: &str,
    location: WeatherLocation,
    fetched_at: u64,
) -> Result<WeatherSnapshot, WeatherError> {
    let root = json::parse(body)?;
    let current = root.field("current")?;
    let temperature_c = celsius(current.field("temperature_2m")?)?;
    let condition = WeatherCondition::from_wmo_code(wmo_code(current.field("weather_code")?)?);

    let daily = root.field("daily")?;
    let dates = array(daily.field("time")?)?;
    let codes = array(daily.field("weather_code")?)?;
    let maxes = array(daily.field("temperature_2m_max")?)?;
    let mins = array(daily.field("temperature_2m_min")?)?;
    let count = dates.len();
    if !(1..=MAX_FORECAST_DAYS).contains(&count)
        || [codes.len(), maxes.len(), mins.len()] != [count; 3]
    {
        return Err(WeatherError::Parse("day count"));
    }

    let mut days = Vec::with_capacity(count);
    for (((date, code), max), min) in dates.iter().zip(codes).zip(maxes).zip(mins) {
        let date = text(date)?;
        if !valid_date(date) {
            return Err(WeatherError::Parse("bad date"));
        }
        let (min_c, max_c) = (celsius(min)?, celsius(max)?);
        if min_c > max_c {
            return Err(WeatherError::Parse("temperature order"));
        }
        let condition = WeatherCondition::from_wmo_code(wmo_code(code)?);
        days.push(DayForecast {
            date: date.to_owned(),
            condition,
            min_c,
            max_c,
        });
    }
    Ok(WeatherSnapshot {
        location,
        temperature_c,
        condition,
        days,
        fetched_at,
    })
}

/// A response without a `results` key means no matches.
pub fn parse_search(body: &str) -> Result<Vec<WeatherLocation>, WeatherError> {
    let root = json::parse(body)?;
    let items = match root.optional("results")? {
        None | Some(json::Json::Null) => return Ok(Vec::new()),
        Some(value) => array(value)?,
    };
    items
        .iter()
        .take(SEARCH_RESULTS)
        .map(|item| -> Result<WeatherLocation, WeatherError> {
            let country = match item.optional("country")? {
                None | Some(json::Json::Null) => None,
                Some(value) => Some(text(value)?),
            };
            let name = text(item.field("name")?)?;
            let latitude = number(item.field("latitude")?)?;
            let longitude = number(item.field("longitude")?)?;
            WeatherLocation::new(name, latitude, longitude, country)
                .map_err(|_| WeatherError::Parse("invalid location"))
        })
        .collect()
}

fn number(value: &json::Json) -> Result<f64, WeatherError> {
    match value {
        json::Json::Num(n) if n.is_finite() => Ok(*n),
        json::Json::Num(_) => Err(WeatherError::Parse("value out of range")),
        _ => Err(WeatherError::Parse("wrong type")),
    }
}

fn celsius(value: &json::Json) -> Result<i32, WeatherError> {
    let n = number(value)?;
    if !(-100.0..=100.0).contains(&n) {
        return Err(WeatherError::Parse("value out of range"));
    }
    Ok(n.round() as i32)
}

fn wmo_code(value: &json::Json) -> Result<u8, WeatherError> {
    let n = number(value)?;
    if n.fract() != 0.0 || !(0.0..=99.0).contains(&n) {
        return Err(WeatherError::Parse("value out of range"));
    }
    Ok(n as u8)
}

fn text(value: &json::Json) -> Result<&str, WeatherError> {
    match value {
        json::Json::Str(s) => Ok(s),
        _ => Err(WeatherError::Parse("wrong type")),
    }
}

fn array(value: &json::Json) -> Result<&[json::Json], WeatherError> {
    match value {
        json::Json::Arr(items) => Ok(items),
        _ => Err(WeatherError::Parse("wrong type")),
    }
}

fn valid_date(s: &str) -> bool {
    let shape = s.len() == 10
        && s.bytes().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        });
    shape
        && (1..=12).contains(&s[5..7].parse::<u32>().unwrap_or(0))
        && (1..=31).contains(&s[8..10].parse::<u32>().unwrap_or(0))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Runs the system curl binary directly (no shell), bounded by curl's own
/// time limits and by [`MAX_RESPONSE_BYTES`]. Stderr is discarded.
#[derive(Debug, Clone, Copy, Default)]
pub struct CurlFetcher;

impl HttpGet for CurlFetcher {
    fn get(&self, url: &str) -> Result<String, WeatherError> {
        let mut child = Command::new("curl")
            .args(["-sS", "--fail", "--max-time", "8", "--connect-timeout", "4"])
            .args(["--proto", "=https"])
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| WeatherError::Transport)?;
        let mut buffer = Vec::new();
        let limit = MAX_RESPONSE_BYTES as u64 + 1;
        let read = match child.stdout.take() {
            Some(out) => out.take(limit).read_to_end(&mut buffer).is_ok(),
            None => false,
        };
        if !read || buffer.len() > MAX_RESPONSE_BYTES {
            let _ = child.kill();
            let _ = child.wait();
            return Err(if read {
                WeatherError::TooLarge
            } else {
                WeatherError::Transport
            });
        }
        if !child.wait().map_err(|_| WeatherError::Transport)?.success() {
            return Err(WeatherError::Transport);
        }
        String::from_utf8(buffer).map_err(|_| WeatherError::Parse("not utf-8"))
    }
}

pub(crate) mod json {
    use super::{MAX_JSON_DEPTH, WeatherError};

    #[derive(Debug, Clone, PartialEq)]
    pub enum Json {
        Null,
        Bool(bool),
        Num(f64),
        Str(String),
        Arr(Vec<Json>),
        Obj(Vec<(String, Json)>),
    }

    impl Json {
        pub fn optional(&self, key: &str) -> Result<Option<&Json>, WeatherError> {
            match self {
                Json::Obj(fields) => Ok(fields.iter().find(|(k, _)| k == key).map(|(_, v)| v)),
                _ => Err(WeatherError::Parse("wrong type")),
            }
        }

        pub fn field(&self, key: &str) -> Result<&Json, WeatherError> {
            self.optional(key)?
                .ok_or(WeatherError::Parse("missing field"))
        }
    }

    fn malformed() -> WeatherError {
        WeatherError::Parse("malformed json")
    }

    pub fn parse(src: &str) -> Result<Json, WeatherError> {
        let mut p = Parser { src, pos: 0 };
        let value = p.value(0)?;
        p.ws();
        if p.pos == src.len() {
            Ok(value)
        } else {
            Err(malformed())
        }
    }

    struct Parser<'a> {
        src: &'a str,
        pos: usize,
    }

    impl Parser<'_> {
        fn peek(&self) -> Option<u8> {
            self.src.as_bytes().get(self.pos).copied()
        }

        fn bump(&mut self) -> Option<char> {
            let c = self.src[self.pos..].chars().next()?;
            self.pos += c.len_utf8();
            Some(c)
        }

        fn eat(&mut self, byte: u8) -> bool {
            let hit = self.peek() == Some(byte);
            self.pos += usize::from(hit);
            hit
        }

        fn ws(&mut self) {
            while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
                self.pos += 1;
            }
        }

        fn word(&mut self, lit: &str, value: Json) -> Result<Json, WeatherError> {
            if !self.src[self.pos..].starts_with(lit) {
                return Err(malformed());
            }
            self.pos += lit.len();
            Ok(value)
        }

        fn value(&mut self, depth: usize) -> Result<Json, WeatherError> {
            if depth > MAX_JSON_DEPTH {
                return Err(malformed());
            }
            self.ws();
            match self.peek() {
                Some(b'{') => self.container(depth, true),
                Some(b'[') => self.container(depth, false),
                Some(b'"') => self.string().map(Json::Str),
                Some(b't') => self.word("true", Json::Bool(true)),
                Some(b'f') => self.word("false", Json::Bool(false)),
                Some(b'n') => self.word("null", Json::Null),
                Some(b'-' | b'0'..=b'9') => self.number(),
                _ => Err(malformed()),
            }
        }

        fn container(&mut self, depth: usize, object: bool) -> Result<Json, WeatherError> {
            let close = if object { b'}' } else { b']' };
            self.pos += 1;
            let (mut fields, mut items) = (Vec::new(), Vec::new());
            self.ws();
            if !self.eat(close) {
                loop {
                    let mut key = None;
                    if object {
                        self.ws();
                        if self.peek() != Some(b'"') {
                            return Err(malformed());
                        }
                        key = Some(self.string()?);
                        self.ws();
                        if !self.eat(b':') {
                            return Err(malformed());
                        }
                    }
                    let value = self.value(depth + 1)?;
                    match key {
                        Some(key) => fields.push((key, value)),
                        None => items.push(value),
                    }
                    self.ws();
                    if self.eat(close) {
                        break;
                    }
                    if !self.eat(b',') {
                        return Err(malformed());
                    }
                }
            }
            Ok(if object {
                Json::Obj(fields)
            } else {
                Json::Arr(items)
            })
        }

        fn string(&mut self) -> Result<String, WeatherError> {
            self.pos += 1;
            let mut out = String::new();
            loop {
                match self.bump().ok_or_else(malformed)? {
                    '"' => return Ok(out),
                    '\\' => out.push(self.escape()?),
                    c if (c as u32) < 0x20 => return Err(malformed()),
                    c => out.push(c),
                }
            }
        }

        fn escape(&mut self) -> Result<char, WeatherError> {
            let c = self.bump().ok_or_else(malformed)?;
            Ok(match c {
                '"' | '\\' | '/' => c,
                'b' => '\u{8}',
                'f' => '\u{c}',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                'u' => {
                    let mut code = 0u32;
                    for _ in 0..4 {
                        let digit = self.bump().and_then(|h| h.to_digit(16));
                        code = code * 16 + digit.ok_or_else(malformed)?;
                    }
                    char::from_u32(code).ok_or_else(malformed)?
                }
                _ => return Err(malformed()),
            })
        }

        fn digits(&mut self) -> Result<(), WeatherError> {
            let start = self.pos;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
            if self.pos == start {
                return Err(malformed());
            }
            Ok(())
        }

        fn number(&mut self) -> Result<Json, WeatherError> {
            let start = self.pos;
            self.eat(b'-');
            self.digits()?;
            if self.eat(b'.') {
                self.digits()?;
            }
            if self.eat(b'e') || self.eat(b'E') {
                if !self.eat(b'+') {
                    self.eat(b'-');
                }
                self.digits()?;
            }
            self.src[start..self.pos]
                .parse::<f64>()
                .map(Json::Num)
                .map_err(|_| malformed())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    const FORECAST_FIXTURE: &str = r#"{"latitude":52.52,"longitude":13.42,
        "current":{"time":"2026-10-16T12:00","temperature_2m":12.4,"weather_code":3},
        "daily":{"time":["2026-10-16","2026-10-17","2026-10-18","2026-10-19","2026-10-20","2026-10-21","2026-10-22"],
        "weather_code":[3,61,71,0,45,95,2],
        "temperature_2m_max":[14.1,11.0,5.5,16.2,9.0,12.3,13.8],
        "temperature_2m_min":[6.2,4.4,-1.9,3.0,0.5,5.1,4.0]}}"#;

    struct MockHttp {
        response: Result<String, WeatherError>,
        requests: RefCell<Vec<String>>,
    }

    impl MockHttp {
        fn new(response: Result<String, WeatherError>) -> Self {
            Self {
                response,
                requests: RefCell::new(Vec::new()),
            }
        }
    }

    impl HttpGet for MockHttp {
        fn get(&self, url: &str) -> Result<String, WeatherError> {
            self.requests.borrow_mut().push(url.to_owned());
            self.response.clone()
        }
    }

    fn berlin() -> WeatherLocation {
        WeatherLocation::new("Berlin", 52.52, 13.41, Some("Germany")).unwrap()
    }

    fn parse(body: &str) -> Result<WeatherSnapshot, WeatherError> {
        parse_forecast(body, berlin(), 0)
    }

    #[test]
    fn forecast_fixture_parses() {
        let snapshot = parse_forecast(FORECAST_FIXTURE, berlin(), 1234).unwrap();
        assert_eq!(snapshot.temperature_c, 12);
        assert_eq!(snapshot.condition, WeatherCondition::Overcast);
        assert_eq!(snapshot.fetched_at, 1234);
        assert_eq!(snapshot.days.len(), 7);
        assert_eq!(snapshot.days[0].date, "2026-10-16");
        assert_eq!(snapshot.days[0].min_c, 6);
        assert_eq!(snapshot.days[0].max_c, 14);
        assert_eq!(snapshot.days[1].condition, WeatherCondition::Rain);
        assert_eq!(snapshot.days[2].min_c, -2);
        assert_eq!(snapshot.days[5].condition, WeatherCondition::Storm);
    }

    #[test]
    fn forecast_rejects_truncated_json() {
        let truncated = &FORECAST_FIXTURE[..FORECAST_FIXTURE.len() - 10];
        assert_eq!(parse(truncated), Err(WeatherError::Parse("malformed json")));
        assert_eq!(parse(""), Err(WeatherError::Parse("malformed json")));
        assert_eq!(
            parse_forecast(&format!("{FORECAST_FIXTURE} x"), berlin(), 0),
            Err(WeatherError::Parse("malformed json"))
        );
    }

    #[test]
    fn forecast_rejects_missing_fields() {
        let no_daily = r#"{"current":{"temperature_2m":1.0,"weather_code":0}}"#;
        let no_temp = r#"{"current":{"weather_code":0},"daily":{"time":[],"weather_code":[],"temperature_2m_max":[],"temperature_2m_min":[]}}"#;
        assert_eq!(parse(no_daily), Err(WeatherError::Parse("missing field")));
        assert_eq!(parse(no_temp), Err(WeatherError::Parse("missing field")));
    }

    #[test]
    fn forecast_rejects_wrong_types() {
        let temp_string = FORECAST_FIXTURE.replace("12.4", "\"12.4\"");
        let code_string =
            FORECAST_FIXTURE.replace("\"weather_code\":3}", "\"weather_code\":\"3\"}");
        let time_not_array = FORECAST_FIXTURE.replace(
            "\"time\":[\"2026-10-16\",",
            "\"time\":\"2026-10-16\",\"x\":[",
        );
        let root_array = "[1,2,3]";
        for body in [
            temp_string.as_str(),
            code_string.as_str(),
            time_not_array.as_str(),
            root_array,
        ] {
            assert!(matches!(parse(body), Err(WeatherError::Parse(_))));
        }
        assert_eq!(parse(&temp_string), Err(WeatherError::Parse("wrong type")));
    }

    #[test]
    fn forecast_rejects_huge_and_fractional_numbers() {
        let huge_temp = FORECAST_FIXTURE.replace("12.4", "1e400");
        let big_temp = FORECAST_FIXTURE.replace("12.4", "1e30");
        let big_code =
            FORECAST_FIXTURE.replace("\"weather_code\":3}", "\"weather_code\":1000000000}");
        let frac_code = FORECAST_FIXTURE.replace("\"weather_code\":3}", "\"weather_code\":2.5}");
        let neg_code = FORECAST_FIXTURE.replace("\"weather_code\":3}", "\"weather_code\":-1}");
        for body in [huge_temp, big_temp, big_code.clone(), frac_code, neg_code] {
            assert!(parse(&body).is_err());
        }
        assert_eq!(
            parse(&big_code),
            Err(WeatherError::Parse("value out of range"))
        );
    }

    #[test]
    fn forecast_rejects_bad_day_counts_dates_and_order() {
        let eight_days =
            FORECAST_FIXTURE.replace("\"2026-10-22\"]", "\"2026-10-22\",\"2026-10-23\"]");
        let mismatch = FORECAST_FIXTURE.replace("[3,61,71,0,45,95,2]", "[3,61]");
        let bad_date = FORECAST_FIXTURE.replace("2026-10-17", "2026-13-17");
        let inverted = FORECAST_FIXTURE.replace("[14.1,", "[-50.0,");
        assert_eq!(parse(&eight_days), Err(WeatherError::Parse("day count")));
        assert_eq!(parse(&mismatch), Err(WeatherError::Parse("day count")));
        assert_eq!(parse(&bad_date), Err(WeatherError::Parse("bad date")));
        assert_eq!(
            parse(&inverted),
            Err(WeatherError::Parse("temperature order"))
        );
    }

    #[test]
    fn search_parses_results_and_empty_responses() {
        let body = r#"{"results":[{"name":"Berlin","latitude":52.52,"longitude":13.41,"country":"Germany","id":1},
            {"name":"Bern","latitude":46.9,"longitude":7.44}]}"#;
        let found = parse_search(body).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0], berlin());
        assert_eq!(found[1].country, None);
        assert_eq!(parse_search("{}").unwrap(), Vec::new());
        assert_eq!(parse_search(r#"{"results":null}"#).unwrap(), Vec::new());
        assert_eq!(
            parse_search(r#"{"results":[{"name":"X","longitude":1.0}]}"#),
            Err(WeatherError::Parse("missing field"))
        );
        assert_eq!(
            parse_search(r#"{"results":[{"name":"X","latitude":99.0,"longitude":1.0}]}"#),
            Err(WeatherError::Parse("invalid location"))
        );
    }

    #[test]
    fn urls_are_built_and_query_is_encoded() {
        let url = forecast_url(&berlin());
        assert!(
            url.starts_with(
                "https://api.open-meteo.com/v1/forecast?latitude=52.52&longitude=13.41"
            )
        );
        assert!(url.contains("forecast_days=7"));
        assert_eq!(
            search_url("São Paulo"),
            "https://geocoding-api.open-meteo.com/v1/search?name=S%C3%A3o%20Paulo&count=5"
        );
        assert_eq!(validate_query("  "), Err(WeatherError::InvalidQuery));
        assert_eq!(
            validate_query(&"q".repeat(QUERY_MAX_CHARS + 1)),
            Err(WeatherError::InvalidQuery)
        );
    }

    #[test]
    fn provider_uses_injected_http_only() {
        let provider = OpenMeteo(MockHttp::new(Ok(FORECAST_FIXTURE.to_owned())));
        let snapshot = provider.forecast(&berlin()).unwrap();
        assert_eq!(snapshot.days.len(), 7);
        assert_eq!(provider.0.requests.borrow().len(), 1);

        let provider = OpenMeteo(MockHttp::new(Ok(r#"{"results":[]}"#.to_owned())));
        assert_eq!(provider.search("Berlin").unwrap(), Vec::new());
        assert!(provider.0.requests.borrow()[0].contains("name=Berlin"));

        let failing = OpenMeteo(MockHttp::new(Err(WeatherError::Transport)));
        assert_eq!(failing.forecast(&berlin()), Err(WeatherError::Transport));
    }
}
