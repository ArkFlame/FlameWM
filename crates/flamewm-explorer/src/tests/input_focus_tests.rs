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
