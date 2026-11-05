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
