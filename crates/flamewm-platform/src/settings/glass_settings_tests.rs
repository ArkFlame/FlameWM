use super::*;

#[test]
fn glass_rows_form_their_own_reset_section() {
    let glass: Vec<_> = STABLE_SETTINGS
        .iter()
        .filter(|row| row.section == "glass")
        .map(|row| row.key)
        .collect();
    assert_eq!(
        glass,
        [
            "reduceTransparency",
            "glassOpacityPercent",
            "glassBlurPx",
            "taskbarMaterialMode",
            "taskbarOpacityPercent",
            "taskbarCustomTint",
        ]
    );
}

#[test]
fn resetting_glass_section_keeps_unrelated_appearance_and_taskbar_values() {
    let defaults = ProductSettings::default();
    let mut settings = ProductSettings {
        accent: "#123456".to_owned(),
        glass_blur_px: 40,
        taskbar_opacity_percent: 60,
        reduce_transparency: true,
        ..ProductSettings::default()
    };
    for row in STABLE_SETTINGS.iter().filter(|row| row.section == "glass") {
        (row.set)(&mut settings, &defaults, None, row.key).expect("reset glass row");
    }
    assert_eq!(settings.glass_blur_px, 28);
    assert_eq!(settings.taskbar_opacity_percent, 8);
    assert!(!settings.reduce_transparency);
    assert_eq!(settings.accent, "#123456");
}

#[test]
fn glass_keys_absent_from_the_file_read_as_defaults() {
    let settings = ConfigStore::default()
        .parse_snapshot("revision=2\n")
        .expect("legacy file without glass keys");
    assert!(!settings.reduce_transparency);
    assert_eq!(settings.glass_opacity_percent, 12);
    assert_eq!(settings.glass_blur_px, 28);
    assert_eq!(settings.taskbar_material_mode, "inherit");
    assert_eq!(settings.taskbar_opacity_percent, 8);
    assert_eq!(settings.taskbar_custom_tint, "#5879EE");
}

#[test]
fn removed_glassify_key_is_tolerated_and_dropped() {
    let settings = ConfigStore::default()
        .parse_snapshot("revision=2\nglassifyContrastZones=1\n")
        .expect("removed key must not fail the file");
    assert!(!settings.unknown_keys.contains_key("glassifyContrastZones"));
    assert!(!ConfigStore::serialize(&settings).contains("glassifyContrastZones"));
}

#[test]
fn glass_keys_round_trip_through_config_store() {
    let settings = ProductSettings {
        reduce_transparency: true,
        glass_opacity_percent: 42,
        glass_blur_px: 54,
        taskbar_material_mode: "custom-tint".to_owned(),
        taskbar_opacity_percent: 75,
        taskbar_custom_tint: "#3DAEE9".to_owned(),
        ..ProductSettings::default()
    };
    let text = ConfigStore::serialize(&settings);
    assert!(text.contains("glassBlurPx=54\n"));
    assert!(text.contains("taskbarMaterialMode=custom-tint\n"));
    let reloaded = ConfigStore::default()
        .parse_snapshot(&text)
        .expect("glass round trip");
    assert_eq!(reloaded, settings);
}

#[test]
fn glass_bounds_are_enforced_by_validate() {
    assert!(ProductSettings::default().validate().is_ok());
    for mutate in [
        |s: &mut ProductSettings| s.glass_opacity_percent = 3,
        |s: &mut ProductSettings| s.glass_opacity_percent = 43,
        |s: &mut ProductSettings| s.glass_blur_px = 55,
        |s: &mut ProductSettings| s.taskbar_opacity_percent = 76,
        |s: &mut ProductSettings| s.taskbar_material_mode = "glassy".to_owned(),
        |s: &mut ProductSettings| s.taskbar_custom_tint = "blue".to_owned(),
    ] {
        let mut settings = ProductSettings::default();
        mutate(&mut settings);
        assert!(settings.validate().is_err());
    }
}
