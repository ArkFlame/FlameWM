//! Typed readers over the settings snapshot and the accent-token projection for the desktop.

pub(crate) use flamewm_api::settings::{setting_bool, setting_integer, setting_percent};
use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_ui_x11::{UiColor, UiDocumentAccess};

pub(crate) fn setting_text(snapshot: &SettingsSnapshot, key: &str) -> String {
    match snapshot.values.get(key) {
        Some(SettingValue::Text(value)) => value.clone(),
        _ => String::new(),
    }
}

/// True when the snapshot's `iconTheme` differs from the last applied value.
pub(crate) fn icon_theme_changed(previous: &str, snapshot: &SettingsSnapshot) -> bool {
    setting_text(snapshot, "iconTheme") != previous
}

/// Keys that the wallpaper request reads (`refresh::request_wallpaper`).
pub(crate) const DESKTOP_WALLPAPER_KEYS: [&str; 5] = [
    "wallpaperMode",
    "wallpaper",
    "wallpaperColor",
    "wallpaperFit",
    "shareWallpaper",
];

/// Single keys the desktop reads or projects, directly or through shared projection crates
/// (`ui-document` appearance and surface tokens, widgets, weather, the render acceleration gate).
pub(crate) const DESKTOP_CONSUMED_KEYS: &[&str] = &[
    "accentOpacity",
    "appearance",
    "backdropBlur",
    "backgroundOpacity",
    "cpuColor",
    "dateSize",
    "DesktopEntryScalePercent",
    "DesktopSelectionFillOpacity",
    "fontBold",
    "fontSizeOffset",
    "iconTheme",
    "cursorTheme",
    "inheritSurface",
    "memoryColor",
    "organizeDownloads",
    "reduceTransparency",
    "ringColor",
    "rxColor",
    "stickyNotesEnabled",
    "surfaceBlurPercent",
    "surfaceColor",
    "surfaceOpacity",
    "surfaceOutlineColor",
    "surfaceOutlineOpacity",
    "textColor",
    "textShadow",
    "textSize",
    "timeSize",
    "trackColor",
    "txColor",
    "weatherEnabled",
    "weatherLocation",
    "weatherUnits",
];

/// Switch groups whose switch and members the desktop consumes. The glass, panel, lock and
/// screensaver groups are shell or lock owned and never change a desktop projection.
pub(crate) const DESKTOP_CONSUMED_GROUPS: [&str; 6] =
    ["accent", "outline", "surface", "cursor", "fonts", "items"];

/// Every key whose effective value can change a desktop projection, wallpaper keys included.
pub(crate) fn desktop_consumed_keys() -> Vec<&'static str> {
    let mut keys: Vec<&'static str> = DESKTOP_CONSUMED_KEYS.to_vec();
    keys.extend(DESKTOP_WALLPAPER_KEYS);
    for group in flamewm_api::settings_groups::SWITCH_GROUPS
        .iter()
        .filter(|group| DESKTOP_CONSUMED_GROUPS.contains(&group.name))
    {
        keys.push(group.switch);
        keys.extend(group.members.iter().map(|(member, _)| *member));
    }
    keys
}

/// True when any key in `keys` resolves to a different effective value in `previous` and `next`.
/// Compares `effective_value`, so a group member hidden behind an off switch counts as unchanged.
pub(crate) fn effective_keys_changed(
    previous: &SettingsSnapshot,
    next: &SettingsSnapshot,
    keys: &[&str],
) -> bool {
    keys.iter().any(|key| {
        flamewm_api::settings_groups::effective_value(previous, key)
            != flamewm_api::settings_groups::effective_value(next, key)
    })
}

/// True when the wallpaper request must be re-issued for `next`.
pub(crate) fn wallpaper_settings_changed(
    previous: &SettingsSnapshot,
    next: &SettingsSnapshot,
) -> bool {
    effective_keys_changed(previous, next, &DESKTOP_WALLPAPER_KEYS)
}

/// True when `next` changes anything the desktop projects, so the visual pass must run.
pub(crate) fn desktop_visual_settings_changed(
    previous: &SettingsSnapshot,
    next: &SettingsSnapshot,
) -> bool {
    effective_keys_changed(previous, next, &desktop_consumed_keys())
}

pub(crate) fn desktop_entry_scale(snapshot: &SettingsSnapshot) -> u16 {
    setting_integer(snapshot, "DesktopEntryScalePercent", 100).clamp(50, 200) as u16
}

pub(crate) fn percent_alpha(percent: u8) -> u8 {
    ((u16::from(percent) * 255) / 100) as u8
}

/// Accent-derived tokens of `ui/shell/flamewm.css`, with the alpha each keeps in its `:root`.
pub(crate) const ACCENT_TOKEN_ALPHAS: [(&str, u8); 8] = [
    ("--accent", 0xff),
    ("--accent-soft", 0x38),
    ("--accent-strong", 0xdd),
    ("--desktop-hover", 0x26),
    ("--desktop-hover-border", 0x4d),
    ("--desktop-selected", 0x47),
    ("--desktop-selected-border", 0xa6),
    ("--snap-fill", 0x33),
];

pub(crate) fn accent_color(snapshot: &SettingsSnapshot) -> UiColor {
    let rgb = flamewm_api::settings::accent_rgb(snapshot);
    UiColor {
        r: (rgb >> 16) as u8,
        g: (rgb >> 8) as u8,
        b: rgb as u8,
        a: flamewm_ui_core::style::PREVIEW_OUTLINE_ALPHA,
    }
}

/// Accent-derived token values for the desktop; `accentOpacity` scales them like the shell does.
pub(crate) fn accent_token_values(snapshot: &SettingsSnapshot) -> Vec<(&'static str, UiColor)> {
    let accent = accent_color(snapshot);
    let opacity = percent_alpha(setting_percent(snapshot, "accentOpacity", 100));
    ACCENT_TOKEN_ALPHAS
        .into_iter()
        .map(|(name, alpha)| {
            let scaled = ((u16::from(alpha) * u16::from(opacity)) / 255) as u8;
            (
                name,
                UiColor {
                    a: scaled,
                    ..accent
                },
            )
        })
        .collect()
}

pub(crate) fn project_accent_tokens(
    document: &mut impl UiDocumentAccess,
    snapshot: &SettingsSnapshot,
) -> Result<(), String> {
    for (name, color) in accent_token_values(snapshot) {
        document.color_variable(name, color)?;
    }
    document.color_variable("--preview-outline", accent_color(snapshot))?;
    // Text and overlay tokens follow the appearance; desktop.css and menu.css :root keep Dark values.
    flamewm_ui_document::appearance_tokens::project_appearance_tokens(document, snapshot)
}

/// Progressive-disclosure switch keys (test-only contract list). A toggle of any of these changes which member values the
/// snapshot resolves, so the desktop re-projects from the snapshot on every change; this list names
/// them for the settings-change contract. Sourced from the one switch table in `flamewm-api`.
#[cfg(test)]
pub(crate) fn group_switch_keys() -> Vec<&'static str> {
    flamewm_api::settings_groups::switch_keys().collect()
}

#[cfg(test)]
mod group_switch_key_tests {
    use super::group_switch_keys;

    #[test]
    fn desktop_key_list_names_every_switch_in_the_table() {
        let keys = group_switch_keys();
        assert_eq!(
            keys.len(),
            flamewm_api::settings_groups::SWITCH_GROUPS.len()
        );
        for switch in [
            "accentCustom",
            "customWindowOutline",
            "surfaceCustom",
            "fontsCustom",
        ] {
            assert!(
                keys.contains(&switch),
                "{switch} missing from desktop key list"
            );
        }
    }
}

#[cfg(test)]
mod settings_change_tests {
    use super::{desktop_visual_settings_changed, wallpaper_settings_changed};
    use flamewm_api::settings::{SettingValue, SettingsSnapshot};
    use std::collections::BTreeMap;

    fn snapshot(entries: &[(&str, SettingValue)]) -> SettingsSnapshot {
        SettingsSnapshot {
            revision: 0,
            values: entries
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    #[test]
    fn identical_values_at_a_new_revision_change_nothing() {
        let before = snapshot(&[("surfaceOpacity", SettingValue::Percent(40))]);
        let mut after = before.clone();
        after.revision = 9;
        assert!(!wallpaper_settings_changed(&before, &after));
        assert!(!desktop_visual_settings_changed(&before, &after));
    }

    #[test]
    fn glass_slider_motion_is_not_desktop_visual() {
        let before = snapshot(&[
            ("glassOpacityPercent", SettingValue::Integer(20)),
            ("glassBlurPx", SettingValue::Integer(28)),
            ("taskbarOpacityPercent", SettingValue::Integer(30)),
        ]);
        let after = snapshot(&[
            ("glassOpacityPercent", SettingValue::Integer(21)),
            ("glassBlurPx", SettingValue::Integer(29)),
            ("taskbarOpacityPercent", SettingValue::Integer(31)),
        ]);
        assert!(!wallpaper_settings_changed(&before, &after));
        assert!(!desktop_visual_settings_changed(&before, &after));
    }

    #[test]
    fn wallpaper_key_change_requests_wallpaper_and_repaints() {
        let before = snapshot(&[("wallpaperFit", SettingValue::Text("fill".into()))]);
        let after = snapshot(&[("wallpaperFit", SettingValue::Text("fit".into()))]);
        assert!(wallpaper_settings_changed(&before, &after));
        assert!(desktop_visual_settings_changed(&before, &after));
    }

    #[test]
    fn consumed_key_change_repaints_without_wallpaper_request() {
        let before = snapshot(&[("textSize", SettingValue::Percent(50))]);
        let after = snapshot(&[("textSize", SettingValue::Percent(51))]);
        assert!(!wallpaper_settings_changed(&before, &after));
        assert!(desktop_visual_settings_changed(&before, &after));
    }

    #[test]
    fn group_member_behind_off_switch_is_unchanged() {
        // accentCustom off: accent resolves to its default whatever is stored.
        let before = snapshot(&[("accent", SettingValue::Text("#111111".into()))]);
        let after = snapshot(&[("accent", SettingValue::Text("#222222".into()))]);
        assert!(!desktop_visual_settings_changed(&before, &after));
        // Switch on: the stored member now resolves, so the change counts.
        let before_on = snapshot(&[
            ("accentCustom", SettingValue::Boolean(true)),
            ("accent", SettingValue::Text("#111111".into())),
        ]);
        let after_on = snapshot(&[
            ("accentCustom", SettingValue::Boolean(true)),
            ("accent", SettingValue::Text("#222222".into())),
        ]);
        assert!(desktop_visual_settings_changed(&before_on, &after_on));
    }

    #[test]
    fn switch_toggle_counts_as_desktop_visual() {
        let before = snapshot(&[("surfaceCustom", SettingValue::Boolean(false))]);
        let after = snapshot(&[("surfaceCustom", SettingValue::Boolean(true))]);
        assert!(desktop_visual_settings_changed(&before, &after));
    }
}
