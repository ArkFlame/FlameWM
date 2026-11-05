use super::*;

#[test]
fn keyboard_focus_yields_only_to_text_fields_and_the_recorder() {
    let (_requests, transport) = recording_transport();
    let mut application = SettingsApplication::new(transport);
    assert!(!application.keyboard_captured(), "an idle window keeps Tab traversal");
    application.nav_search_focused = true;
    assert!(application.keyboard_captured(), "the sidebar search takes typed keys");
    application.nav_search_focused = false;
    application.hotkey_armed = true;
    assert!(application.keyboard_captured(), "an armed recorder takes Tab, Space and Enter");
}
