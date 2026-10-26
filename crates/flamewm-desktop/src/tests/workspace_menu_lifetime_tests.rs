use super::*;

#[test]
fn workspace_transition_clears_opened_entry_menu_and_delete_confirmation() {
    for kind in [
        MenuKind::Blank,
        MenuKind::Entry,
        MenuKind::ConvertWheel,
        MenuKind::Sticky,
    ] {
        let mut menu = Some(MenuState {
            kind,
            anchor: Point::new(100, 200),
            output: OutputId::new("default"),
            selected_id: Some("/tmp/my-image.png".to_owned()),
        });
        let mut pending_delete = Some(vec![PathBuf::from("/tmp/my-image.png")]);
        clear_workspace_menu_state(&mut menu, &mut pending_delete);
        assert!(menu.is_none(), "a menu must not survive workspace changes");
        assert!(
            pending_delete.is_none(),
            "a modal delete intent must not survive either"
        );
    }
}
