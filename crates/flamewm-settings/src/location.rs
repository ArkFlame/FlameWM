//! Split from `lib.rs` (`location`); see crate docs.

pub(crate) const WEATHER_UNSET_HINT: &str = "Not set - click the weather icon in the taskbar";

/// `(country, city)` from the shell's stored `lat\tlon\tcountry\tname` location.
pub(crate) fn decode_weather_location(encoded: &str) -> Option<(String, String)> {
    let mut parts = encoded.splitn(4, '\t');
    let coordinates_ok = [parts.next(), parts.next()]
        .into_iter()
        .all(|value| value.is_some_and(|value| value.parse::<f64>().is_ok()));
    let country = parts.next().unwrap_or_default();
    match parts.next() {
        Some(name) if coordinates_ok && !name.is_empty() => {
            Some((country.to_owned(), name.to_owned()))
        }
        _ => None,
    }
}

/// City name from the stored location, or the unset hint.
pub(crate) fn weather_location_label(encoded: &str) -> String {
    decode_weather_location(encoded).map_or_else(|| WEATHER_UNSET_HINT.to_owned(), |(_, city)| city)
}

/// Location row text; while automatic detection is ON the city is prefixed `Automatic:`.
pub(crate) fn weather_location_text(encoded: &str, automatic: bool) -> String {
    match decode_weather_location(encoded) {
        Some((country, city)) if automatic && country.is_empty() => format!("Automatic: {city}"),
        Some((country, city)) if automatic => format!("Automatic: {city}, {country}"),
        Some((_, city)) => city,
        None => WEATHER_UNSET_HINT.to_owned(),
    }
}

/// Current IANA zone: the IP-lookup cache only while automatic time zone is on, else the system zone.
pub(crate) fn time_zone_label(automatic: bool, cached: &str) -> String {
    pick_time_zone(automatic, cached, &system_time_zone())
}

pub(crate) fn pick_time_zone(automatic: bool, cached: &str, system: &str) -> String {
    if automatic && !cached.is_empty() {
        cached.to_owned()
    } else {
        system.to_owned()
    }
}

pub(crate) fn system_time_zone() -> String {
    std::fs::read_link("/etc/localtime")
        .ok()
        .and_then(|path| {
            path.to_str()
                .and_then(|path| path.strip_prefix("/usr/share/zoneinfo/"))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "unknown".to_owned())
}
