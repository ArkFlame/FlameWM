use super::{is_menu_action, sole_renamable};

#[test]
fn rename_editor_buttons_are_dispatched_as_menu_actions() {
    assert!(is_menu_action("rename.confirm"));
    assert!(is_menu_action("rename.cancel"));
    assert!(is_menu_action("entry.rename"));
}

#[test]
fn f2_renames_only_a_sole_renamable_selection() {
    let one = |id: &str| (id.to_owned(), true);
    assert_eq!(sole_renamable([one("a")]), Some("a".to_owned()));
    assert_eq!(sole_renamable([one("a"), one("b")]), None);
    assert_eq!(sole_renamable(Vec::<(String, bool)>::new()), None);
    // The trash pseudo-item is not renamable, so it does not block a real single selection.
    assert_eq!(
        sole_renamable([("trash".to_owned(), false), one("a")]),
        Some("a".to_owned())
    );
    assert_eq!(sole_renamable([("trash".to_owned(), false)]), None);
}
