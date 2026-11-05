use flamewm_api::settings_groups::SWITCH_GROUPS;

use super::{STABLE_SETTINGS, stable_setting};

#[test]
fn every_group_with_more_than_three_members_has_a_registry_switch() {
    for group in SWITCH_GROUPS.iter().filter(|group| group.members.len() > 3) {
        let row = stable_setting(group.switch)
            .unwrap_or_else(|| panic!("{} has no registry switch", group.switch));
        assert_eq!(row.key, group.switch);
    }
}

#[test]
fn every_switch_key_is_a_registry_boolean_defaulting_off() {
    for group in SWITCH_GROUPS {
        assert!(
            STABLE_SETTINGS.iter().any(|row| row.key == group.switch),
            "{} missing from the registry",
            group.switch
        );
    }
    assert_eq!(
        SWITCH_GROUPS.len(),
        13,
        "switch count drifted from the inventory"
    );
}
