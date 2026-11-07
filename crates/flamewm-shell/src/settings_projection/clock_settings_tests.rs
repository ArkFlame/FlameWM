use super::clock_display;
use crate::projection::ClockDisplay;
use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_platform::settings::ClockFormat;

#[test]
fn missing_keys_resolve_to_product_clock_defaults() {
    assert_eq!(
        clock_display(&SettingsSnapshot::new(1)),
        ClockDisplay::default()
    );
}

#[test]
fn clock_keys_map_to_display_and_unknown_format_falls_back_to_24h() {
    let mut snapshot = SettingsSnapshot::new(2);
    snapshot.values.insert(
        "taskbarClockFormat".to_owned(),
        SettingValue::Text("12h".to_owned()),
    );
    snapshot
        .values
        .insert("taskbarShowSeconds".to_owned(), SettingValue::Boolean(true));
    snapshot
        .values
        .insert("taskbarShowDate".to_owned(), SettingValue::Boolean(false));
    let display = clock_display(&snapshot);
    assert_eq!(display.format, ClockFormat::Twelve);
    assert!(display.show_seconds);
    assert!(!display.show_date);

    snapshot.values.insert(
        "taskbarClockFormat".to_owned(),
        SettingValue::Text("bogus".to_owned()),
    );
    assert_eq!(clock_display(&snapshot).format, ClockFormat::TwentyFour);
}
