use super::*;

fn load(text: &str) -> FlameResult<ProductSettings> {
    ConfigStore::default().parse_snapshot(text)
}

#[test]
fn accessibility_and_night_mode_defaults_are_neutral() {
    let settings = ProductSettings::default();
    assert!(!settings.high_contrast);
    assert!(!settings.reduced_motion);
    assert_eq!(settings.text_scale_percent, 100);
    assert!(!settings.sticky_keys);
    assert!(!settings.slow_keys);
    assert_eq!(settings.night_shift_mode, NightShiftMode::Manual);
    settings.validate().expect("defaults are valid");
}

#[test]
fn accessibility_rows_form_their_own_reset_section() {
    let keys: Vec<_> = STABLE_SETTINGS
        .iter()
        .filter(|row| row.section == "accessibility")
        .map(|row| row.key)
        .collect();
    assert_eq!(
        keys,
        [
            "highContrast",
            "reducedMotion",
            "textScalePercent",
            "stickyKeys",
            "slowKeys",
            "shakeToLocate",
            "colourFilter",
            "zoomLevel",
        ]
    );
}

#[test]
fn night_shift_mode_parses_its_wire_values() {
    assert_eq!(
        NightShiftMode::parse("manual"),
        Some(NightShiftMode::Manual)
    );
    assert_eq!(
        NightShiftMode::parse("sunset"),
        Some(NightShiftMode::Sunset)
    );
    assert_eq!(NightShiftMode::parse("Sunset"), None);
    assert_eq!(NightShiftMode::Sunset.as_str(), "sunset");
}

#[test]
fn accessibility_and_night_mode_round_trip_through_file_text() {
    let text = "revision=2\nhighContrast=1\nreducedMotion=true\ntextScalePercent=125\n\
                stickyKeys=1\nslowKeys=true\nnightShiftMode=sunset\n";
    let parsed = load(text).expect("valid file");
    assert!(parsed.high_contrast);
    assert!(parsed.reduced_motion);
    assert_eq!(parsed.text_scale_percent, 125);
    assert!(parsed.sticky_keys);
    assert!(parsed.slow_keys);
    assert_eq!(parsed.night_shift_mode, NightShiftMode::Sunset);

    let serialized = ConfigStore::serialize(&parsed);
    assert_eq!(load(&serialized).expect("reload"), parsed);
}

#[test]
fn default_accessibility_and_night_mode_values_are_not_written() {
    let text = ConfigStore::serialize(&ProductSettings::default());
    for key in [
        "highContrast",
        "reducedMotion",
        "textScalePercent",
        "stickyKeys",
        "slowKeys",
        "nightShiftMode",
    ] {
        assert!(!text.contains(key), "{key} written as default: {text}");
    }
}

#[test]
fn text_scale_outside_one_hundred_to_two_hundred_is_rejected_on_load() {
    for value in ["99", "201", "wide"] {
        let text = format!("revision=2\ntextScalePercent={value}\n");
        assert!(load(&text).is_err(), "{value} must be rejected");
    }
    assert_eq!(
        load("revision=2\ntextScalePercent=100\n")
            .expect("lower bound")
            .text_scale_percent,
        100
    );
    assert_eq!(
        load("revision=2\ntextScalePercent=200\n")
            .expect("upper bound")
            .text_scale_percent,
        200
    );
}

#[test]
fn validate_rejects_text_scale_outside_range() {
    for value in [99, 201] {
        let settings = ProductSettings {
            text_scale_percent: value,
            ..ProductSettings::default()
        };
        let error = settings.validate().expect_err("out of range");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }
}

#[test]
fn unknown_night_shift_mode_is_rejected() {
    let error = load("revision=2\nnightShiftMode=dusk\n").expect_err("bad mode");
    assert_eq!(error.code, ErrorCode::InvalidArgument);
}

#[test]
fn sticky_keys_rejects_non_boolean_values() {
    for value in ["maybe", "2", "enabled"] {
        let text = format!("revision=2\nstickyKeys={value}\n");
        assert!(load(&text).is_err(), "stickyKeys={value} must be rejected");
    }
}

#[test]
fn colour_filter_defaults_to_none_and_accepts_its_six_values() {
    assert_eq!(ProductSettings::default().colour_filter, "none");
    assert_eq!(load("revision=2\n").expect("default").colour_filter, "none");
    for value in [
        "none",
        "grayscale",
        "invert",
        "protanopia",
        "deuteranopia",
        "tritanopia",
    ] {
        let settings = load(&format!("revision=2\ncolourFilter={value}\n")).expect(value);
        assert_eq!(settings.colour_filter, value);
    }
    assert!(load("revision=2\ncolourFilter=sepia\n").is_err());
}
