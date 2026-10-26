use super::*;

fn snapshot_with_theme(theme: Option<&str>) -> SettingsSnapshot {
    let mut snapshot = SettingsSnapshot::new(1);
    if let Some(theme) = theme {
        snapshot
            .values
            .insert("iconTheme".to_owned(), SettingValue::Text(theme.to_owned()));
    }
    snapshot
}

#[test]
fn icon_theme_change_detects_new_and_removed_values() {
    let papirus = snapshot_with_theme(Some("Papirus"));
    assert!(icon_theme_changed("", &papirus));
    assert!(!icon_theme_changed("Papirus", &papirus));
    assert!(icon_theme_changed("PlagueSur", &papirus));
    assert!(icon_theme_changed("Papirus", &snapshot_with_theme(None)));
    assert!(!icon_theme_changed("", &snapshot_with_theme(None)));
}
