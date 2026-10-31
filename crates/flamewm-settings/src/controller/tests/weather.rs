use super::*;

#[test]
fn weather_units_segments_write_exact_values_and_highlight_the_snapshot() {
    let (requests, mut transport) = recording_transport();
    transport.responses.extend([
        Ok(ControlResponse::Settings(lock_snapshot(
            1,
            "weatherUnits",
            SettingValue::Text("celsius".to_owned()),
        ))),
        Ok(ControlResponse::Settings(lock_snapshot(
            2,
            "weatherUnits",
            SettingValue::Text("fahrenheit".to_owned()),
        ))),
    ]);
    let mut application = settings_application(transport);
    let mut document = ProbeDocument::default();
    application
        .sync(&mut document)
        .expect("sync initial snapshot");
    assert!(document.highlighted["weather-units-celsius"]);
    assert!(!document.highlighted["weather-units-fahrenheit"]);

    application
        .handle_action(&release("settings.weather.units.fahrenheit"), &mut document)
        .expect("weather units fahrenheit");
    assert!(document.highlighted["weather-units-fahrenheit"]);
    assert!(!document.highlighted["weather-units-celsius"]);
    let written = written_values(&requests, "weatherUnits");
    assert_eq!(written, vec![SettingValue::Text("fahrenheit".to_owned())]);
}

#[test]
fn weather_location_label_decodes_the_saved_city_and_hides_malformed_values() {
    use crate::weather_location_label;
    assert_eq!(weather_location_label("51.5\t-0.12\tGB\tLondon"), "London");
    assert_eq!(
        weather_location_label("51.5\t-0.12\t\tSpringfield"),
        "Springfield"
    );
    let unset = "Not set - click the weather icon in the taskbar";
    assert_eq!(weather_location_label(""), unset);
    assert_eq!(weather_location_label("abc\t-0.12\tGB\tLondon"), unset);
    assert_eq!(weather_location_label("51.5\t-0.12\tGB\t"), unset);
}

#[test]
fn automatic_location_row_is_prefixed_and_manual_row_is_the_bare_city() {
    use crate::{time_zone_label, weather_location_text};
    assert_eq!(
        weather_location_text("51.5\t-0.12\tGB\tLondon", true),
        "Automatic: London, GB"
    );
    assert_eq!(
        weather_location_text("51.5\t-0.12\t\tSpringfield", true),
        "Automatic: Springfield"
    );
    assert_eq!(
        weather_location_text("51.5\t-0.12\tGB\tLondon", false),
        "London"
    );
    assert_eq!(
        weather_location_text("", true),
        "Not set - click the weather icon in the taskbar"
    );
    assert_eq!(time_zone_label(true, "Europe/Paris"), "Europe/Paris");
}

#[test]
fn weather_location_clear_writes_empty_and_shows_the_unset_hint() {
    let (requests, mut transport) = recording_transport();
    transport.responses.extend([
        Ok(ControlResponse::Settings(lock_snapshot(
            1,
            "weatherLocation",
            SettingValue::Text("51.5\t-0.12\tGB\tLondon".to_owned()),
        ))),
        Ok(ControlResponse::Settings(lock_snapshot(
            2,
            "weatherLocation",
            SettingValue::Text(String::new()),
        ))),
    ]);
    let mut application = settings_application(transport);
    let mut document = ProbeDocument::default();
    application
        .sync(&mut document)
        .expect("sync initial snapshot");
    assert_eq!(document.texts["weather-location-value"], "London");

    application
        .handle_action(&release("settings.weather.location.clear"), &mut document)
        .expect("clear weather location");
    assert_eq!(
        document.texts["weather-location-value"],
        "Not set - click the weather icon in the taskbar"
    );
    let written = written_values(&requests, "weatherLocation");
    assert_eq!(written, vec![SettingValue::Text(String::new())]);
}
