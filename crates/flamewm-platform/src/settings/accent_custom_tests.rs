use super::{ConfigStore, DEFAULT_ACCENT};

fn parse(text: &str) -> super::ProductSettings {
    ConfigStore::default().parse_snapshot(text).expect("parse")
}

#[test]
fn accent_custom_defaults_off_and_uses_system_accent() {
    let settings = parse("revision=1\naccent=#112233\naccentCustom=false\n");
    assert!(!settings.accent_custom);
    assert_eq!(settings.accent, DEFAULT_ACCENT);
}

#[test]
fn explicit_non_default_accent_migrates_to_custom() {
    let settings = parse("revision=1\naccent=#112233\n");
    assert!(settings.accent_custom);
    assert_eq!(settings.accent, "#112233");
}

#[test]
fn default_accent_without_toggle_stays_off() {
    let settings = parse("revision=1\naccent=#539BFF\n");
    assert!(!settings.accent_custom);
    assert_eq!(settings.accent, DEFAULT_ACCENT);
}

#[test]
fn custom_accent_is_kept_when_toggle_is_on() {
    let settings = parse("revision=1\naccent=#112233\naccentCustom=true\n");
    assert!(settings.accent_custom);
    assert_eq!(settings.accent, "#112233");
}
