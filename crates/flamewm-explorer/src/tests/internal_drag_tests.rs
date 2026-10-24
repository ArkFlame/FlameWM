use super::*;

#[test]
fn rubber_motion_fast_path_only_for_a_clean_pure_rubber_drag() {
    assert!(rubber_motion_only(UiActionPhase::Motion,false,true,false));
    assert!(!rubber_motion_only(UiActionPhase::Motion,true,true,false), "pending list change needs a full projection");
    assert!(!rubber_motion_only(UiActionPhase::Motion,false,true,true), "row/column/scroll drags need a full projection");
    assert!(!rubber_motion_only(UiActionPhase::Motion,false,false,false));
    assert!(!rubber_motion_only(UiActionPhase::Release,false,true,false), "release commits the selection");
    assert!(!rubber_motion_only(UiActionPhase::Press,false,true,false));
}

#[test]
fn hover_events_never_reproject_but_pending_list_changes_still_do() {
    // Counter: a run of pointer moves over rows must project zero times.
    let projections=(0..500).filter(|_|!hover_skips_projection(UiActionPhase::Hover,false,false)).count();
    assert_eq!(projections,0);
    // Counter: a run of motions with no drag in progress projects zero times; with a drag, every one does.
    let idle_motion=(0..500).filter(|_|!hover_skips_projection(UiActionPhase::Motion,false,false)).count();
    assert_eq!(idle_motion,0);
    let dragged_motion=(0..500).filter(|_|!hover_skips_projection(UiActionPhase::Motion,false,true)).count();
    assert_eq!(dragged_motion,500);
    assert!(!hover_skips_projection(UiActionPhase::Hover,true,false));
    assert!(!hover_skips_projection(UiActionPhase::Press,false,false));
}
#[test]
fn drag_requires_threshold_and_release_can_cancel() {
    let mut drag=RowDrag::new(PathBuf::from("/tmp/item"),10.0,10.0);
    assert!(!drag.motion(13.0,14.0));
    assert!(!drag.dragging);
    assert!(drag.motion(18.0,10.0));
    assert!(drag.dragging);
    assert!(drag.motion(18.0,10.0));
}
#[test]
fn tab_header_and_other_panes_cannot_be_silent_drop_targets() {
    let (tx,_rx)=mpsc::channel();
    let (scan_tx,_scan_rx)=mpsc::channel();
    let (chooser_tx,_chooser_rx)=mpsc::channel();
    let mut app=ExplorerApp::new(PathBuf::from("/tmp"),tx,scan_tx,chooser_tx);
    let dir=PathBuf::from("/tmp/folder");
    app.model.tab_mut().entries.push(Entry::new(dir.clone(),"folder".into(),true,false,0,None));
    assert!(app.folder_at(150.0,80.0).is_none());
    assert!(app.folder_at(230.0,45.0).is_none());
    assert_eq!(app.folder_at(230.0,list_top(false)+1.0),Some(dir));
}
