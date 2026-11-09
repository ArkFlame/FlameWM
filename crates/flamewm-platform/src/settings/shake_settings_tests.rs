use super::*;

fn load(text: &str) -> FlameResult<ProductSettings> {
    ConfigStore::default().parse_snapshot(text)
}

#[test]
fn shake_to_locate_defaults_off_and_is_valid() {
    let d = ProductSettings::default();
    assert!(!d.shake_to_locate);
    d.validate().expect("default is valid");
}

#[test]
fn default_shake_to_locate_is_not_written() {
    let text = ConfigStore::serialize(&ProductSettings::default());
    assert!(!text.contains("shakeToLocate"), "default written: {text}");
}

#[test]
fn shake_to_locate_parses_and_round_trips() {
    let parsed = load("revision=2\nshakeToLocate=1\n").expect("valid file");
    assert!(parsed.shake_to_locate);
    let text = ConfigStore::serialize(&parsed);
    assert!(text.contains("shakeToLocate=true"), "{text}");
    assert_eq!(load(&text).expect("reload"), parsed);
}

#[test]
fn invalid_shake_to_locate_values_are_rejected() {
    for value in ["maybe", "2", "on", ""] {
        let text = format!("revision=2\nshakeToLocate={value}\n");
        let error = load(&text).expect_err(&text);
        assert_eq!(error.code, ErrorCode::InvalidArgument, "{text}");
    }
}
