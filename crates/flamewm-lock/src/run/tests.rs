use std::time::Duration;

use flamewm_ui_x11::SurfaceRuntime;

use super::preview_mode::preview_lock_ui;
use super::*;

#[test]
fn live_settings_detect_typography_and_wallpaper_changes() {
    let defaults = LockSettings::default();
    let bold = LockSettings {
        font_bold: true,
        text_shadow: true,
        ..defaults.clone()
    };
    assert!(typography_changed(&defaults, &bold));
    assert!(!background_changed(&defaults, &bold));
    let shared = LockSettings {
        share_wallpaper: true,
        ..defaults.clone()
    };
    assert!(background_changed(&defaults, &shared));
    assert!(!typography_changed(&defaults, &shared));
}

#[test]
fn control_signal_reducer_keeps_the_latest_typed_snapshots() {
    let settings = flamewm_api::settings::SettingsSnapshot::new(2);
    let displays = DisplaySnapshot {
        generation: 3,
        outputs: Vec::new(),
        pending: None,
    };
    let (settings, displays) = latest_control_snapshots([
        ControlSignal::SettingsSnapshotChanged {
            snapshot: flamewm_api::settings::SettingsSnapshot::new(1),
        },
        ControlSignal::SettingsSnapshotChanged { snapshot: settings },
        ControlSignal::DisplaysSnapshotChanged { snapshot: displays },
    ]);
    assert_eq!(settings.expect("settings snapshot").revision, 2);
    assert_eq!(displays.expect("display snapshot").generation, 3);
}

#[test]
fn display_rect_changes_rebuild_even_when_root_size_is_unchanged() {
    let root_before = Rect::new(0, 0, 1920, 1080);
    let root_after = Rect::new(0, 0, 1920, 1080);
    let current = [Rect::new(0, 0, 1920, 1080)];
    let rearranged = [Rect::new(0, 0, 1280, 720), Rect::new(1280, 0, 640, 1080)];
    assert!(!output_rects_need_rebuild(&current, &current));
    assert!(output_rects_need_rebuild(&current, &rearranged));
    assert_eq!(root_before, root_after, "root geometry remains unchanged");
}

#[test]
fn live_settings_projection_does_not_replace_authentication_owners() {
    let body = method_body("fn update_live_settings(");
    assert!(!body.contains("self.state"));
    assert!(!body.contains("outcome_tx") && !body.contains("outcome_rx"));
    assert!(body.contains("live_typography_dirty"));
}

#[test]
fn live_output_projection_uses_the_same_canonical_clipping_fallback() {
    let root = Rect::new(0, 0, 1920, 1080);
    let empty = DisplaySnapshot {
        generation: 1,
        outputs: Vec::new(),
        pending: None,
    };
    assert_eq!(
        crate::session::lock_output_rects_from_snapshot(&empty, root),
        vec![root]
    );
}

/// Needs a live X display (`DISPLAY`). Creates and drops the render stack
/// normally: a regression here is a SIGSEGV at `XCloseDisplay`.
#[test]
#[ignore = "requires DISPLAY"]
fn runtime_drops_cleanly_after_xft_and_xrender_load() {
    let runtime = SurfaceRuntime::new().expect("create UI runtime");
    drop(runtime);
}

/// The shared preview body must never reach a grab, PAM, a screensaver or DPMS.
#[test]
fn preview_loop_has_no_grab_auth_screensaver_or_dpms_call_sites() {
    let source = run_sources();
    let start = source.find("fn preview_loop(").expect("preview loop");
    let end = start + source[start..].find("\n}\n").expect("preview loop end");
    let body = &source[start..end];
    for forbidden in [
        "set_display_power",
        "grab_pointer",
        "grab_with_retry",
        "try_grab",
        "pam::",
        "Screensaver::new",
        "perform_on_worker",
    ] {
        assert!(
            !body.contains(forbidden),
            "preview path reaches {forbidden}"
        );
    }
}

/// The Ribbons preview builds no lock projection; the lock preview builds exactly one.
#[test]
fn ribbons_preview_never_projects_the_lock_ui() {
    let mut calls = 0;
    assert_eq!(preview_lock_ui(true, || calls += 1), None);
    assert_eq!(calls, 0, "ribbons preview called the lock projection");
    assert_eq!(
        preview_lock_ui(false, || {
            calls += 1;
            7
        }),
        Some(7)
    );
    assert_eq!(calls, 1);
}

/// A failing lock-screen step is logged and reported, never propagated: the loop
/// keeps its grabs and retries on a later turn.
#[test]
fn failing_lock_step_is_logged_and_never_fatal() {
    let mut attempts = 0;
    let ok = keep_lock_step("test step", || {
        attempts += 1;
        Err("document unavailable".to_string())
    });
    assert!(!ok);
    assert_eq!(attempts, 1);
    assert!(keep_lock_step("test step", || Ok(())));
}

/// Body of the method in this file whose signature starts with `name`.
fn method_body(name: &str) -> String {
    let source = run_sources();
    let start = source.find(name).expect("function present");
    let end = start + source[start..].find("\n    }\n").expect("function end");
    source[start..end].to_string()
}

/// Every module of the lock loop, the settings preview first, so the source-contract
/// checks see the whole loop wherever its functions now live.
fn run_sources() -> String {
    [
        include_str!("preview_mode.rs"),
        include_str!("../run.rs"),
        include_str!("change.rs"),
        include_str!("control_watch.rs"),
        include_str!("idle.rs"),
        include_str!("input.rs"),
        include_str!("live.rs"),
        include_str!("paint.rs"),
        include_str!("steps.rs"),
        include_str!("surface.rs"),
        include_str!("surfaces.rs"),
        include_str!("tick.rs"),
    ]
    .join("\n")
}

/// The loop-reachable functions must not propagate with `?`: a rendering or
/// background error there would end `tick()` and drop the lock. The only tolerated
/// propagation is the destroy after a successful authentication.
#[test]
fn lock_loop_paths_never_propagate_render_errors() {
    let body = method_body;
    for name in [
        "fn wake(",
        "fn enter(",
        "fn animate(",
        "fn restore_background(",
        "fn repaint(",
    ] {
        assert!(!body(name).contains("-> Result"), "{name} returns Result");
    }
    for name in [
        "fn wake(",
        "fn enter(",
        "fn animate(",
        "fn restore_background(",
    ] {
        assert_eq!(body(name).matches(")?").count(), 0, "{name} propagates");
    }
    assert_eq!(body("fn tick(").matches(")?").count(), 1, "tick propagates");
}

/// Hiding on entry and restoring on wake must both exist, or the widgets stay hidden.
#[test]
fn saver_hides_lock_ui_on_entry_and_restores_on_wake() {
    let body = method_body;
    assert!(body("fn enter(").contains("ui_shown_wanted = false"));
    assert!(body("fn wake(").contains("ui_shown_wanted = true"));
    assert!(body("fn sync_ui_visibility(").contains("set_lock_ui_shown(view, shown)"));
}

/// The projection is skipped only while the paint key matches; every input bumps a revision.
#[test]
fn repaint_skips_projection_on_unchanged_paint_key() {
    let body = method_body;
    assert!(body("fn repaint(").contains("self.painted_key == Some(key)"));
    assert!(body("fn power_event(").contains("self.power_revision += 1"));
    assert!(body("fn update_live_settings(").contains("self.settings_revision += 1"));
}

/// A refused wallpaper restore must stay pending and be retried; a success clears it.
#[test]
fn failed_wallpaper_restore_stays_pending_until_it_succeeds() {
    let mut stale = false;
    let mut attempts = 0;
    settle(&mut stale, "test restore", || {
        attempts += 1;
        Err("document unavailable".to_string())
    });
    assert!(
        stale,
        "a refused restore must stay pending, not drop the Ribbons frame silently"
    );
    settle(&mut stale, "test restore", || {
        attempts += 1;
        Ok(())
    });
    assert!(!stale, "a successful retry clears the pending restore");
    assert_eq!(attempts, 2);
}

/// A refused widget update must keep the loop alive: it is retried on the next turn,
/// and a success clears the dirty flag.
#[test]
fn failed_widget_visibility_is_retried_next_turn() {
    let mut dirty = true;
    let mut attempts = 0;
    settle(&mut dirty, "test visibility", || {
        attempts += 1;
        Err("document unavailable".to_string())
    });
    assert!(dirty, "a failed update stays dirty for the next tick");
    settle(&mut dirty, "test visibility", || {
        attempts += 1;
        Ok(())
    });
    assert!(!dirty, "a successful retry clears the flag");
    assert_eq!(attempts, 2, "the retry re-applies the same wanted state");
}

/// Counts the frame wakes that arrive within `window`.
fn wakes_within(reader: &mut UnixStream, window: Duration) -> usize {
    let deadline = Instant::now() + window;
    let mut count = 0;
    let mut byte = [0u8; 1];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return count;
        }
        reader
            .set_read_timeout(Some(remaining))
            .expect("read timeout");
        match reader.read(&mut byte) {
            Ok(1) => count += 1,
            _ => return count,
        }
    }
}

#[test]
fn frame_pacer_wakes_only_while_the_animation_runs() {
    let (mut reader, writer) = UnixStream::pair().expect("socket pair");
    let pacer = FramePacer::start(writer).expect("start pacer");
    assert_eq!(
        wakes_within(&mut reader, Duration::from_millis(150)),
        0,
        "idle pacer must stay silent"
    );
    pacer.set_running(true);
    assert!(
        wakes_within(&mut reader, Duration::from_millis(300)) >= 3,
        "active pacer ticks at frame rate"
    );
    pacer.set_running(false);
    thread::sleep(FRAME_INTERVAL * 2);
    let _ = wakes_within(&mut reader, Duration::from_millis(10));
    assert_eq!(
        wakes_within(&mut reader, Duration::from_millis(150)),
        0,
        "stopped pacer must stay silent"
    );
}
