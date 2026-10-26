use super::general_settings;

#[test]
fn general_settings_intent_has_no_page_deep_link() {
    let intent = general_settings();
    assert_eq!(intent.program, "flamewm-settings");
    assert!(intent.argv.is_empty());
    assert!(intent.cwd.is_none());
}

#[test]
fn desktop_menu_settings_row_is_flamewm_settings_only() {
    let html = include_str!("../../../../ui/desktop/index.html");
    assert_eq!(html.matches("id=\"menu-row-settings\"").count(), 1);
    assert!(html.contains("<span>FlameWM Settings</span>"));
    assert!(
        html.contains(
            "src=\"assets/breeze/preferences-desktop.svg\"><span>FlameWM Settings</span>"
        )
    );
    assert!(!html.contains("Desktop and Wallpaper"));
}
