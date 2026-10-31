mod app;
mod event_router;
mod launch_feedback;
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
use flamewm_dbus_reactor::{BusKind, BusWatch};
use flamewm_reactor::{Reactor, SystemWallClock, WallClockSource};
use flamewm_shell::async_projection;
use flamewm_shell::projection;
use flamewm_shell::start;
use flamewm_shell::{ShellSnapshot, ShellSurfaces};
use flamewm_ui_x11::{
    decode_document, run_surface_runtime_with_reactor_access, SurfaceConfig, SurfaceEvent,
    SurfaceHandle, SurfaceInputMode, SurfaceRole, SurfaceRuntime, UiDocumentAccess, UiTemplate,
};

use app::ShellLoop;

const NOTIFICATION_ARTIFACT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-notification.rwr"));
const NOTIFICATION_WIDTH: u32 = 360;
const NOTIFICATION_HEIGHT: u32 = 148;
/// Once the startup burst (icon loads, backdrop decode, first paints) has settled, its freed
/// heap is handed back; later churn is trimmed on the minute clock tick.
const STARTUP_HEAP_TRIM_DELAY: Duration = Duration::from_secs(10);

struct NotificationHost {
    provider: flamewm_integrations_linux::notifications::NotificationProvider,
    surface: SurfaceHandle,
    visible: Option<flamewm_integrations_linux::notifications::NotificationView>,
}

impl NotificationHost {
    fn connect(
        runtime: &mut SurfaceRuntime,
        snapshot: &ShellSnapshot,
    ) -> Result<Option<Self>, String> {
        let mut provider =
            flamewm_integrations_linux::notifications::NotificationProvider::connect()
                .map_err(|error| error.message)?;
        if !provider.start().map_err(|error| error.message)? {
            return Ok(None);
        }
        let Some(output) = snapshot
            .displays
            .outputs
            .iter()
            .find(|output| output.connected)
        else {
            let _ = provider.stop();
            return Ok(None);
        };
        let x = output
            .geometry
            .x
            .saturating_add(output.geometry.width)
            .saturating_sub(i32::try_from(NOTIFICATION_WIDTH).unwrap_or(i32::MAX))
            .saturating_sub(16);
        let y = output.geometry.y.saturating_add(16);
        let document = decode_document(NOTIFICATION_ARTIFACT)
            .map_err(|error| format!("decode notification UI: {error:?}"))?;
        let surface = runtime
            .create_surface(
                UiTemplate::new(document),
                SurfaceConfig {
                    width: NOTIFICATION_WIDTH,
                    height: NOTIFICATION_HEIGHT,
                    title: "FlameWM notification".to_owned(),
                    role: SurfaceRole::Overlay,
                    input: SurfaceInputMode::Interactive,
                    initially_visible: false,
                    x,
                    y,
                },
            )
            .map_err(|error| format!("create notification surface: {error:?}"))?;
        Ok(Some(Self {
            provider,
            surface,
            visible: None,
        }))
    }

    fn watch(&self) -> BusWatch {
        self.provider.watch()
    }

    fn on_ready(&mut self) -> Result<(), String> {
        self.provider
            .on_ready()
            .map(|_| ())
            .map_err(|error| error.message)
    }

    fn handle_event(&mut self, event: &SurfaceEvent, runtime: &mut SurfaceRuntime) -> bool {
        if event.surface != self.surface
            || event.action.action != "notification.dismiss"
            || event.action.phase != flamewm_ui_x11::ActionPhase::Release
            || !event.action.inside
        {
            return false;
        }
        if let Some(notification) = &self.visible {
            if let Err(error) = self.provider.dismiss(notification.id) {
                eprintln!(
                    "flamewm-shell: notification dismiss failed: {}",
                    error.message
                );
            }
        }
        if let Err(error) = self.tick(runtime, None) {
            eprintln!("flamewm-shell: notification update failed: {error}");
        }
        true
    }

    fn tick(
        &mut self,
        runtime: &mut SurfaceRuntime,
        wake: Option<&flamewm_reactor::WakeDeadline>,
    ) -> Result<(), String> {
        self.provider
            .expire(std::time::Instant::now())
            .map_err(|error| error.message)?;
        let _ = self.provider.drain_ui_updates();
        let next = self.provider.active().into_iter().last();
        if self.visible == next {
            if let Some(deadline) = self.provider.next_deadline() {
                if let Some(wake) = wake {
                    wake.set(Some(wake.get().map_or(deadline, |old| old.min(deadline))));
                }
            }
            return Ok(());
        }
        self.visible.clone_from(&next);
        if let Some(notification) = next {
            runtime
                .with_document(self.surface, |document| {
                    document.text("notification-app", display_text(&notification.app_name, 80))?;
                    document.text(
                        "notification-summary",
                        display_text(&notification.summary, 120),
                    )?;
                    document.text("notification-body", display_text(&notification.body, 500))?;
                    document.visible("notification-progress", notification.progress.is_some())?;
                    if let Some(progress) = notification.progress {
                        document.size(
                            "notification-progress-fill",
                            3.32 * f32::from(progress),
                            4.0,
                        )?;
                    }
                    Ok(())
                })
                .map_err(|error| format!("project notification: {error:?}"))?;
            runtime
                .show(self.surface)
                .map_err(|error| format!("show notification: {error:?}"))?;
        } else {
            runtime
                .hide(self.surface)
                .map_err(|error| format!("hide notification: {error:?}"))?;
        }
        if let Some(deadline) = self.provider.next_deadline() {
            if let Some(wake) = wake {
                wake.set(Some(wake.get().map_or(deadline, |old| old.min(deadline))));
            }
        }
        Ok(())
    }
}

fn display_text(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let text: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{text}…")
    } else {
        text
    }
}

pub fn main() {
    flamewm_ui_x11::return_large_frees_to_system();
    // J07 role branching at process entry: `--quick-control-host` runs the
    // helper role (profiler `flamewm-quick-control`, owns
    // audio/network/calendar surfaces); otherwise the normal shell role
    // (profiler `flamewm-shell`, owns panel/start/task menu/media).
    if std::env::args().any(|arg| arg == "--quick-control-host") {
        if let Err(error) = flamewm_shell::quick_controls::run_quick_control_host(packaged_root()) {
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
            eprintln!(
                "flamewm-shell: GetSettings failed (nonfatal): {}",
                error.message
            );
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
    let surfaces = {
        let scope = flamewm_profiler::ProfilePoint::new("shell.startup.surface_create");
        let _s = scope.start();
        ShellSurfaces::create(&mut runtime, &snapshot)?
    };
    let start_model = start::state(snapshot.applications.clone());
    {
        let scope = flamewm_profiler::ProfilePoint::new("shell.startup.panel_project");
        let _s = scope.start();
        project_panel_critical(
            &mut runtime,
            &surfaces,
            &snapshot,
            &start_model,
            settings_projection::taskbar_items(startup_settings.as_ref()),
        )?;
    }
    {
        let scope = flamewm_profiler::ProfilePoint::new("shell.startup.panel_show");
        let _s = scope.start();
        for panel in surfaces.panel_handles() {
            runtime
                .show(panel)
                .map_err(|e| format!("show panel: {e:?}"))?;
        }
    }
    let loader_root = packaged_root();
    // Separate X connection observes freedesktop startup notifications from
    // Flame launchers *and* compliant external launchers (Dolphin/xdg-open).
    // Failure is nonfatal: a missing X observer must not stop the desktop.
    let badge = match launch_feedback::LaunchFeedback::connect(loader_root.clone()) {
        Ok(observer) => Some(Rc::new(RefCell::new(observer))),
        Err(error) => {
            eprintln!("flamewm-shell: launch feedback disabled: {error}");
            None
        }
    };
    let notifications = match NotificationHost::connect(&mut runtime, &snapshot) {
        Ok(Some(host)) => Some(Rc::new(RefCell::new(host))),
        Ok(None) => None,
        Err(error) => {
            eprintln!("flamewm-shell: native notifications disabled: {error}");
            None
        }
    };

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
    // One self-realigning wall-clock timer: fires only at minute boundaries, so the
    // displayed "%H:%M" changes on the tick it is due (no free-running second timer).
    let minute_due = Arc::clone(&clock_due);
    reactor
        .register_wall_clock(SystemWallClock, false, move || {
            minute_due.store(true, Ordering::Release);
        })
        .map_err(|error| format!("register clock timer: {error}"))?;
    reactor
        .register_timer(
            STARTUP_HEAP_TRIM_DELAY,
            false,
            flamewm_ui_x11::release_free_heap,
        )
        .map_err(|error| format!("register startup heap trim: {error}"))?;

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
                            .apply_settings_snapshot(snapshot);
                    },
                );
                flamewm_reactor::FdAction::Continue
            })
            .map_err(|error| format!("register mutation result source: {error}"))?;
    }
    // J07 parent: start the helper asynchronously after panel show. The
    // parent never blocks on the helper; spawn failure is nonfatal.
    state.borrow_mut().start_helper_after_panel();
    if let Some(watch) = state.borrow_mut().start_job_view_provider() {
        let job_state = Rc::clone(&state);
        reactor
            .register_raw_fd_with_action(watch.fd, signal_interest(watch.events), move |_, _| {
                job_state.borrow_mut().job_view_ready();
                flamewm_reactor::FdAction::Continue
            })
            .map_err(|error| format!("register KIO job progress source: {error}"))?;
    }
    if let Some(host) = notifications.as_ref() {
        let watch = host.borrow().watch();
        let notification_state = Rc::clone(host);
        reactor
            .register_raw_fd_with_action(watch.fd, signal_interest(watch.events), move |_, _| {
                match notification_state.borrow_mut().on_ready() {
                    Ok(()) => flamewm_reactor::FdAction::Continue,
                    Err(error) => {
                        eprintln!("flamewm-shell: notification D-Bus source failed: {error}");
                        flamewm_reactor::FdAction::Remove
                    }
                }
            })
            .map_err(|error| format!("register notification source: {error}"))?;
    }
    let event_state = Rc::clone(&state);
    let tick_state = Rc::clone(&state);
    let signal_state = Rc::clone(&state);
    // Signal subscription into the same Reactor: snapshot signals apply
    // their payload directly in `mark_signal` (zero fetch); legacy
    // revision-only signals are ignored for normal refresh. The tick
    // drains applied domains once per turn.
    let signal_client = ControlSignalClient::connect(BusKind::Session)
        .map_err(|error| format!("subscribe control signals: {error}"))?;
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
    if let Some(observer) = badge.as_ref() {
        let observer = Rc::clone(observer);
        let fd = observer.borrow().fd();
        reactor
            .register_raw_fd_with_action(fd, calloop::Interest::READ, move |_, _| {
                match observer.borrow_mut().on_ready() {
                    Ok(()) => flamewm_reactor::FdAction::Continue,
                    Err(error) => {
                        eprintln!("flamewm-shell: launch notification observer failed: {error}");
                        flamewm_reactor::FdAction::Remove
                    }
                }
            })
            .map_err(|error| format!("register launch observer: {error}"))?;
    }
    // Volume OSD and weather: the tick publishes their deadlines to the reactor wake slot.
    state.borrow_mut().attach_wake(reactor.wake_deadline());
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
    if flamewm_profiler::enabled() {
        reactor
            .register_timer(Duration::from_secs(profile_interval), true, move || {
                profile_flag.store(true, Ordering::Release);
            })
            .map_err(|error| format!("register profile timer: {error}"))?;
    }
    let mut badge_tick = badge;
    let shared_wake = reactor.wake_deadline();
    run_surface_runtime_with_reactor_access(
        &mut runtime,
        &mut reactor,
        {
            let notification_state = notifications.clone();
            move |event: SurfaceEvent, runtime| {
                if notification_state
                    .as_ref()
                    .is_some_and(|host| host.borrow_mut().handle_event(&event, runtime))
                {
                    Ok(())
                } else {
                    event_state.borrow_mut().route_event(event, runtime)
                }
            }
        },
        {
            let tick_profile = Arc::clone(&profile_due);
            move |runtime| {
                if tick_profile.swap(false, Ordering::Acquire) {
                    let _ = flamewm_profiler::report_window();
                }
                let pointer = if tick_state.borrow().needs_output_pointer() {
                    badge_tick
                        .as_ref()
                        .and_then(|observer| observer.borrow().pointer_position())
                } else {
                    None
                };
                tick_state.borrow_mut().set_output_pointer(pointer);
                tick_state.borrow_mut().tick(runtime, &clock_due)?;
                if let Some(host) = notifications.as_ref() {
                    if let Err(error) = host.borrow_mut().tick(runtime, Some(&shared_wake)) {
                        eprintln!("flamewm-shell: notification UI update failed: {error}");
                    }
                }
                let badge_result = badge_tick.as_ref().map(|observer| {
                    let state = tick_state.borrow();
                    observer.borrow_mut().tick(
                        runtime,
                        &state.shell.snapshot().displays,
                        state.settings_cache.as_ref(),
                        std::time::Instant::now(),
                    )
                });
                match badge_result {
                    Some(Ok(Some(next))) => {
                        shared_wake.set(Some(shared_wake.get().map_or(next, |old| old.min(next))));
                    }
                    Some(Err(error)) => {
                        // An unavailable XShape/input overlay may disable the badge,
                        // but must never terminate the shell or its taskbar.
                        eprintln!("flamewm-shell: launch feedback disabled: {error:?}");
                        badge_tick = None;
                    }
                    _ => {}
                }
                Ok(())
            }
        },
    )
    .map_err(|error| format!("surface runtime: {error:?}"))
}

/// Parent helper program: re-exec this binary with the helper role flag so
/// no second binary or install path is required.
fn current_exe_program() -> String {
    std::env::current_exe().map_or_else(
        |_| "flamewm-shell".to_owned(),
        |path| path.to_string_lossy().into_owned(),
    )
}

fn project_panel_critical(
    runtime: &mut SurfaceRuntime,
    surfaces: &ShellSurfaces,
    snapshot: &ShellSnapshot,
    start_model: &flamewm_shell_core::StartModel,
    items: projection::TaskbarItems,
) -> Result<(), String> {
    projection::for_each_panel_record(snapshot, surfaces.panel_outputs(), |handle, view| {
        runtime.with_document(handle, |document| {
            async_projection::project_panel_async(
                document,
                view.with_items(items),
                None,
                &mut Default::default(),
            )
        })
    })
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

    #[test]
    fn notification_text_is_truncated_on_character_boundaries() {
        assert_eq!(display_text("short", 5), "short");
        assert_eq!(display_text("🙂🙂🙂", 2), "🙂🙂…");
    }

    #[test]
    fn notification_template_has_runtime_text_nodes() {
        let mut document = decode_document(NOTIFICATION_ARTIFACT).expect("notification template");
        for id in [
            "notification-app",
            "notification-summary",
            "notification-body",
        ] {
            document
                .text(id, "test".to_owned())
                .unwrap_or_else(|error| panic!("{id}: {error}"));
        }
    }
}
