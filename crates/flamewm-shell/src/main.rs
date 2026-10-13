mod app;
mod event_router;
use flamewm_shell::settings_projection;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use flamewm_api::ports::FdEvents;
use flamewm_control_dbus::{ControlClient, ControlSignalClient};
use flamewm_control_wire::ControlSignal;
use flamewm_dbus_reactor::BusKind;
use flamewm_reactor::{Reactor, SystemWallClock, WallClockSource};
use flamewm_shell::async_projection;
use flamewm_shell::projection;
use flamewm_shell::start;
use flamewm_shell::{ShellSnapshot, ShellSurfaces};
use flamewm_ui_x11::{run_surface_runtime_with_reactor_access, SurfaceEvent, SurfaceRuntime};

use app::ShellLoop;

fn main() {
    // J07 role branching at process entry: `--quick-control-host` runs the
    // helper role (profiler `flamewm-quick-control`, owns
    // audio/network/calendar surfaces); otherwise the normal shell role
    // (profiler `flamewm-shell`, owns panel/start/task menu/media).
    if std::env::args().any(|arg| arg == "--quick-control-host") {
        if let Err(error) = flamewm_shell::quick_controls::run_quick_control_host() {
            eprintln!("flamewm-quick-control: {error}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(error) = execute() {
        eprintln!("flamewm-shell: {error}");
        std::process::exit(1);
    }
}

fn execute() -> Result<(), String> {
    flamewm_profiler::init_process("flamewm-shell");
    flamewm_debug::init_process("flamewm-shell");
    let total = flamewm_profiler::ProfilePoint::new("shell.startup.total");
    let total_guard = total.start();
    let client = {
        let scope = flamewm_profiler::ProfilePoint::new("shell.startup.control_connect");
        let _s = scope.start();
        ControlClient::connect(BusKind::Session)
            .map_err(|error| format!("connect control session: {}", error.message))?
    };
    let snapshot = {
        let scope = flamewm_profiler::ProfilePoint::new("shell.startup.bootstrap_call");
        let _s = scope.start();
        ShellSnapshot::load(&client)?
    };
    // Settings are not part of the shell bootstrap aggregate. Fetch the
    // current settings once here so the first projection applies saved
    // presentation values (font family/size, accent, taskbar color) instead
    // of waiting for a live SettingsSnapshotChanged signal. Nonfatal: defaults
    // remain in effect when the fetch fails.
    let startup_settings = match client.call(&flamewm_control_core::ControlRequest::GetSettings) {
        Ok(flamewm_control_core::ControlResponse::Settings(snapshot)) => Some(snapshot),
        Ok(response) => {
            eprintln!("flamewm-shell: GetSettings returned {response:?} (nonfatal)");
            None
        }
        Err(error) => {
            eprintln!("flamewm-shell: GetSettings failed (nonfatal): {}", error.message);
            None
        }
    };
    // The startup client owns only the bootstrap roundtrip. Steady-state
    // mutations use the dispatcher worker and snapshots arrive by signal.
    drop(client);
    let mut runtime = {
        let scope = flamewm_profiler::ProfilePoint::new("shell.startup.runtime_create");
        let _s = scope.start();
        SurfaceRuntime::new().map_err(|error| format!("create UI runtime: {error:?}"))?
    };
    let mut surfaces = {
        let scope = flamewm_profiler::ProfilePoint::new("shell.startup.surface_create");
        let _s = scope.start();
        ShellSurfaces::create(&mut runtime, &snapshot)?
    };
    let start_model = start::state(snapshot.applications.clone());
    {
        let scope = flamewm_profiler::ProfilePoint::new("shell.startup.panel_project");
        let _s = scope.start();
        project_panel_critical(&mut runtime, &surfaces, &snapshot, &start_model)?;
    }
    // Snapshot before the first map: the bar's own translucent pixels must not be captured.
    if let Err(error) = surfaces.capture_panel_backdrop(&mut runtime) {
        eprintln!("flamewm-shell: taskbar backdrop capture skipped: {error}");
    }
    {
        let scope = flamewm_profiler::ProfilePoint::new("shell.startup.panel_show");
        let _s = scope.start();
        for panel in surfaces.panel_handles() {
            runtime
                .show(panel)
                .map_err(|e| format!("show panel: {e:?}"))?;
        }
        // Our own Map changes the root under the bar: a blank earlier capture gets a fresh attempt.
        surfaces.reopen_panel_capture();
    }
    let loader_root = packaged_root();

    let profile_interval = profile_interval_secs();
    let profile_due = Arc::new(AtomicBool::new(false));
    let profile_flag = Arc::clone(&profile_due);

    let dispatcher = flamewm_control_dbus::MutationDispatcher::start(BusKind::Session, 64).ok();
    let mut reactor = {
        let scope = flamewm_profiler::ProfilePoint::new("shell.startup.reactor_enter");
        let _s = scope.start();
        Reactor::new().map_err(|error| format!("create reactor: {error}"))?
    };
    drop(total_guard);
    let clock_due = Arc::new(AtomicBool::new(false));
    let plan = flamewm_shell_core::clock_timer_plan("%H:%M", SystemWallClock.now_epoch_ms());
    let initial_due = Arc::clone(&clock_due);
    reactor
        .register_timer(
            Duration::from_millis(plan.initial_delay_ms),
            false,
            move || initial_due.store(true, Ordering::Release),
        )
        .map_err(|error| format!("register clock alignment timer: {error}"))?;
    let repeat_due = Arc::clone(&clock_due);
    reactor
        .register_timer(Duration::from_millis(plan.interval_ms), true, move || {
            repeat_due.store(true, Ordering::Release)
        })
        .map_err(|error| format!("register clock timer: {error}"))?;

    let state = Rc::new(RefCell::new(ShellLoop::new(
        flamewm_shell::ShellRuntime::new(snapshot),
        surfaces,
        start_model,
        dispatcher,
        loader_root,
        current_exe_program(),
    )));
    if let Some(snapshot) = startup_settings {
        // Same entry point as the live SettingsSnapshotChanged path.
        state.borrow_mut().apply_settings_snapshot(snapshot);
    }
    if let Some(dispatcher) = state.borrow().dispatcher.as_ref() {
        let watch_fd = dispatcher.wake_fd();
        let wake_dispatcher = Arc::clone(dispatcher);
        let mutation_state = Rc::clone(&state);
        reactor
            .register_raw_fd_with_action(watch_fd, calloop::Interest::READ, move |_, _| {
                flamewm_shell::control_actions::drain_results_with_settings(
                    Some(&wake_dispatcher),
                    |snapshot| {
                        mutation_state
                            .borrow_mut()
                            .apply_settings_snapshot(snapshot)
                    },
                );
                flamewm_reactor::FdAction::Continue
            })
            .map_err(|error| format!("register mutation result source: {error}"))?;
    }
    // J07 parent: start the helper asynchronously after panel show. The
    // parent never blocks on the helper; spawn failure is nonfatal.
    state.borrow_mut().start_helper_after_panel();
    let event_state = Rc::clone(&state);
    let tick_state = Rc::clone(&state);
    let signal_state = Rc::clone(&state);
    // Signal subscription into the same Reactor: snapshot signals apply
    // their payload directly in `mark_signal` (zero fetch); legacy
    // revision-only signals are ignored for normal refresh. The tick
    // drains applied domains once per turn.
    let signal_client = ControlSignalClient::connect(BusKind::Session)
        .map_err(|error| format!("subscribe control signals: {}", error))?;
    // Leak the client for process lifetime: its pump fd must stay open for the
    // reactor registration below, and the reactor never owns the descriptor.
    let signal_client: &'static ControlSignalClient = Box::leak(Box::new(signal_client));
    {
        let watch = signal_client.watch();
        let interest = signal_interest(watch.events);
        reactor
            .register_raw_fd_with_action(watch.fd, interest, move |_, _| {
                let _ = signal_client.on_ready(|signal: ControlSignal| {
                    signal_state.borrow_mut().note_signal(&signal);
                });
                flamewm_reactor::FdAction::Continue
            })
            .map_err(|error| format!("register control signal source: {error}"))?;
    }
    let tick_profile_flag = Arc::clone(&profile_due);
    reactor
        .register_timer(Duration::from_secs(5), false, move || {
            tick_profile_flag.store(true, Ordering::Release);
        })
        .map_err(|error| format!("register profile one-shot: {error}"))?;
    // Canonical IconService wake FD: registered before the run loop so
    // worker completions wake the reactor; the tick drains them.
    {
        let icon_fd = state.borrow_mut().icon_wake_fd();
        let icon_state = Rc::clone(&state);
        reactor
            .register_raw_fd_with_action(icon_fd, calloop::Interest::READ, move |_, _| {
                icon_state.borrow_mut().drain_icons_pending();
                flamewm_reactor::FdAction::Continue
            })
            .map_err(|error| format!("register icon result source: {error}"))?;
    }
    reactor
        .register_timer(Duration::from_secs(profile_interval), true, move || {
            profile_flag.store(true, Ordering::Release)
        })
        .map_err(|error| format!("register profile timer: {error}"))?;
    run_surface_runtime_with_reactor_access(
        &mut runtime,
        &mut reactor,
        move |event: SurfaceEvent, runtime| event_state.borrow_mut().route_event(event, runtime),
        {
            let tick_profile = Arc::clone(&profile_due);
            move |runtime| {
                if tick_profile.swap(false, Ordering::Acquire) {
                    let _ = flamewm_profiler::report_window();
                }
                tick_state.borrow_mut().tick(runtime, &clock_due)
            }
        },
    )
    .map_err(|error| format!("surface runtime: {error:?}"))
}

/// Parent helper program: re-exec this binary with the helper role flag so
/// no second binary or install path is required.
fn current_exe_program() -> String {
    std::env::current_exe()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "flamewm-shell".to_owned())
}

fn project_panel_critical(
    runtime: &mut SurfaceRuntime,
    surfaces: &ShellSurfaces,
    snapshot: &ShellSnapshot,
    start_model: &flamewm_shell_core::StartModel,
) -> Result<(), String> {
    projection::for_each_panel_record(
        snapshot,
        surfaces.panel_outputs(),
        |handle, view| {
            runtime.with_document(handle, |document| {
                async_projection::project_panel_async(document, view, None)
            })
        },
    )
    .map_err(|error| format!("project panel: {error:?}"))?;
    runtime
        .with_document(surfaces.start, |document| {
            async_projection::project_start_unified(
                document,
                start_model,
                start::StartCategory::All,
                "",
                &snapshot.session,
                None,
            )
        })
        .map_err(|error| format!("project start: {error:?}"))?;
    Ok(())
}

fn signal_interest(events: FdEvents) -> calloop::Interest {
    match (
        events.contains(FdEvents::READABLE),
        events.contains(FdEvents::WRITABLE),
    ) {
        (true, true) => calloop::Interest::BOTH,
        (false, true) => calloop::Interest::WRITE,
        _ => calloop::Interest::READ,
    }
}

fn profile_interval_secs() -> u64 {
    flamewm_profiler::profile_interval_secs()
}

fn packaged_root() -> PathBuf {
    let manifest_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .expect("flamewm workspace root");
    resolve_packaged_root(
        std::env::current_exe().ok().as_deref(),
        &manifest_root,
        |root| root.join("assets").is_dir(),
    )
}

// Mirrors flamewm-desktop's resolve_packaged_root: an installed
// /usr/bin/flamewm-shell resolves to /usr/share/flamewm when it has assets/,
// otherwise the checkout manifest root is used.
fn resolve_packaged_root(
    executable: Option<&Path>,
    manifest_root: &Path,
    assets_available: impl FnOnce(&Path) -> bool,
) -> PathBuf {
    executable
        .and_then(installed_data_root)
        .filter(|root| assets_available(root))
        .unwrap_or_else(|| manifest_root.to_path_buf())
}

fn installed_data_root(executable: &Path) -> Option<PathBuf> {
    let bin_dir = executable.parent()?;
    if bin_dir.file_name()? != "bin" {
        return None;
    }
    let usr_dir = bin_dir.parent()?;
    if usr_dir.file_name()? != "usr" {
        return None;
    }
    Some(usr_dir.join("share/flamewm"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_clock_is_utc_epoch_millis() {
        let now = SystemWallClock.now_epoch_ms();
        let utc = chrono::Utc::now().timestamp_millis() as u64;
        assert!(
            now.abs_diff(utc) < 1_000,
            "wall clock {now} vs chrono UTC {utc}"
        );
    }
}
