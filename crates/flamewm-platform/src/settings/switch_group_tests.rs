use flamewm_api::settings::SettingValue;
use flamewm_api::settings_groups::{SWITCH_GROUPS, member_default, switch_keys};

use super::{ConfigStore, ProductSettings, STABLE_SETTINGS};
use crate::services::settings::to_api_snapshot;

#[test]
fn every_switch_and_member_is_a_registry_key_with_the_table_default() {
    let snapshot = to_api_snapshot(&ProductSettings::default());
    for switch in switch_keys() {
        assert_eq!(
            snapshot.values.get(switch),
            Some(&SettingValue::Boolean(false)),
            "{switch} must default off"
        );
    }
    for group in SWITCH_GROUPS {
        for (member, _) in group.members {
            assert!(
                STABLE_SETTINGS.iter().any(|row| row.key == *member),
                "{member} is not in the registry"
            );
            assert_eq!(
                snapshot.values.get(*member).cloned(),
                member_default(member),
                "{member}: table default differs from ProductSettings::default()"
            );
        }
    }
}

#[test]
fn clean_config_loads_every_switch_off() {
    let settings = ConfigStore::default()
        .parse_snapshot("revision=1\n")
        .expect("parse");
    assert!(!settings.surface_custom && !settings.fonts_custom && !settings.items_custom);
}

fn parse(text: &str) -> ProductSettings {
    ConfigStore::default().parse_snapshot(text).expect("parse")
}

#[test]
fn switch_off_keeps_stored_members_through_save_and_reload() {
    let off = parse("revision=1\nsurfaceCustom=0\nsurfaceOpacity=10\n");
    assert!(!off.surface_custom);
    assert_eq!(off.surface_opacity, 10, "stored value kept while off");
    assert_eq!(
        off.effective().surface_opacity,
        65,
        "readers see the default"
    );

    let saved = ConfigStore::serialize(&off);
    assert!(saved.contains("surfaceOpacity=10"), "{saved}");
    let reloaded = parse(&saved);
    assert!(!reloaded.surface_custom, "switch stays off after reload");
    assert_eq!(reloaded.surface_opacity, 10);

    let on = parse(&saved.replace("surfaceCustom=false", "surfaceCustom=true"));
    assert!(on.surface_custom);
    assert_eq!(on.effective().surface_opacity, 10, "turning on restores it");
}

#[test]
fn effective_is_idempotent_and_leaves_stored_values_alone() {
    let stored = parse("revision=1\nsurfaceCustom=0\nlockClockCustom=0\nlockShowDate=0\n");
    let once = stored.effective();
    assert_eq!(once.effective(), once);
    assert!(
        !stored.lock_show_date,
        "effective() must not touch stored state"
    );
    assert!(once.lock_show_date);
}
