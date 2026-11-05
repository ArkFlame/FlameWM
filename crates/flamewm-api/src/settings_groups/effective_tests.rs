use super::{SWITCH_GROUPS, effective_value, is_group_custom, member_default};
use crate::settings::{SettingValue, SettingsSnapshot};

/// A value of the same wire shape as `default` that differs from it.
fn stored_value(default: &SettingValue) -> SettingValue {
    match default {
        SettingValue::Boolean(value) => SettingValue::Boolean(!value),
        SettingValue::Integer(value) => SettingValue::Integer(value + 1),
        SettingValue::Percent(value) => SettingValue::Percent(value.wrapping_add(1)),
        SettingValue::Text(value) => SettingValue::Text(format!("{value}-stored")),
        other => other.clone(),
    }
}

#[test]
fn every_group_resolves_members_to_default_off_and_stored_on() {
    for group in SWITCH_GROUPS {
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot
            .values
            .insert(group.switch.into(), SettingValue::Boolean(false));
        for (member, _) in group.members {
            let default = member_default(member).expect("member has a default");
            snapshot
                .values
                .insert((*member).into(), stored_value(&default));
        }
        assert!(!is_group_custom(&snapshot, group.name), "{}", group.name);
        for (member, _) in group.members {
            assert_eq!(
                effective_value(&snapshot, member),
                member_default(member),
                "{member} off"
            );
        }

        snapshot
            .values
            .insert(group.switch.into(), SettingValue::Boolean(true));
        assert!(is_group_custom(&snapshot, group.name), "{}", group.name);
        for (member, _) in group.members {
            let stored = snapshot.values.get(*member).cloned();
            assert_eq!(effective_value(&snapshot, member), stored, "{member} on");
        }
    }
}

#[test]
fn effective_value_is_idempotent_on_an_effective_snapshot() {
    let mut snapshot = SettingsSnapshot::new(1);
    snapshot
        .values
        .insert("fontsCustom".into(), SettingValue::Boolean(false));
    snapshot
        .values
        .insert("fontBold".into(), SettingValue::Boolean(true));
    let once = effective_value(&snapshot, "fontBold");
    let mut resolved = snapshot.clone();
    resolved
        .values
        .insert("fontBold".into(), once.clone().expect("resolved"));
    assert_eq!(effective_value(&resolved, "fontBold"), once);
}
