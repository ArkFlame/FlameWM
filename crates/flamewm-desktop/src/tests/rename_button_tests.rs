use super::is_menu_action;

#[test]
fn rename_editor_buttons_are_dispatched_as_menu_actions() {
    assert!(is_menu_action("rename.confirm"));
    assert!(is_menu_action("rename.cancel"));
    assert!(is_menu_action("entry.rename"));
}
