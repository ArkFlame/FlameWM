use super::{ConfigStore, ProductSettings};

fn parse(text: &str) -> ProductSettings {
    ConfigStore::default().parse_snapshot(text).expect("parse")
}

#[test]
fn a_group_member_off_default_without_its_switch_switches_the_group_on() {
    let migrated = parse("revision=1\nsurfaceOpacity=10\n");
    assert!(migrated.surface_custom);
    assert_eq!(migrated.effective().surface_opacity, 10);
}

#[test]
fn a_non_default_outline_toggle_migrates_the_surface_group_on() {
    assert!(parse("revision=1\noutlines=false\n").surface_custom);
}

#[test]
fn a_clean_legacy_config_keeps_every_switch_off() {
    let clean = parse("revision=1\nsurfaceOpacity=75\nlockShowDate=1\n");
    assert!(!clean.surface_custom && !clean.lock_clock_custom);
}

#[test]
fn an_explicit_switch_wins_over_the_migration_rule() {
    let explicit = parse("revision=1\nsurfaceCustom=false\nsurfaceOpacity=10\n");
    assert!(!explicit.surface_custom);
    assert_eq!(explicit.surface_opacity, 10);
}

#[test]
fn migration_is_idempotent_across_save_and_reload() {
    let first = parse("revision=1\nlockClockFormat=12h\n");
    let saved = ConfigStore::serialize(&first);
    let second = parse(&saved);
    assert_eq!(second.lock_clock_custom, first.lock_clock_custom);
    assert_eq!(ConfigStore::serialize(&second), saved);
}
