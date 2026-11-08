use super::*;

fn app() -> (ExplorerApp,std::sync::mpsc::Receiver<WorkerJob>) {
    let (tx,_)=std::sync::mpsc::channel();
    let (scan_tx,scans)=std::sync::mpsc::channel();
    let (chooser_tx,_)=std::sync::mpsc::channel();
    (ExplorerApp::new(PathBuf::from("/a"),tx,scan_tx,chooser_tx),scans)
}

#[test]
fn breadcrumb_paths_are_bounded_and_preserve_native_components() {
    use std::os::unix::ffi::OsStringExt;
    assert_eq!(breadcrumbs(Path::new("/")),vec![("/".into(),PathBuf::from("/"))]);
    let path=PathBuf::from("/one/two words").join(std::ffi::OsString::from_vec(vec![b'x',0xff]));
    let crumbs=breadcrumbs(&path);
    assert_eq!(crumbs[2],("two words".into(),PathBuf::from("/one/two words")));
    assert_eq!(crumbs.last().unwrap().1,path);
    let long=breadcrumbs(Path::new("/a/b/c/d/e/f/g"));
    assert_eq!(long.len(),BREADCRUMB_SLOTS);
    assert_eq!(long[0].1,Path::new("/a/b"));
    assert_eq!(long.last().unwrap().1,Path::new("/a/b/c/d/e/f/g"));
    assert!(breadcrumbs(Path::new("sftp://u@host/path")).is_empty());
}

#[test]
fn breadcrumb_dispatch_uses_validated_navigation_and_history() {
    let (mut app,scans)=app();
    app.model.tab_mut().visit(PathBuf::from("/a/b/c"));
    let event=UiActionEvent{action:"explorer.breadcrumb.2".into(),phase:UiActionPhase::Release,inside:true,button:1,modifiers:0,x:0.0,y:0.0,text:None};
    app.event(&event);
    assert_eq!(app.model.tab().path(),Path::new("/a/b/c"));
    let WorkerJob::Scan{generation,path,..}=scans.recv().unwrap() else {panic!("expected scan")};
    assert_eq!(path,Path::new("/a/b"));
    app.wake(WorkerResult::Scanned{generation,path,entries:Ok(vec![])});
    assert_eq!(app.model.tab().path(),Path::new("/a/b"));
    assert!(app.model.tab_mut().back());
    assert_eq!(app.model.tab().path(),Path::new("/a/b/c"));
}
fn result(app:&mut ExplorerApp, scans:&std::sync::mpsc::Receiver<WorkerJob>, entries:std::io::Result<Vec<Entry>>) {
    let WorkerJob::Scan{generation,path,..}=scans.recv().unwrap() else {panic!("expected scan")};
    app.wake(WorkerResult::Scanned{generation,path,entries});
}

#[test]
fn invalid_navigation_preserves_history_and_forward() {
    for kind in [std::io::ErrorKind::NotFound,std::io::ErrorKind::NotADirectory,std::io::ErrorKind::PermissionDenied] {
        let (mut app,scans)=app();
        app.model.tab_mut().visit(PathBuf::from("/b"));
        app.model.tab_mut().visit(PathBuf::from("/c"));
        assert!(app.model.tab_mut().back());
        app.navigate(PathBuf::from("/invalid"));
        assert_eq!(app.model.tab().path(),Path::new("/b"));
        result(&mut app,&scans,Err(kind.into()));
        assert_eq!(app.model.tab().path(),Path::new("/b"));
        assert!(app.model.tab().can_forward());
        assert!(app.model.tab_mut().back());
        assert_eq!(app.model.tab().path(),Path::new("/a"));
    }
}

#[test]
fn stale_failure_cannot_override_newer_success() {
    let (mut app,scans)=app();
    app.navigate(PathBuf::from("/missing"));
    let WorkerJob::Scan{generation,path,..}=scans.recv().unwrap() else {panic!("expected scan")};
    app.navigate(PathBuf::from("/new"));
    result(&mut app,&scans,Ok(vec![]));
    app.wake(WorkerResult::Scanned{generation,path,entries:Err(std::io::ErrorKind::NotFound.into())});
    assert_eq!(app.model.tab().path(),Path::new("/new"));
    assert!(app.activity.is_none());
}

#[test]
fn vanished_current_directory_returns_home_with_notice() {
    let (mut app,scans)=app();
    app.model.tab_mut().visit(PathBuf::from("/vanished"));
    app.scan();
    result(&mut app,&scans,Err(std::io::ErrorKind::NotFound.into()));
    assert_eq!(app.model.tab().path(),Path::new("/vanished"));
    result(&mut app,&scans,Ok(vec![]));
    assert_eq!(app.model.tab().path(),home_dir());
    assert!(app.activity.as_deref().is_some_and(|notice|notice.contains("no longer available")));
    assert!(app.model.tab_mut().back());
    assert_eq!(app.model.tab().path(),Path::new("/a"));
}

#[test]
fn rapid_history_navigation_accumulates_and_ignores_stale_results() {
    let (mut app,scans)=app();
    app.model.tab_mut().visit(PathBuf::from("/b"));
    app.model.tab_mut().visit(PathBuf::from("/c"));
    app.history_navigation(flamewm_explorer::Tab::back);
    let WorkerJob::Scan{generation,path,..}=scans.recv().unwrap() else {panic!("expected scan")};
    app.history_navigation(flamewm_explorer::Tab::back);
    assert_eq!(app.model.tab().path(),Path::new("/c"));
    let WorkerJob::Scan{generation:g2,path:p2,..}=scans.try_recv().expect("expected second scan") else {panic!("expected scan")};
    assert_eq!(p2,Path::new("/a"));
    app.history_navigation(flamewm_explorer::Tab::back); // boundary leaves the pending request intact
    assert!(scans.try_recv().is_err());
    app.wake(WorkerResult::Scanned{generation,path,entries:Ok(vec![])});
    assert_eq!(app.model.tab().path(),Path::new("/c"));
    app.wake(WorkerResult::Scanned{generation:g2,path:p2,entries:Ok(vec![])});
    assert_eq!(app.model.tab().path(),Path::new("/a"));
}

#[test]
fn rapid_back_forward_returns_to_original_cursor() {
    let (mut app,scans)=app();
    app.model.tab_mut().visit(PathBuf::from("/b"));
    app.model.tab_mut().visit(PathBuf::from("/c"));
    app.history_navigation(flamewm_explorer::Tab::back);
    let WorkerJob::Scan{generation,path,..}=scans.recv().unwrap() else {panic!("expected scan")};
    app.history_navigation(flamewm_explorer::Tab::forward);
    let WorkerJob::Scan{generation:g2,path:p2,..}=scans.try_recv().expect("expected second scan") else {panic!("expected scan")};
    assert_eq!(p2,Path::new("/c"));
    app.wake(WorkerResult::Scanned{generation:g2,path:p2,entries:Ok(vec![])});
    app.wake(WorkerResult::Scanned{generation,path,entries:Ok(vec![])});
    assert_eq!(app.model.tab().path(),Path::new("/c"));
    assert!(app.model.tab_mut().back());
    assert_eq!(app.model.tab().path(),Path::new("/b"));
}

#[test]
fn changed_directory_is_refreshed_after_failed_candidate() {
    let (mut app,scans)=app();
    app.navigate(PathBuf::from("/missing"));
    app.wake(WorkerResult::DirChanged);
    app.wake(WorkerResult::DirChanged);
    result(&mut app,&scans,Err(std::io::ErrorKind::NotFound.into()));
    let WorkerJob::Scan{path,..}=scans.try_recv().expect("expected refresh") else {panic!("expected refresh")};
    assert_eq!(path,Path::new("/a"));
    assert!(scans.try_recv().is_err());
    assert_eq!(app.model.tab().path(),Path::new("/a"));
}

#[test]
fn dirty_notification_does_not_replace_newer_success() {
    let (mut app,scans)=app();
    app.navigate(PathBuf::from("/old"));
    let WorkerJob::Scan{generation,path,..}=scans.recv().unwrap() else {panic!("expected scan")};
    app.wake(WorkerResult::DirChanged);
    app.navigate(PathBuf::from("/new"));
    app.wake(WorkerResult::Scanned{generation,path,entries:Err(std::io::ErrorKind::NotFound.into())});
    let WorkerJob::Scan{generation:g2,path:p2,..}=scans.try_recv().expect("newer candidate") else {panic!("expected scan")};
    assert!(scans.try_recv().is_err());
    assert_eq!(app.model.tab().path(),Path::new("/a"));
    assert!(app.pending_directory_dirty);
    app.wake(WorkerResult::Scanned{generation:g2,path:p2,entries:Ok(vec![])});
    let WorkerJob::Scan{path:refreshed,..}=scans.try_recv().expect("new directory refresh") else {panic!("expected scan")};
    assert_eq!(refreshed,Path::new("/new"));
    assert_eq!(app.model.tab().path(),Path::new("/new"));
}

#[test]
fn cancelling_pending_navigation_refreshes_the_committed_directory() {
    let (mut app,scans)=app();
    app.navigate(PathBuf::from("/pending"));
    let WorkerJob::Scan{generation,path,..}=scans.recv().unwrap() else {panic!("expected scan")};
    app.wake(WorkerResult::DirChanged);
    app.request_listing();
    let WorkerJob::Scan{path:refreshed,..}=scans.try_recv().expect("expected refresh") else {panic!("expected scan")};
    assert_eq!(refreshed,Path::new("/a"));
    app.wake(WorkerResult::Scanned{generation,path,entries:Ok(vec![])});
    assert_eq!(app.model.tab().path(),Path::new("/a"));
}
