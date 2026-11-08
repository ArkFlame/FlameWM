use super::*;

fn berlin() -> WeatherLocation {
    WeatherLocation::new("Berlin", 52.52, 13.41, Some("Germany")).unwrap()
}

fn snapshot() -> WeatherSnapshot {
    WeatherSnapshot {
        location: berlin(),
        temperature_c: -3,
        condition: WeatherCondition::Snow,
        days: vec![
            DayForecast {
                date: "2026-10-16".to_owned(),
                condition: WeatherCondition::Drizzle,
                min_c: -5,
                max_c: 2,
            },
            DayForecast {
                date: "2026-10-17".to_owned(),
                condition: WeatherCondition::Storm,
                min_c: 0,
                max_c: 9,
            },
        ],
        fetched_at: 1_800_000_000,
    }
}

fn feed(state: WeatherState) -> WeatherFeed {
    WeatherFeed {
        settings: WeatherSettings {
            enabled: true,
            location: Some(berlin()),
            units: TemperatureUnit::Fahrenheit,
        },
        state,
    }
}

#[test]
fn every_state_round_trips_through_the_wire() {
    let states = [
        WeatherState::Disabled,
        WeatherState::Unconfigured,
        WeatherState::Loading,
        WeatherState::Ready(snapshot()),
        WeatherState::Offline(Some(snapshot())),
        WeatherState::Offline(None),
        WeatherState::Error(WeatherError::Transport),
        WeatherState::Error(WeatherError::TooLarge),
    ];
    for state in states {
        let original = feed(state);
        let text = original.encode();
        assert!(text.len() < 4000, "feed too long for one protocol line");
        assert_eq!(WeatherFeed::decode(&text), Some(original));
    }
}

#[test]
fn settings_without_location_round_trip() {
    let original = WeatherFeed {
        settings: WeatherSettings::default(),
        state: WeatherState::Disabled,
    };
    assert_eq!(WeatherFeed::decode(&original.encode()), Some(original));
}

#[test]
fn malformed_feed_is_rejected() {
    for bad in [
        "",
        "1\x1fcelsius\x1f\x1fbogus\x1f",
        "2\x1fcelsius\x1f\x1fdisabled\x1f",
        "1\x1fkelvin\x1f\x1fdisabled\x1f",
        "1\x1fcelsius\x1fnot-a-location\x1fdisabled\x1f",
        "1\x1fcelsius\x1f\x1fready\x1fwarm",
    ] {
        assert_eq!(WeatherFeed::decode(bad), None, "accepted {bad:?}");
    }
}
