use super::*;

#[test]
fn open_resolved_clears_the_opening_status_once_the_file_opens() {
    let (tx, _rx) = mpsc::channel();
    let (scan_tx, _scan_rx) = mpsc::channel();
    let (chooser_tx, _chooser_rx) = mpsc::channel();
    let mut app = ExplorerApp::new(PathBuf::from("/tmp"), tx, scan_tx, chooser_tx);
    app.model.status = "Opening…".into();
    app.wake(WorkerResult::OpenResolved {
        path: PathBuf::from("/tmp/report.pdf"),
        result: Ok(true),
    });
    assert!(app.model.status.is_empty());
}
