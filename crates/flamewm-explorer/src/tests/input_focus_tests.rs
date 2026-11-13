use super::*;

fn press(action: &str) -> UiActionEvent {
    UiActionEvent {
        action: action.into(),
        phase: UiActionPhase::Press,
        inside: false,
        button: 1,
        modifiers: 0,
        x: 0.0,
        y: 0.0,
        text: None,
    }
}

fn app() -> (ExplorerApp, mpsc::Receiver<WorkerJob>, mpsc::Receiver<WorkerJob>, mpsc::Receiver<ChooserJob>) {
    let (tx, rx) = mpsc::channel();
    let (scan_tx, scan_rx) = mpsc::channel();
    let (chooser_tx, chooser_rx) = mpsc::channel();
    (ExplorerApp::new(PathBuf::from("/tmp"), tx, scan_tx, chooser_tx), rx, scan_rx, chooser_rx)
}

#[test]
fn press_outside_clears_the_path_focus_without_navigating() {
    let (mut app, _rx, _scan_rx, _chooser_rx) = app();
    app.begin_edit(EditMode::Path, "/usr".into(), None);
    app.event(&press("explorer.row.0"));
    assert!(app.edit.is_none());
    assert_eq!(app.model.tab().path(), std::path::Path::new("/tmp"));
}

#[test]
fn press_outside_clears_the_search_focus_and_keeps_the_applied_search() {
    let (mut app, _rx, _scan_rx, _chooser_rx) = app();
    app.model.tab_mut().search = "report".into();
    app.begin_edit(EditMode::Search, "rep".into(), None);
    app.event(&press("explorer.row.0"));
    assert!(app.edit.is_none());
    assert_eq!(app.model.tab().search, "report");
}

#[test]
fn press_inside_either_field_keeps_its_focus() {
    for (mode, action) in [
        (EditMode::Path, "explorer.address"),
        (EditMode::Path, "explorer.address-text"),
        (EditMode::Search, "explorer.search"),
        (EditMode::Search, "explorer.search-text"),
    ] {
        let (mut app, _rx, _scan_rx, _chooser_rx) = app();
        app.begin_edit(mode, "text".into(), None);
        app.event(&press(action));
        assert!(app.edit.is_some(), "{action} must keep focus");
    }
}

fn key_press(key: &str, text: Option<&str>) -> UiKeyEvent {
    UiKeyEvent { phase: UiKeyPhase::Press, modifiers: Vec::new(), key: key.into(), text: text.map(str::to_owned) }
}

#[test]
fn modal_confirm_swallows_typed_letters_before_type_ahead() {
    let (mut app, _rx, _scan_rx, _chooser_rx) = app();
    app.dialog = Some(Dialog::Confirm { title: "Move to trash?".into(), lines: Vec::new(), pending: Pending::Trash(Vec::new()) });
    app.key_inner(&key_press("a", Some("a")));
    assert!(app.typed.is_empty(), "a letter under a modal dialog must not start type-ahead");
    assert!(app.dialog.is_some(), "the dialog stays open for Enter or Escape");
}

#[test]
fn permissions_field_takes_keys_while_properties_is_open_and_escape_only_ends_the_edit() {
    let (mut app, _rx, _scan_rx, _chooser_rx) = app();
    app.open_properties(PathBuf::from("/tmp"));
    assert!(app.dialog.is_some());
    app.dialog_action("primary");
    assert!(matches!(app.edit, Some(EditMode::Mode)));
    let before = app.field.text.clone();
    app.key_inner(&key_press("backspace", None));
    assert_eq!(app.field.text, before[..before.len() - 1].to_string(), "Backspace edits the permissions field");
    app.key_inner(&key_press("escape", None));
    assert!(app.edit.is_none(), "Escape ends the permissions edit");
    assert!(app.dialog.is_some(), "Escape must not close the Properties dialog it sits in");
}
