use super::*;

fn load(text: &str) -> FlameResult<ProductSettings> {
    ConfigStore::default().parse_snapshot(text)
}

// OBSERVABLE: both providers are on and the refresh is five minutes by default; defaults are not written.
// MUTATION: a default of false or a different interval fails here, or a written default fails the text check.
#[test]
fn usage_defaults_are_both_providers_and_five_minutes_and_not_written() {
    let defaults = ProductSettings::default();
    assert!(defaults.usage_claude && defaults.usage_command_code);
    assert_eq!(defaults.usage_refresh_seconds, 300);
    let text = ConfigStore::serialize(&defaults);
    assert!(
        !text.contains("usage."),
        "defaults must not be written: {text}"
    );
}

// OBSERVABLE: explicit off and a non-default interval survive a round trip.
// MUTATION: dropping either key from the stable table makes the parse or the written text miss it.
#[test]
fn usage_keys_round_trip_off_and_interval() {
    let parsed =
        load("revision=2\nusage.claude=false\nusage.refresh_seconds=900\n").expect("parse");
    assert!(!parsed.usage_claude);
    assert!(parsed.usage_command_code);
    assert_eq!(parsed.usage_refresh_seconds, 900);
    let text = ConfigStore::serialize(&parsed);
    assert!(text.contains("usage.claude=false\n"), "{text}");
    assert!(text.contains("usage.refresh_seconds=900\n"), "{text}");
    assert_eq!(load(&text).expect("round trip"), parsed);
}
