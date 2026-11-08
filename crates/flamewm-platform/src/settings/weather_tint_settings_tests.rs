use super::*;

fn load(text: &str) -> FlameResult<ProductSettings> {
    ConfigStore::default().parse_snapshot(text)
}

// OBSERVABLE: the tint is on by default and the default is not written to the file.
// MUTATION: a default of false, or serializing the default, fails here.
#[test]
fn weather_tint_defaults_on_and_is_not_written_at_default() {
    let defaults = ProductSettings::default();
    assert!(defaults.weather_tint);
    let text = ConfigStore::serialize(&defaults);
    assert!(!text.contains("weatherTint"), "default must not be written");
}

// OBSERVABLE: an explicit off survives a round trip; the explicit on is accepted and dropped.
#[test]
fn weather_tint_round_trips_off_and_drops_explicit_on() {
    let off = load("revision=2\nweatherTint=false\n").expect("explicit off");
    assert!(!off.weather_tint);
    let text = ConfigStore::serialize(&off);
    assert!(text.contains("weatherTint=false\n"));
    assert_eq!(load(&text).expect("round trip"), off);

    let on = load("revision=2\nweatherTint=true\n").expect("explicit on");
    assert!(on.weather_tint);
    assert!(!ConfigStore::serialize(&on).contains("weatherTint"));
}

// OBSERVABLE: values follow the shared bool convention (1/yes/true, 0/no/false);
// anything else is rejected, not coerced, and the key is not unknown.
#[test]
fn weather_tint_rejects_invalid_values_and_is_a_typed_row() {
    for alias in ["1", "yes"] {
        assert!(
            load(&format!("revision=2\nweatherTint={alias}\n"))
                .expect("alias")
                .weather_tint
        );
    }
    for bad in ["maybe", "on", ""] {
        assert!(
            load(&format!("revision=2\nweatherTint={bad}\n")).is_err(),
            "weatherTint={bad:?} must be rejected"
        );
    }
    let parsed = load("revision=2\nweatherTint=false\n").expect("valid");
    assert!(
        parsed.unknown_keys.is_empty(),
        "weatherTint must not fall into unknown_keys"
    );
    let stable = super::stable_setting("weatherTint").expect("typed weather tint row");
    assert_eq!(
        (stable.get)(&ProductSettings::default()),
        flamewm_api::settings::SettingValue::Boolean(true)
    );
}
