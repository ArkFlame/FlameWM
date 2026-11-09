//! Progressive-disclosure switch groups (`docs/SETTINGS_PROGRESSIVE_DISCLOSURE.md`).
//!
//! One static table is the single owner of which keys belong to which `Custom <group>` switch and
//! of each member's default. Switch off: every member resolves to its default while its stored
//! value is kept. Switch on: the stored value applies.

use crate::settings::{DEFAULT_ACCENT, SettingValue, SettingsSnapshot};

/// Default of one member key, in the same wire shape `to_api_snapshot` emits for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupDefault {
    Boolean(bool),
    Integer(i64),
    Percent(u8),
    Text(&'static str),
}

impl GroupDefault {
    #[must_use]
    pub fn to_value(self) -> SettingValue {
        match self {
            Self::Boolean(value) => SettingValue::Boolean(value),
            Self::Integer(value) => SettingValue::Integer(value),
            Self::Percent(value) => SettingValue::Percent(value),
            Self::Text(value) => SettingValue::Text(value.to_owned()),
        }
    }
}

/// One switch group. `name` is the group id used by [`is_group_custom`]; `switch` is the boolean
/// key stored in the registry; `members` are the controls the switch hides and defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwitchGroup {
    pub name: &'static str,
    pub switch: &'static str,
    pub members: &'static [(&'static str, GroupDefault)],
}

use GroupDefault::{Boolean as B, Integer as I, Percent as P, Text as T};

/// Single owner of the default translucent surface: neutral black, 75% opacity, no tint. Every
/// `surfaceColor` / `surfaceOpacity` / `glassOpacityPercent` default reads these.
pub const DEFAULT_SURFACE_COLOR: &str = "#000000";
pub const DEFAULT_SURFACE_OPACITY: u8 = 75;
pub const DEFAULT_GLASS_OPACITY: u8 = 75;

/// Every switch group. Key names are the persisted registry keys; a key appears in at most one
/// group. `customWindowOutline` is the existing window-outline switch.
pub static SWITCH_GROUPS: &[SwitchGroup] = &[
    SwitchGroup {
        name: "accent",
        switch: "accentCustom",
        members: &[("accent", T(DEFAULT_ACCENT))],
    },
    SwitchGroup {
        name: "outline",
        switch: "customWindowOutline",
        members: &[
            ("windowOutlineColor", T("#999999")),
            ("windowOutlineOpacity", P(50)),
            ("windowOutlineWidth", I(1)),
        ],
    },
    SwitchGroup {
        name: "surface",
        switch: "surfaceCustom",
        members: &[
            ("outlines", B(true)),
            ("surfaceColor", T(DEFAULT_SURFACE_COLOR)),
            ("surfaceOpacity", P(DEFAULT_SURFACE_OPACITY)),
            ("surfaceOutlineColor", T("#999999")),
            ("surfaceOutlineOpacity", P(50)),
            ("surfaceOutlineWidth", I(1)),
        ],
    },
    SwitchGroup {
        name: "glass",
        switch: "glassCustom",
        members: &[
            ("reduceTransparency", B(false)),
            ("glassOpacityPercent", P(DEFAULT_GLASS_OPACITY)),
            ("glassBlurPx", P(28)),
        ],
    },
    SwitchGroup {
        name: "cursor",
        switch: "cursorCustom",
        members: &[
            ("cursorTheme", T("")),
            ("cursorSize", I(24)),
            ("pointerSpeed", I(0)),
            ("pointerRawInput", B(false)),
        ],
    },
    SwitchGroup {
        name: "minimizeMaximize",
        switch: "minimizeMaximizeCustom",
        members: &[
            ("minimizeAnimation", T("none")),
            ("minimizeAnimationEnabled", B(true)),
            ("maximizeAnimation", T("none")),
            ("maximizeAnimationEnabled", B(false)),
        ],
    },
    SwitchGroup {
        name: "lockBackground",
        switch: "lockBackgroundCustom",
        members: &[
            ("lockBackgroundMode", T("default")),
            ("lockBackgroundColor", T("#232629")),
            ("lockBackgroundImage", T("")),
            ("lockBackgroundBlurPercent", I(40)),
            ("lockBackgroundDimPercent", I(25)),
        ],
    },
    SwitchGroup {
        name: "lockClock",
        switch: "lockClockCustom",
        members: &[
            ("lockClockFormat", T("24h")),
            ("lockShowDate", B(true)),
            ("lockShowUsername", B(true)),
            ("avatarImage", T("")),
        ],
    },
    SwitchGroup {
        name: "screensaver",
        switch: "screensaverCustom",
        members: &[
            ("screensaverMode", T("ribbons")),
            ("screensaverAfterLockMinutes", I(1)),
            ("screensaverDisplayOffMinutes", I(20)),
        ],
    },
    SwitchGroup {
        name: "windows",
        switch: "windowsCustom",
        members: &[
            ("focusFollowsMouse", B(false)),
            ("titlebars", B(true)),
            ("showContentWhileDragging", B(false)),
            ("windowMargin", I(2)),
            ("snapPreviewEnabled", B(true)),
        ],
    },
    SwitchGroup {
        name: "panel",
        switch: "panelCustom",
        members: &[
            ("taskbarHeight", I(44)),
            ("taskbarStyle", T("classic")),
            ("taskbarSeparate", B(false)),
        ],
    },
    SwitchGroup {
        name: "items",
        switch: "itemsCustom",
        members: &[
            ("taskbarShowWeather", B(true)),
            ("taskbarShowVolume", B(true)),
            ("taskbarShowNetwork", B(true)),
            ("taskbarShowBattery", B(true)),
            ("taskbarBatteryOnlyWhenRelevant", B(false)),
            ("taskbarShowClock", B(true)),
            ("taskbarShowWorkspaces", B(true)),
            ("taskbarShowDesktopButton", B(true)),
        ],
    },
    SwitchGroup {
        name: "fonts",
        switch: "fontsCustom",
        members: &[
            ("fontFamily", T("IBM Plex Sans")),
            ("fontBold", B(false)),
            ("textShadow", B(true)),
            ("fontSizeOffset", I(0)),
        ],
    },
];

/// Every registry key that is a group switch, in table order.
pub fn switch_keys() -> impl Iterator<Item = &'static str> {
    SWITCH_GROUPS.iter().map(|group| group.switch)
}

/// The group a key belongs to, as a member or as its switch. `None` for single controls.
#[must_use]
pub fn group_of(key: &str) -> Option<&'static SwitchGroup> {
    SWITCH_GROUPS
        .iter()
        .find(|group| group.switch == key || group.members.iter().any(|(member, _)| *member == key))
}

/// The default of a member key, or `None` when the key is not a group member.
#[must_use]
pub fn member_default(key: &str) -> Option<SettingValue> {
    group_of(key)?
        .members
        .iter()
        .find(|(member, _)| *member == key)
        .map(|(_, default)| default.to_value())
}

/// Global surface key -> per-dock override key. The dock paints each global key through
/// [`dock_surface_value`]: the override while `dock.surfaceCustom` is on, otherwise the global value.
pub const DOCK_SURFACE_OVERRIDES: &[(&str, &str)] = &[
    ("surfaceColor", "dock.surfaceColor"),
    ("surfaceOpacity", "dock.surfaceOpacity"),
    ("surfaceBlurPercent", "dock.surfaceBlurPercent"),
    ("surfaceOutlineColor", "dock.surfaceOutlineColor"),
    ("surfaceOutlineOpacity", "dock.surfaceOutlineOpacity"),
    ("surfaceOutlineWidth", "dock.surfaceOutlineWidth"),
];

/// The value the dock paints a surface key with. `global_key` is one of [`DOCK_SURFACE_OVERRIDES`]'
/// global keys. While `dock.surfaceCustom` is on, the dock's own key applies; while it is off, or
/// when the dock key is absent, the global value applies. `None` for keys outside the table.
#[must_use]
pub fn dock_surface_value(snapshot: &SettingsSnapshot, global_key: &str) -> Option<SettingValue> {
    let (_, dock_key) = DOCK_SURFACE_OVERRIDES
        .iter()
        .find(|(global, _)| *global == global_key)?;
    let custom = switch_on(snapshot, "dock.surfaceCustom");
    if custom && let Some(value) = snapshot.values.get(*dock_key) {
        return Some(value.clone());
    }
    snapshot.values.get(global_key).cloned()
}

/// True when the group's switch is on. An absent switch is off; an unknown group is off.
#[must_use]
pub fn is_group_custom(snapshot: &SettingsSnapshot, group: &str) -> bool {
    SWITCH_GROUPS
        .iter()
        .find(|candidate| candidate.name == group)
        .is_some_and(|group| switch_on(snapshot, group.switch))
}

fn switch_on(snapshot: &SettingsSnapshot, switch: &str) -> bool {
    matches!(
        snapshot.values.get(switch),
        Some(SettingValue::Boolean(true))
    )
}

/// The value a reader must use for `key`. A group member is its default while its group switch is
/// off and its stored value while on. Every other key, including switches, returns its stored
/// value. Idempotent: resolving an already-effective snapshot changes nothing.
#[must_use]
pub fn effective_value(snapshot: &SettingsSnapshot, key: &str) -> Option<SettingValue> {
    match group_of(key) {
        Some(group) if group.switch != key && !switch_on(snapshot, group.switch) => {
            member_default(key)
        }
        _ => snapshot.values.get(key).cloned(),
    }
}

#[cfg(test)]
mod effective_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_switch_is_a_boolean_registry_name_and_no_key_is_in_two_groups() {
        let mut seen = std::collections::BTreeSet::new();
        for group in SWITCH_GROUPS {
            assert!(seen.insert(group.switch), "{} duplicated", group.switch);
            for (member, _) in group.members {
                assert!(seen.insert(member), "{member} is in two groups");
            }
        }
    }

    #[test]
    fn keyboard_layout_is_an_independent_setting_never_reset_by_a_switch() {
        assert!(group_of("keyboardLayout").is_none());
        assert!(member_default("keyboardLayout").is_none());
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot
            .values
            .insert("cursorCustom".into(), SettingValue::Boolean(false));
        snapshot
            .values
            .insert("keyboardLayout".into(), SettingValue::Text("fr".into()));
        assert_eq!(
            effective_value(&snapshot, "keyboardLayout"),
            Some(SettingValue::Text("fr".into()))
        );
    }

    #[test]
    fn group_of_finds_members_and_switches_only() {
        assert_eq!(group_of("surfaceOpacity").unwrap().name, "surface");
        assert_eq!(group_of("surfaceCustom").unwrap().name, "surface");
        assert!(group_of("weatherUnits").is_none());
    }

    #[test]
    fn effective_value_defaults_when_switch_off_and_keeps_stored_when_on() {
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot
            .values
            .insert("surfaceOpacity".into(), SettingValue::Percent(10));
        assert_eq!(
            effective_value(&snapshot, "surfaceOpacity"),
            Some(SettingValue::Percent(DEFAULT_SURFACE_OPACITY))
        );
        snapshot
            .values
            .insert("surfaceCustom".into(), SettingValue::Boolean(true));
        assert_eq!(
            effective_value(&snapshot, "surfaceOpacity"),
            Some(SettingValue::Percent(10))
        );
        assert!(is_group_custom(&snapshot, "surface"));
        assert!(!is_group_custom(&snapshot, "glass"));
    }
}

#[cfg(test)]
mod translucent_default_tests {
    use super::*;

    /// The translucent default is one owner: black at 75%, reduceTransparency off, and the glass
    /// members follow the same table so hidden groups resolve to the same translucent material.
    #[test]
    fn surface_defaults_are_black_75_percent_translucent() {
        assert_eq!(
            member_default("surfaceColor"),
            Some(SettingValue::Text("#000000".to_owned()))
        );
        assert_eq!(
            member_default("surfaceOpacity"),
            Some(SettingValue::Percent(75))
        );
        assert_eq!(
            member_default("glassOpacityPercent"),
            Some(SettingValue::Percent(75))
        );
        assert_eq!(
            member_default("reduceTransparency"),
            Some(SettingValue::Boolean(false))
        );
    }

    #[test]
    fn glass_group_defaults_to_translucent_and_members_resolve_when_hidden() {
        let snapshot = SettingsSnapshot::new(1);
        assert_eq!(
            effective_value(&snapshot, "reduceTransparency"),
            Some(SettingValue::Boolean(false))
        );
        assert_eq!(
            effective_value(&snapshot, "surfaceOpacity"),
            Some(SettingValue::Percent(75))
        );
    }

    #[test]
    fn dock_surface_value_follows_global_until_the_dock_switch_is_on() {
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot
            .values
            .insert("surfaceOpacity".into(), SettingValue::Percent(60));
        snapshot
            .values
            .insert("dock.surfaceOpacity".into(), SettingValue::Percent(20));
        assert_eq!(
            dock_surface_value(&snapshot, "surfaceOpacity"),
            Some(SettingValue::Percent(60)),
            "switch absent: global applies"
        );
        snapshot
            .values
            .insert("dock.surfaceCustom".into(), SettingValue::Boolean(true));
        assert_eq!(
            dock_surface_value(&snapshot, "surfaceOpacity"),
            Some(SettingValue::Percent(20)),
            "switch on: dock override applies"
        );
        assert_eq!(dock_surface_value(&snapshot, "accent"), None);
    }
}
