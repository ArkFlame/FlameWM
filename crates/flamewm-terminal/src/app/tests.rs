//! Unit tests for the app module and its submodules.

use super::grid::overlay;
use super::layout::{Axis, DIVIDER, Rect, SPLIT_RATIO, split_fits, split_rect};
use super::overlay::{BANNER, BANNER_ART_ROWS};
use super::pointer::{mouse_report, mouse_report_raw};
use super::selection::{next_click_count, word_bounds};
use super::*;
use crate::pty::Launch;
use crate::pty::PtySize;
use crate::screen::MouseMode;
use crate::session::SessionEvent;
use crate::store::TerminalConfig;
use crate::store::ThemeStore;
use crate::suggest::History;
use crate::view::text_run;
use flamewm_ui_x11::UiActionPhase;
use flamewm_ui_x11::UiKeyEvent;
use flamewm_ui_x11::UiKeyPhase;
use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use crate::pty::PtyProcess;
use crate::themes::builtin_themes;

/// A pane's shell: `cat` keeps the PTY open and quiet until it is killed.
fn cat() -> Launch {
    Launch {
        program: "/bin/cat".into(),
        args: Vec::new(),
        cwd: None,
    }
}

/// A key press event with no modifiers.
fn key(name: &str) -> UiKeyEvent {
    UiKeyEvent {
        phase: UiKeyPhase::Press,
        modifiers: Vec::new(),
        key: name.to_owned(),
        text: None,
    }
}

/// An App without a window: no clipboard, an empty history and a theme directory that does not exist.
fn test_app() -> App {
    let (events, _receiver) = std::sync::mpsc::channel();
    let (_peer, wake) = UnixStream::pair().expect("socket pair");
    let mut app = App {
        tabs: Vec::new(),
        active: 0,
        next_id: 1,
        cols: 80,
        grid_rows: 23,
        cell_w: 8.0,
        cell_h: 16.0,
        config: TerminalConfig::default(),
        config_path: PathBuf::from("/nonexistent/flamewm-terminal/terminal.toml"),
        store: ThemeStore::new("/nonexistent/flamewm-terminal/themes"),
        themes: builtin_themes(),
        snapshot: None,
        palette: builtin_themes()[0].palette.clone(),
        surface_alpha: 255,
        surface_stroke: flamewm_ui_x11::UiColor { r:255, g:255, b:255, a:63 },
        view: View::Terminal,
        clipboard: None,
        events,
        wake,
        first_launch: None,
        packaged_root: PathBuf::from("/"),
        ui: crate::ui::UiCache::default(),
        pane_runs: HashMap::new(),
        grid_composed: Vec::new(),
        menu: None,
        rename: None,
        history: History::open(None, &[]),
        banner: false,
        window: (640, 384),
    };
    app.refresh_metrics();
    app.set_window(640, 384);
    app
}

/// Every line of a pane's screen, for assertions about which pane received output.
fn pane_text(app: &App, id: u64) -> String {
    let pane = app
        .tabs
        .iter()
        .find_map(|tab| tab.root.pane(id))
        .expect("pane");
    let screen = &pane.session.screen;
    let top = screen.total_lines().saturating_sub(screen.rows());
    (0..screen.rows())
        .filter_map(|row| screen.line_abs(top + row))
        .map(|line| line.iter().map(|cell| cell.ch).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn overlap(a: Rect, b: Rect) -> bool {
    a.x0 < b.x0 + b.cols && b.x0 < a.x0 + a.cols && a.y0 < b.y0 + b.rows && b.y0 < a.y0 + a.rows
}

/// Opens one tab and splits it once, returning (first leaf, second leaf).
fn split_tab(app: &mut App, axis: Axis) -> (u64, u64) {
    app.open_tab(cat()).expect("open tab");
    let first = app.tabs[0].focused;
    app.split_focused(axis).expect("split");
    let second = app.tabs[0].focused;
    (first, second)
}

/// Grid size a pane was last resized to.
fn pane_size(app: &App, id: u64) -> (usize, usize) {
    app.pane(id).map(|pane| pane.size).expect("pane")
}

#[test]
fn split_rect_gives_both_axes_a_divider() {
    let grid = Rect {
        x0: 0,
        y0: 0,
        cols: 80,
        rows: 23,
    };
    let (left, right) = split_rect(grid, Axis::Columns, SPLIT_RATIO);
    assert_eq!(
        left,
        Rect {
            x0: 0,
            y0: 0,
            cols: 40,
            rows: 23
        }
    );
    assert_eq!(
        right,
        Rect {
            x0: 41,
            y0: 0,
            cols: 39,
            rows: 23
        },
        "one divider column between them"
    );
    let (top, bottom) = split_rect(grid, Axis::Rows, SPLIT_RATIO);
    assert_eq!(
        top,
        Rect {
            x0: 0,
            y0: 0,
            cols: 80,
            rows: 11
        }
    );
    assert_eq!(
        bottom,
        Rect {
            x0: 0,
            y0: 12,
            cols: 80,
            rows: 11
        },
        "one divider row between them"
    );
    // The divider never falls outside the rectangle.
    for rect in [left, right, top, bottom] {
        assert!(rect.x0 + rect.cols <= grid.cols && rect.y0 + rect.rows <= grid.rows);
    }
}

#[test]
fn nested_split_rects_stay_inside_their_parent() {
    let grid = Rect {
        x0: 0,
        y0: 0,
        cols: 80,
        rows: 23,
    };
    let (left, right) = split_rect(grid, Axis::Columns, SPLIT_RATIO);
    let (top_left, bottom_left) = split_rect(left, Axis::Rows, SPLIT_RATIO);
    assert_eq!(
        top_left,
        Rect {
            x0: 0,
            y0: 0,
            cols: 40,
            rows: 11
        }
    );
    assert_eq!(
        bottom_left,
        Rect {
            x0: 0,
            y0: 12,
            cols: 40,
            rows: 11
        }
    );
    let all = [top_left, bottom_left, right];
    for (index, a) in all.iter().enumerate() {
        for b in all.iter().skip(index + 1) {
            assert!(!overlap(*a, *b), "{a:?} overlaps {b:?}");
        }
        assert!(a.x0 + a.cols <= grid.cols && a.y0 + a.rows <= grid.rows);
    }
}

#[test]
fn a_split_can_be_refused_when_the_halves_would_be_too_small() {
    let narrow = Rect {
        x0: 0,
        y0: 0,
        cols: 4,
        rows: 23,
    };
    assert!(
        !split_fits(narrow, Axis::Columns),
        "2+1+2 leaves two 2-column panes; 3 does not"
    );
    assert!(split_fits(Rect { cols: 5, ..narrow }, Axis::Columns));
    let short = Rect {
        x0: 0,
        y0: 0,
        cols: 80,
        rows: 1,
    };
    assert!(
        !split_fits(short, Axis::Rows),
        "a one-row grid has no room for two panes"
    );
    assert!(split_fits(Rect { rows: 3, ..short }, Axis::Rows));
}

#[test]
fn splitting_the_focused_pane_gives_it_the_focus_and_a_new_session() {
    let mut app = test_app();
    let (first, second) = split_tab(&mut app, Axis::Columns);
    assert_ne!(first, second, "each leaf has its own session id");
    assert_eq!(app.tabs[0].focused, second);
    assert_eq!(app.tabs[0].root.rects(app.grid_rect()).len(), 2);
    assert!(app.pane(first).is_some() && app.pane(second).is_some());
    // The tab keeps the session id of its first leaf.
    assert_eq!(app.tabs[0].id, first);
}

#[test]
fn nested_splits_tile_the_grid_without_overlapping() {
    let mut app = test_app();
    app.open_tab(cat()).expect("tab");
    app.split_focused(Axis::Columns).expect("split columns");
    app.split_focused(Axis::Rows).expect("split rows");
    let rects: Vec<Rect> = app.tabs[0]
        .root
        .rects(app.grid_rect())
        .into_iter()
        .map(|(_, rect)| rect)
        .collect();
    assert_eq!(rects.len(), 3);
    for (index, a) in rects.iter().enumerate() {
        for b in rects.iter().skip(index + 1) {
            assert!(!overlap(*a, *b), "{a:?} overlaps {b:?}");
        }
        assert!(
            a.x0 + a.cols <= app.cols && a.y0 + a.rows <= app.grid_rows,
            "{a:?} leaves the grid"
        );
    }
    // Panes plus the one-column and one-row dividers fill the grid exactly.
    let panes: usize = rects.iter().map(|rect| rect.cols * rect.rows).sum();
    assert_eq!(
        panes + app.grid_rows * DIVIDER + rects[1].cols * DIVIDER,
        app.cols * app.grid_rows
    );
}

#[test]
fn an_exit_collapses_the_pane_onto_the_survivor() {
    let mut app = test_app();
    let (first, second) = split_tab(&mut app, Axis::Columns);
    app.session_event(SessionEvent::Output {
        id: second,
        bytes: b"bye".to_vec(),
    });
    assert!(
        !app.session_event(SessionEvent::Exited { id: second }),
        "the tab survives"
    );
    assert_eq!(app.tabs.len(), 1);
    assert_eq!(
        app.tabs[0].root.rects(app.grid_rect()).len(),
        1,
        "the parent split is gone"
    );
    assert_eq!(app.tabs[0].focused, first, "focus moves to the survivor");
    let rect = app.tabs[0].root.rects(app.grid_rect())[0].1;
    assert_eq!(
        rect,
        app.grid_rect(),
        "the survivor takes the whole grid back"
    );
    assert_eq!(
        app.tabs[0].root.leaf().map(|pane| pane.size),
        Some((app.cols, app.grid_rows))
    );
}

#[test]
fn the_last_leaf_takes_the_tab_with_it() {
    let mut app = test_app();
    let (first, second) = split_tab(&mut app, Axis::Rows);
    app.session_event(SessionEvent::Exited { id: second });
    assert_eq!(app.tabs.len(), 1);
    assert!(
        app.session_event(SessionEvent::Exited { id: first }),
        "the last tab closing ends the window"
    );
    assert!(app.tabs.is_empty());
}

#[test]
fn output_and_exit_route_to_the_leaf_not_the_tab() {
    let mut app = test_app();
    let (first, second) = split_tab(&mut app, Axis::Columns);
    app.session_event(SessionEvent::Output {
        id: first,
        bytes: b"alpha".to_vec(),
    });
    assert!(pane_text(&app, first).contains("alpha"));
    assert!(
        !pane_text(&app, second).contains("alpha"),
        "the other pane must not see it"
    );
    app.session_event(SessionEvent::Output {
        id: second,
        bytes: b"beta".to_vec(),
    });
    assert!(pane_text(&app, second).contains("beta"));
    assert!(!pane_text(&app, first).contains("beta"));
    // An unfocused leaf exiting does not touch the focused one.
    app.session_event(SessionEvent::Exited { id: first });
    assert_eq!(app.tabs[0].focused, second);
    assert!(pane_text(&app, second).contains("beta"));
}

#[test]
fn each_pane_keeps_its_own_grid_and_pty_size() {
    let mut app = test_app();
    let (first, second) = split_tab(&mut app, Axis::Columns);
    let (left, right) = (pane_size(&app, first), pane_size(&app, second));
    assert_ne!(
        left, right,
        "side-by-side panes have different column counts"
    );
    assert_eq!(left.0 + right.0 + DIVIDER, app.cols);
    assert_eq!(left.1, app.grid_rows);
    assert_eq!(right.1, app.grid_rows);
    assert_ne!(
        app.pty_size(left.0, left.1),
        app.pty_size(right.0, right.1),
        "each PTY is told its own size"
    );
    assert_eq!(
        (
            app.pty_size(left.0, left.1).cols as usize,
            app.pty_size(left.0, left.1).rows as usize
        ),
        left
    );
    assert_eq!(
        (
            app.pty_size(right.0, right.1).cols as usize,
            app.pty_size(right.0, right.1).rows as usize
        ),
        right
    );
    // A window resize reaches both panes, still with different sizes.
    app.set_window(1000, 600);
    assert_ne!(pane_size(&app, first), left);
    assert_eq!(
        pane_size(&app, first).0 + pane_size(&app, second).0 + DIVIDER,
        app.cols
    );
    // A horizontal split turns the sizes into rows instead of columns.
    let mut stacked = test_app();
    let (top, bottom) = split_tab(&mut stacked, Axis::Rows);
    let (top, bottom) = (pane_size(&stacked, top), pane_size(&stacked, bottom));
    assert_eq!(top.0, stacked.cols);
    assert_eq!(bottom.0, stacked.cols);
    assert!(
        top.1 < stacked.grid_rows && bottom.1 < stacked.grid_rows,
        "each pane is shorter than the grid"
    );
    assert_eq!(top.1 + bottom.1 + DIVIDER, stacked.grid_rows);
}

#[test]
fn distinct_pane_sizes_reach_two_real_ptys() {
    let grid = Rect {
        x0: 0,
        y0: 0,
        cols: 80,
        rows: 23,
    };
    let (first, second) = split_rect(grid, Axis::Columns, SPLIT_RATIO);
    let size = |rect: Rect| PtySize {
        cols: rect.cols as u16,
        rows: rect.rows as u16,
        pixel_width: 0,
        pixel_height: 0,
    };
    let left = PtyProcess::spawn(&cat(), size(first)).expect("left pty");
    let right = PtyProcess::spawn(&cat(), size(second)).expect("right pty");
    assert_ne!(
        left.winsize().expect("left size"),
        right.winsize().expect("right size")
    );
    assert_eq!(left.winsize().expect("left size").cols as usize, first.cols);
    assert_eq!(
        right.winsize().expect("right size").cols as usize,
        second.cols
    );
    // The same PTY follows a new layout, which is what splitting and collapsing do.
    let (top, _bottom) = split_rect(grid, Axis::Rows, SPLIT_RATIO);
    left.resize(size(top)).expect("resize");
    let now = left.winsize().expect("resized size");
    assert_eq!((now.cols as usize, now.rows as usize), (top.cols, top.rows));
}

#[test]
fn function_keys_change_tabs_and_never_go_to_the_child() {
    let mut app = test_app();
    app.open_tab(cat()).expect("first");
    app.open_tab(cat()).expect("second");
    assert_eq!(app.active, 1);
    app.key(&key("f4"));
    assert_eq!(app.active, 0, "F4 wraps to the next tab");
    app.key(&key("f3"));
    assert_eq!(app.active, 1, "F3 wraps back to the previous tab");
    // F1 is not a shortcut: it is encoded for the child and changes nothing here.
    app.key(&key("f1"));
    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.active, 1);
    app.key(&key("f2"));
    assert_eq!(app.tabs.len(), 3, "F2 opens a tab with a shell of its own");
    assert_eq!(app.active, 2);
    let ids: Vec<u64> = app.tabs.iter().map(|tab| tab.id).collect();
    assert_eq!(ids.len(), 3);
    assert!(
        ids.iter().all(|id| app.pane(*id).is_some()),
        "every tab has its own session"
    );
    assert_eq!(
        ids.len(),
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        "session ids are unique"
    );
}

#[test]
fn a_split_frame_keeps_each_pane_in_its_own_columns_with_one_cursor() {
    let mut app = test_app();
    let (first, focused) = split_tab(&mut app, Axis::Columns);
    app.session_event(SessionEvent::Output {
        id: first,
        bytes: b"left".to_vec(),
    });
    app.session_event(SessionEvent::Output {
        id: focused,
        bytes: b"right".to_vec(),
    });
    let rects = app.tabs[0].root.rects(app.grid_rect());
    let left = rects
        .iter()
        .find(|(id, _)| *id == first)
        .expect("left pane")
        .1;
    let right = rects
        .iter()
        .find(|(id, _)| *id == focused)
        .expect("right pane")
        .1;
    let frame = app.frame();
    assert_eq!(
        frame.len(),
        app.grid_rows + 1,
        "tab bar plus the whole grid"
    );
    let row = 1 + left.y0;
    let (mut saw_left, mut saw_right) = (false, false);
    for run in frame[row].iter() {
        let end = run.col + run.cells;
        if run.col >= left.x0 && end <= left.x0 + left.cols {
            saw_left = true;
        } else if run.col >= right.x0 && end <= right.x0 + right.cols {
            saw_right = true;
        } else {
            panic!("run crosses a pane edge: {run:?}");
        }
    }
    assert!(saw_left && saw_right, "both panes draw on the shared row");
    assert!(frame[row].iter().any(|run| run.text.starts_with("left")));
    assert!(frame[row].iter().any(|run| run.text.starts_with("right")));
    let cursors: Vec<usize> = frame[row]
        .iter()
        .filter(|run| run.bg == Some(app.palette.cursor))
        .map(|run| run.col)
        .collect();
    assert_eq!(cursors.len(), 1, "only the focused pane draws a cursor");
    assert!(cursors[0] >= right.x0, "the cursor is in the focused pane");
    // A stacked split puts the panes on different rows instead of columns.
    let mut stacked = test_app();
    let (top, bottom) = split_tab(&mut stacked, Axis::Rows);
    stacked.session_event(SessionEvent::Output {
        id: top,
        bytes: b"top".to_vec(),
    });
    stacked.session_event(SessionEvent::Output {
        id: bottom,
        bytes: b"bottom".to_vec(),
    });
    let frame = stacked.frame();
    let rows: Vec<Rect> = stacked.tabs[0]
        .root
        .rects(stacked.grid_rect())
        .into_iter()
        .map(|(_, rect)| rect)
        .collect();
    let (top_rect, bottom_rect) = (rows[0], rows[1]);
    assert!(
        frame[1 + top_rect.y0]
            .iter()
            .any(|run| run.text.starts_with("top"))
    );
    assert!(
        frame[1 + bottom_rect.y0]
            .iter()
            .any(|run| run.text.starts_with("bottom"))
    );
    assert!(
        !frame[1 + top_rect.y0]
            .iter()
            .any(|run| run.text.starts_with("bottom"))
    );
}

#[test]
fn a_visible_suggestion_still_accepts_tab_and_stays_inside_its_pane() {
    let store =
        std::env::temp_dir().join(format!("flamewm-terminal-ghost-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&store);
    let mut history = History::open(Some(store.clone()), &[]);
    history.record("ls -la").expect("record a command");
    let mut app = test_app();
    app.history = history;
    let (_first, focused) = split_tab(&mut app, Axis::Columns);
    let rect = app.pane_rect(focused).expect("focused rect");
    if let Some(pane) = app.focused_pane_mut() {
        pane.input.paste("ls");
        pane.prompt = Some(true);
    }
    assert_eq!(app.ghost(), Some(" -la"));
    // The ghost is drawn inside the focused pane, never across the divider.
    let frame = app.frame();
    let row = 1 + rect.y0;
    let ghost = frame[row]
        .iter()
        .find(|run| run.text == " -la")
        .expect("ghost run");
    assert!(ghost.col >= rect.x0 && ghost.col + ghost.cells <= rect.x0 + rect.cols);
    // Tab still accepts the suggestion, exactly as it does with a single pane.
    app.key(&key("tab"));
    assert_eq!(
        app.focused_pane().map(|pane| pane.input.text()),
        Some("ls -la".to_string())
    );
    assert_ne!(
        app.tabs[0].focused, app.tabs[0].id,
        "the suggestion went to the focused pane"
    );
    let _ = std::fs::remove_file(&store);
}

#[test]
fn closing_a_tab_reaps_every_leaf() {
    let mut app = test_app();
    app.open_tab(cat()).expect("tab");
    app.split_focused(Axis::Columns).expect("split columns");
    app.split_focused(Axis::Rows).expect("split rows");
    let pids: Vec<u32> = app.tabs[0]
        .root
        .leaves()
        .iter()
        .map(|pane| pane.session.pty.id())
        .collect();
    assert_eq!(pids.len(), 3);
    app.close_tab(0);
    assert!(app.tabs.is_empty());
    for pid in pids {
        assert!(
            !PathBuf::from(format!("/proc/{pid}")).exists(),
            "child {pid} was not reaped"
        );
    }
}

#[test]
fn closing_another_tab_keeps_the_active_one() {
    let mut app = test_app();
    app.open_tab(cat()).expect("first");
    app.open_tab(cat()).expect("second");
    app.open_tab(cat()).expect("third");
    let active = app.tabs[app.active].id;
    app.close_tab(0);
    assert_eq!(app.tabs.len(), 2);
    assert_eq!(
        app.tabs[app.active].id, active,
        "the active tab does not shift"
    );
}

#[test]
fn closing_a_pane_leaves_the_other_pane_and_its_tab() {
    let mut app = test_app();
    let (first, second) = split_tab(&mut app, Axis::Columns);
    app.close_pane();
    assert_eq!(app.tabs.len(), 1);
    assert_eq!(app.tabs[0].focused, first);
    assert!(app.pane(second).is_none());
    // The last pane closes the tab.
    app.close_pane();
    assert!(app.tabs.is_empty());
    assert!(app.pane(first).is_none());
}

#[test]
fn one_typed_char_rebuilds_only_its_row_and_idle_frames_rebuild_nothing() {
    let mut app = test_app();
    app.open_tab(cat()).expect("tab");
    let id = app.tabs[0].focused;
    app.session_event(SessionEvent::Output {
        id,
        bytes: b"hello".to_vec(),
    });
    let before = app.frame();
    assert_eq!(app.frame(), before, "an idle frame repeats the same rows");
    assert_eq!(
        app.pane_runs[&id].rebuilt, 0,
        "an idle frame rebuilds no rows"
    );
    app.session_event(SessionEvent::Output {
        id,
        bytes: b"!".to_vec(),
    });
    let after = app.frame();
    assert_eq!(
        app.pane_runs[&id].rebuilt, 1,
        "one typed char rebuilds exactly one row"
    );
    let changed: Vec<usize> = (0..before.len())
        .filter(|&row| before[row] != after[row])
        .collect();
    assert_eq!(changed.len(), 1, "only the typed row changed: {changed:?}");
    assert!(
        after[changed[0]]
            .iter()
            .any(|run| run.text.contains("hello!"))
    );
}

#[test]
fn unchanged_grid_rows_are_shared_allocations() {
    let mut app = test_app();
    app.open_tab(cat()).expect("tab");
    let id = app.tabs[0].focused;
    app.session_event(SessionEvent::Output {
        id,
        bytes: b"hello".to_vec(),
    });
    let before = app.frame();
    let grid = 1 + app.banner_rows();
    let idle = app.frame();
    assert!(
        (grid..before.len()).all(|row| Arc::ptr_eq(&before[row], &idle[row])),
        "idle grid rows are the same allocations"
    );
    app.session_event(SessionEvent::Output {
        id,
        bytes: b"!".to_vec(),
    });
    let typed = app.frame();
    let new_rows: Vec<usize> = (grid..typed.len())
        .filter(|&row| !Arc::ptr_eq(&idle[row], &typed[row]))
        .collect();
    assert_eq!(
        new_rows.len(),
        1,
        "only the typed row is a new allocation: {new_rows:?}"
    );
    assert!(
        typed[new_rows[0]]
            .iter()
            .any(|run| run.text.contains("hello!"))
    );
}

#[test]
fn banner_and_menu_geometry_stay_in_window_coordinates() {
    let mut app = test_app();
    app.open_tab(cat()).expect("tab");
    // The banner only shows for a single-leaf tab.
    assert!(app.banner_rows() > 0);
    app.menu = Some((1, 2));
    let frame = app.frame();
    let item = frame[1]
        .iter()
        .find(|run| run.text.starts_with(" New Tab"))
        .expect("menu item");
    assert_eq!(item.col, 2, "the menu is drawn where the click was");
    app.split_focused(Axis::Columns).expect("split");
    assert_eq!(
        app.banner_rows(),
        0,
        "a split tab never shows the start banner"
    );
    assert_eq!(app.frame().len(), app.grid_rows + 1);
}

#[test]
fn banner_art_spells_flamewm_with_every_glyph() {
    let art = &BANNER[..BANNER_ART_ROWS];
    let width = art[0].chars().count();
    assert!(
        art.iter().all(|row| row.chars().count() == width),
        "art rows must share one width"
    );
    // Seven letters, six distinct glyphs; the E glyph's middle row is the one that is easiest to lose.
    assert!(
        art[1].contains("| ____|"),
        "the E glyph is missing from the banner"
    );
    // Slanted: each row down starts one column further left than the row above.
    let indent = |row: &str| row.chars().take_while(|c| *c == ' ').count();
    assert!(
        art.windows(2).all(|pair| indent(pair[0]) > indent(pair[1])),
        "the art must lean right"
    );
    assert_eq!(
        art.iter().filter(|row| row.contains('_')).count(),
        BANNER_ART_ROWS,
        "every art row carries glyph strokes"
    );
}

#[test]
fn overlay_replaces_cells_and_keeps_the_runs_on_either_side() {
    let row = vec![text_run(0, "abcdef", 1, None, false)];
    let menu = text_run(2, "XY", 2, Some(3), false);
    let out = overlay(row, 2, 2, menu);
    let texts: Vec<(usize, usize, &str)> = out
        .iter()
        .map(|run| (run.col, run.cells, run.text.as_str()))
        .collect();
    assert_eq!(texts, vec![(0, 2, "ab"), (2, 2, "XY"), (4, 2, "ef")]);
}

#[test]
fn repeated_presses_on_one_cell_cycle_single_double_triple() {
    let t0 = Instant::now();
    let first = next_click_count(None, t0, 3, 4);
    assert_eq!(first, 1);
    let second = next_click_count(
        Some((t0, 3, 4, first)),
        t0 + Duration::from_millis(100),
        3,
        4,
    );
    assert_eq!(second, 2);
    let third = next_click_count(
        Some((t0, 3, 4, second)),
        t0 + Duration::from_millis(200),
        3,
        4,
    );
    assert_eq!(third, 3);
    let fourth = next_click_count(
        Some((t0, 3, 4, third)),
        t0 + Duration::from_millis(300),
        3,
        4,
    );
    assert_eq!(fourth, 1);
}

#[test]
fn slow_or_moved_presses_start_over() {
    let t0 = Instant::now();
    assert_eq!(
        next_click_count(Some((t0, 3, 4, 1)), t0 + Duration::from_millis(900), 3, 4),
        1
    );
    assert_eq!(
        next_click_count(Some((t0, 3, 4, 1)), t0 + Duration::from_millis(50), 3, 5),
        1
    );
    assert_eq!(
        next_click_count(Some((t0, 3, 4, 1)), t0 + Duration::from_millis(50), 2, 4),
        1
    );
}

#[test]
fn word_bounds_cover_the_word_under_the_column_only() {
    let chars: Vec<char> = "ls -la foo_bar.rs".chars().collect();
    assert_eq!(word_bounds(&chars, 0), Some((0, 1)));
    assert_eq!(word_bounds(&chars, 4), Some((4, 5)));
    assert_eq!(word_bounds(&chars, 9), Some((7, 13)));
    assert_eq!(word_bounds(&chars, 14), None);
    assert_eq!(word_bounds(&chars, 15), Some((15, 16)));
    assert_eq!(word_bounds(&chars, 2), None);
    assert_eq!(word_bounds(&chars, 99), None);
}

#[test]
fn sgr_mouse_press_and_release_are_one_based() {
    let press = mouse_report(true, 0, UiActionPhase::Press, 4, 2, MouseMode::Press).unwrap();
    assert_eq!(press, b"\x1b[<0;5;3M".to_vec());
    let release = mouse_report(true, 0, UiActionPhase::Release, 4, 2, MouseMode::Press).unwrap();
    assert_eq!(release, b"\x1b[<0;5;3m".to_vec());
}

#[test]
fn motion_is_reported_only_in_drag_or_any_modes() {
    assert!(mouse_report(true, 0, UiActionPhase::Motion, 1, 1, MouseMode::Press).is_none());
    assert_eq!(
        mouse_report(true, 0, UiActionPhase::Motion, 1, 1, MouseMode::Drag).unwrap(),
        b"\x1b[<32;2;2M".to_vec()
    );
}

#[test]
fn x10_reports_release_as_button_three_and_offsets_coordinates() {
    let release = mouse_report(false, 0, UiActionPhase::Release, 0, 0, MouseMode::Press).unwrap();
    assert_eq!(release, vec![0x1b, b'[', b'M', 35, 33, 33]);
}

#[test]
fn wheel_reports_use_buttons_64_and_65() {
    assert_eq!(
        mouse_report_raw(true, 64, true, 0, 0),
        b"\x1b[<64;1;1M".to_vec()
    );
    assert_eq!(
        mouse_report_raw(true, 65, true, 0, 0),
        b"\x1b[<65;1;1M".to_vec()
    );
}

/// Records only what the regression test reads; every other write is accepted without effect.
#[derive(Default)]
struct NodeBackdropSpy {
    backgrounds: Vec<String>,
    node_backdrops: Vec<(String, Option<u8>)>,
}

impl flamewm_ui_x11::UiDocumentAccess for NodeBackdropSpy {
    fn text(&mut self, _: &str, _: String) -> Result<(), String> {
        Ok(())
    }
    fn image_rgb8(&mut self, _: &str, _: flamewm_ui_x11::RuntimeImage) -> Result<(), String> {
        Ok(())
    }
    fn image_rgba8(&mut self, _: &str, _: flamewm_ui_x11::RuntimeImage) -> Result<(), String> {
        Ok(())
    }
    fn visible(&mut self, _: &str, _: bool) -> Result<(), String> {
        Ok(())
    }
    fn toggle(&mut self, _: &str, _: &str, _: bool) -> Result<(), String> {
        Ok(())
    }
    fn position(&mut self, _: &str, _: f32, _: f32) -> Result<(), String> {
        Ok(())
    }
    fn size(&mut self, _: &str, _: f32, _: f32) -> Result<(), String> {
        Ok(())
    }
    fn background(&mut self, id: &str, _: flamewm_ui_x11::UiColor) -> Result<(), String> {
        self.backgrounds.push(id.to_owned());
        Ok(())
    }
    fn border(&mut self, _: &str, _: flamewm_ui_x11::UiColor) -> Result<(), String> {
        Ok(())
    }
    fn node_backdrop(&mut self, id: &str, blur_percent: Option<u8>) -> Result<(), String> {
        self.node_backdrops.push((id.to_owned(), blur_percent));
        Ok(())
    }
    fn foreground(&mut self, _: &str, _: flamewm_ui_x11::UiColor) -> Result<(), String> {
        Ok(())
    }
    fn background_clear(&mut self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn border_clear(&mut self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn foreground_clear(&mut self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn font_size(&mut self, _: &str, _: f32) -> Result<(), String> {
        Ok(())
    }
    fn font_weight(&mut self, _: &str, _: u16) -> Result<(), String> {
        Ok(())
    }
    fn overflow(
        &mut self,
        _: &str,
        _: flamewm_ui_x11::Overflow,
        _: flamewm_ui_x11::Overflow,
    ) -> Result<(), String> {
        Ok(())
    }
    fn layer(&mut self, _: &str, _: flamewm_ui_core::style::UiLayer) -> Result<(), String> {
        Ok(())
    }
    fn font_family(&mut self, _: String) -> Result<(), String> {
        Ok(())
    }
    fn color_variable(&mut self, _: &str, _: flamewm_ui_x11::UiColor) -> Result<(), String> {
        Ok(())
    }
}

/// Regression: a translucent terminal blurs through the window backdrop only. A root node backdrop read the whole
/// backbuffer back and re-blurred it on every paint.
#[test]
fn a_translucent_terminal_paints_no_root_node_backdrop() {
    let mut app = test_app();
    app.surface_alpha = crate::theme::surface_alpha(80);
    let mut doc = NodeBackdropSpy::default();
    app.project(&mut doc).expect("project");
    assert_eq!(
        doc.backgrounds,
        vec!["terminal".to_owned()],
        "the translucent fill is still projected"
    );
    assert!(
        doc.node_backdrops.is_empty(),
        "no node backdrop writes: {:?}",
        doc.node_backdrops
    );
}
