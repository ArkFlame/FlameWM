mod app;
mod event_router;
mod launch_feedback;
use flamewm_shell::settings_projection;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use flamewm_api::ports::FdEvents;
use flamewm_api::settings::SettingsSnapshot;
use flamewm_control_dbus::{ControlClient, ControlSignalClient};
use flamewm_control_wire::ControlSignal;
use flamewm_dbus_reactor::{BusKind, BusWatch};
use flamewm_integrations_linux::notifications;
use flamewm_reactor::{Reactor, SystemWallClock};
use flamewm_shell::async_projection;
use flamewm_shell::bus_offline;
use flamewm_shell::icon_loader::{IconLoader, IconTarget};
use flamewm_shell::notification_center;
use flamewm_shell::projection;
use flamewm_shell::start;
use flamewm_shell::{ShellSnapshot, ShellSurfaces};
use flamewm_ui_x11::{
    decode_document, run_surface_runtime_with_reactor_access, RuntimeImage, SurfaceConfig,
    SurfaceEvent, SurfaceHandle, SurfaceInputMode, SurfaceRole, SurfaceRuntime, UiDocumentAccess,
    UiTemplate,
};

use app::ShellLoop;

const NOTIFICATION_ARTIFACT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-notification.rwr"));
const NOTIFICATION_WIDTH: u32 = 360;
const NOTIFICATION_HEIGHT: u32 = 148;
const NOTIFICATION_CENTER_ARTIFACT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-notification-center.rwr"));
const NOTIFICATION_CENTER_WIDTH: u32 = 380;
const NOTIFICATION_CENTER_HEIGHT: u32 = 520;
/// Gap between the center and the bottom panel it opens above (`.taskbar` is 44px tall).
const NOTIFICATION_CENTER_CLEARANCE: i32 = 44 + 16;
/// Once the startup burst (icon loads, backdrop decode, first paints) has settled, its freed
/// heap is handed back; later churn is trimmed on the minute clock tick.
const STARTUP_HEAP_TRIM_DELAY: Duration = Duration::from_secs(10);
/// Degraded-start retry cadence for the session control bus: at most one attempt per interval.
const BUS_RETRY_INTERVAL: Duration = Duration::from_secs(30);

struct NotificationHost {
    provider: flamewm_integrations_linux::notifications::NotificationProvider,
    surface: SurfaceHandle,
    visible: Option<flamewm_integrations_linux::notifications::NotificationView>,
    center: SurfaceHandle,
    center_open: bool,
    center_view: notification_center::CenterView,
    /// History revision the center was last projected from; `None` forces a projection.
    center_revision: Option<u64>,
    /// Active notification ids the center was last projected with; toast expiry changes them.
    center_active: Vec<u32>,
    /// Set when an outside press closed the centre; consumes the same click's opener release.
    closed_by_press: Option<Instant>,
    /// Loads toast icons by theme name; spawned on the first named icon.
    icons: Option<IconLoader>,
    icon_root: PathBuf,
    /// True while an icon request for the visible toast is outstanding.
    icon_wait: bool,
}

impl NotificationHost {
    fn connect(
        runtime: &mut SurfaceRuntime,
        snapshot: &ShellSnapshot,
        icon_root: PathBuf,
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
        let center_width = i32::try_from(NOTIFICATION_CENTER_WIDTH).unwrap_or(i32::MAX);
        let center_height = i32::try_from(NOTIFICATION_CENTER_HEIGHT).unwrap_or(i32::MAX);
        let center_x = output
            .geometry
            .x
            .saturating_add(output.geometry.width)
            .saturating_sub(center_width)
            .saturating_sub(16);
        let center_y = output
            .geometry
            .y
            .saturating_add(output.geometry.height)
            .saturating_sub(center_height)
            .saturating_sub(NOTIFICATION_CENTER_CLEARANCE);
        let center_document = decode_document(NOTIFICATION_CENTER_ARTIFACT)
            .map_err(|error| format!("decode notification center UI: {error:?}"))?;
        let center = runtime
            .create_surface(
                UiTemplate::new(center_document),
                SurfaceConfig {
                    width: NOTIFICATION_CENTER_WIDTH,
                    height: NOTIFICATION_CENTER_HEIGHT,
                    title: "FlameWM notification center".to_owned(),
                    role: SurfaceRole::Overlay,
                    input: SurfaceInputMode::Interactive,
                    initially_visible: false,
                    x: center_x,
                    y: center_y,
                },
            )
            .map_err(|error| format!("create notification center surface: {error:?}"))?;
        Ok(Some(Self {
            provider,
            surface,
            visible: None,
            center,
            center_open: false,
            center_view: notification_center::CenterView::default(),
            center_revision: None,
            center_active: Vec::new(),
            closed_by_press: None,
            icons: None,
            icon_root,
            icon_wait: false,
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
        if event.action.phase == flamewm_ui_x11::ActionPhase::Release
            && event.surface == self.center
            && notification_center::outside_press_closes(
                &event.action.action,
                event.action.inside,
                self.center_open,
            )
        {
            // Observed root press: the centre closes and the click still reaches the window under it.
            self.closed_by_press = Some(Instant::now());
            self.toggle_center(runtime);
            return true;
        }
        if event.action.phase != flamewm_ui_x11::ActionPhase::Release || !event.action.inside {
            return false;
        }
        if self.center_open
            && notification_center::is_escape_key(
                &event.action.action,
                event.action.text.as_deref(),
            )
        {
            self.toggle_center(runtime);
            // Escape still reaches the shell's own popups unless the center had focus.
            return event.surface == self.center;
        }
        if event.surface == self.center {
            if let Some(command) = notification_center::Command::parse(&event.action.action) {
                self.apply_center_command(command, runtime);
            }
            return true;
        }
        if event.action.action == notification_center::OPEN_ACTION {
            // The bell release that closed the centre by outside press is the same click.
            if !notification_center::latch_consumes_open(&mut self.closed_by_press, Instant::now())
            {
                self.toggle_center(runtime);
            }
            return true;
        }
        if event.surface != self.surface {
            return false;
        }
        if let Some(notification) = self.visible.clone() {
            // Body click invokes `default` when the client sent one, else dismisses.
            let outcome = match event.action.action.as_str() {
                // Secondary release dismisses the card, like the centre's dismiss control.
                action
                    if notification_center::is_secondary_dismiss(action, event.action.button) =>
                {
                    self.provider.dismiss(notification.id)
                }
                "notification.dismiss" if notification.default_action => self
                    .provider
                    .invoke_action(notification.id, notifications::DEFAULT_ACTION_KEY),
                "notification.dismiss" => self.provider.dismiss(notification.id),
                action => match action
                    .strip_prefix("notification.action.")
                    .and_then(|index| index.parse::<usize>().ok())
                    .and_then(|index| notification.actions.get(index.wrapping_sub(1)))
                {
                    Some(button) => self.provider.invoke_action(notification.id, &button.key),
                    None => return false,
                },
            };
            if let Err(error) = outcome {
                eprintln!(
                    "flamewm-shell: notification action failed: {}",
                    error.message
                );
            }
        } else if event.action.action != "notification.dismiss" {
            return false;
        }
        if let Err(error) = self.tick(runtime, None) {
            eprintln!("flamewm-shell: notification update failed: {error}");
        }
        true
    }

    fn toggle_center(&mut self, runtime: &mut SurfaceRuntime) {
        self.center_open = !self.center_open;
        self.center_revision = None;
        let shown = if self.center_open {
            let shown = runtime.show(self.center);
            // The pointer grab hands the override-redirect centre keyboard focus, so Escape
            // reaches it. A refused grab leaves the centre open without that focus.
            if shown.is_ok() {
                if let Err(error) = runtime.grab_pointer(self.center) {
                    eprintln!("flamewm-shell: notification center focus grab failed: {error:?}");
                }
            }
            shown
        } else {
            // Releases the focus handoff; a centre that never took it has nothing to restore.
            let _ = runtime.ungrab_pointer(self.center);
            runtime.hide(self.center)
        };
        if let Err(error) = shown {
            eprintln!("flamewm-shell: notification center toggle failed: {error:?}");
            self.center_open = false;
            return;
        }
        if let Err(error) = self.refresh_center(runtime) {
            eprintln!("flamewm-shell: notification center update failed: {error}");
        }
    }

    fn apply_center_command(
        &mut self,
        command: notification_center::Command,
        runtime: &mut SurfaceRuntime,
    ) {
        use notification_center::Command;
        match command {
            Command::Clear => self.provider.clear_history(),
            Command::ToggleDnd => {
                let enabled = !self.provider.history().dnd();
                self.provider.set_dnd(enabled);
            }
            Command::ToggleGroupMute(index) => {
                if let Some(group) = self.center_view.groups.get(index).cloned() {
                    if !self.provider.set_app_muted(&group.app_name, !group.muted) {
                        eprintln!("flamewm-shell: notification mute list is full");
                    }
                }
            }
            Command::DismissItem { group, item } => {
                let seq = self
                    .center_view
                    .groups
                    .get(group)
                    .and_then(|group| group.items.get(item))
                    .map(|item| item.seq);
                if let Some(seq) = seq {
                    self.provider.remove_history_entry(seq);
                }
            }
            Command::InvokeItemAction {
                group,
                item,
                action,
            } => {
                let target = self
                    .center_view
                    .groups
                    .get(group)
                    .and_then(|group| group.items.get(item))
                    .and_then(|item| {
                        item.actions
                            .get(action)
                            .map(|action| (item.seq, item.notification_id, action.key.clone()))
                    });
                if let Some((seq, id, key)) = target {
                    match self.provider.invoke_action(id, &key) {
                        // The invoke emits ActionInvoked and closes the toast; the row closes too.
                        Ok(true) => {
                            self.provider.remove_history_entry(seq);
                            if let Err(error) = self.tick(runtime, None) {
                                eprintln!("flamewm-shell: notification update failed: {error}");
                            }
                        }
                        Ok(false) => {
                            eprintln!("flamewm-shell: notification {id} is no longer active");
                        }
                        Err(error) => {
                            eprintln!(
                                "flamewm-shell: notification action failed: {}",
                                error.message
                            );
                        }
                    }
                }
            }
        }
        self.center_revision = None;
        if let Err(error) = self.refresh_center(runtime) {
            eprintln!("flamewm-shell: notification center update failed: {error}");
        }
    }

    /// Re-projects the center only when it is open and history changed since the last projection.
    fn refresh_center(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        if !self.center_open {
            return Ok(());
        }
        let revision = self.provider.history().revision();
        let active: Vec<u32> = self
            .provider
            .active()
            .iter()
            .map(|active| active.id)
            .collect();
        if self.center_revision == Some(revision) && self.center_active == active {
            return Ok(());
        }
        let mut view = notification_center::view(self.provider.history());
        notification_center::hide_inactive_actions(&mut view, |id| active.contains(&id));
        let nodes = notification_center::nodes(&view);
        runtime
            .with_document(self.center, |document| {
                for node in &nodes {
                    match node {
                        notification_center::Node::Text { id, value } => {
                            document.text(id, value.clone())?;
                        }
                        notification_center::Node::Visible { id, shown } => {
                            document.visible(id, *shown)?;
                        }
                    }
                }
                Ok(())
            })
            .map_err(|error| format!("project notification center: {error:?}"))?;
        self.center_view = view;
        self.center_revision = Some(revision);
        self.center_active = active;
        Ok(())
    }

    /// Shows the toast icon for a newly visible notification: `image-data` pixels are already
    /// downscaled; a named icon is requested from the loader and shown when it arrives.
    fn project_icon(
        &mut self,
        runtime: &mut SurfaceRuntime,
        notification: Option<&notifications::NotificationView>,
    ) -> Result<(), String> {
        self.icon_wait = false;
        let Some(notification) = notification else {
            return self.set_icon(runtime, None);
        };
        if let Some(image) = &notification.image {
            let icon = RuntimeImage {
                source: format!("notification-icon-{}", notification.id),
                width: image.width,
                height: image.height,
                pixels: image.rgba.clone(),
            };
            return self.set_icon(runtime, Some(icon));
        }
        let Some(name) = notification.icon_name.as_deref() else {
            return self.set_icon(runtime, None);
        };
        let root = self.icon_root.clone();
        let loader = self
            .icons
            .get_or_insert_with(|| IconLoader::spawn(root, String::new()));
        loader.request(
            IconTarget::TaskSlot(0),
            name,
            notifications::NOTIFICATION_ICON_EDGE,
            notifications::NOTIFICATION_ICON_EDGE,
        );
        self.icon_wait = true;
        self.set_icon(runtime, None)
    }

    /// Applies a finished named icon when it belongs to the visible toast. While a request is
    /// outstanding, the reactor is woken again shortly to collect it.
    fn poll_icon(
        &mut self,
        runtime: &mut SurfaceRuntime,
        wake: Option<&flamewm_reactor::WakeDeadline>,
    ) -> Result<(), String> {
        if !self.icon_wait {
            return Ok(());
        }
        let wanted = self
            .visible
            .as_ref()
            .and_then(|notification| notification.icon_name.clone());
        let Some(loader) = self.icons.as_mut() else {
            self.icon_wait = false;
            return Ok(());
        };
        let mut arrived = None;
        for result in loader.drain_ready() {
            let matches = result.target == IconTarget::TaskSlot(0)
                && Some(&result.name) == wanted.as_ref()
                && result.width == notifications::NOTIFICATION_ICON_EDGE
                && result.height == notifications::NOTIFICATION_ICON_EDGE;
            if matches {
                arrived = Some(result.raster);
            }
        }
        if let Some(raster) = arrived {
            self.icon_wait = false;
            let icon = raster.map(|raster| RuntimeImage {
                source: raster.source,
                width: raster.width,
                height: raster.height,
                pixels: raster.pixels,
            });
            self.set_icon(runtime, icon)
        } else {
            if let Some(wake) = wake {
                let deadline = Instant::now() + Duration::from_millis(15);
                wake.set(Some(wake.get().map_or(deadline, |old| old.min(deadline))));
            }
            Ok(())
        }
    }

    fn set_icon(
        &mut self,
        runtime: &mut SurfaceRuntime,
        icon: Option<RuntimeImage>,
    ) -> Result<(), String> {
        runtime
            .with_document(self.surface, |document| match icon {
                Some(icon) => {
                    document.image_rgba8("notification-icon", icon)?;
                    document.visible("notification-icon", true)
                }
                None => document.visible("notification-icon", false),
            })
            .map_err(|error| format!("project notification icon: {error:?}"))
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
        self.refresh_center(runtime)?;
        let next = self.provider.active().into_iter().last();
        self.poll_icon(runtime, wake)?;
        if self.visible == next {
            if let Some(deadline) = self.provider.next_deadline() {
                if let Some(wake) = wake {
                    wake.set(Some(wake.get().map_or(deadline, |old| old.min(deadline))));
                }
            }
            return Ok(());
        }
        self.visible.clone_from(&next);
        self.project_icon(runtime, next.as_ref())?;
        if let Some(notification) = next {
            runtime
                .with_document(self.surface, |document| {
                    document.text("notification-app", display_text(&notification.app_name, 80))?;
                    document.text(
                        "notification-summary",
                        display_text(&notification.summary, 120),
                    )?;
                    document.text("notification-body", display_text(&notification.body, 500))?;
                    document.visible("notification-actions", !notification.actions.is_empty())?;
                    for index in 0..notifications::MAX_VISIBLE_ACTIONS {
                        let id = format!("notification-action-{}", index + 1);
                        match notification.actions.get(index) {
                            Some(action) => {
                                document.visible(&id, true)?;
                                document.text(&id, display_text(&action.label, 24))?;
                            }
                            None => document.visible(&id, false)?,
                        }
                    }
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
        if let Err(error) = flamewm_shell::quick_controls::run_quick_control_host(
            flamewm_api::paths::workspace_packaged_root(),
        ) {
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
    let root = flamewm_ui_x11::root_geometry().unwrap_or((0, 0));
    let (snapshot, startup_settings, signal_client, bus_offline) = match load_bus_bootstrap() {
        Ok((snapshot, settings, signal_client)) => (snapshot, settings, Some(signal_client), false),
        Err(error) => {
            // Logged once. The retry timer below attaches the bus if it appears later.
            eprintln!(
                "flamewm-shell: session control bus unavailable, running without it (retry every {}s): {error}",
                BUS_RETRY_INTERVAL.as_secs()
            );
            (bus_offline::offline_snapshot(root), None, None, true)
        }
    };
    flamewm_ui_x11::install_default_layout_measure();
    let mut runtime = {
        let scope = flamewm_profiler::ProfilePoint::new("shell.startup.runtime_create");
        let _s = scope.start();
        let mut runtime =
            SurfaceRuntime::new().map_err(|error| format!("create UI runtime: {error:?}"))?;
        // The shell owns the taskbar, Start and volume tray targets for wheel ticks.
        runtime.set_node_wheel_targets(true);
        runtime
    };
    let surfaces = {
        let scope = flamewm_profiler::ProfilePoint::new("shell.startup.surface_create");
        let _s = scope.start();
        ShellSurfaces::create(&mut runtime, &snapshot)?
    };
    let mut start_model = start::state(snapshot.applications.clone());
    start_model.set_favorites(settings_projection::start_favorites(
        startup_settings.as_ref(),
    ));
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
    let loader_root = flamewm_api::paths::workspace_packaged_root();
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
    let notifications =
        match NotificationHost::connect(&mut runtime, &snapshot, loader_root.clone()) {
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
    // StatusNotifier host: shares this reactor's bus source, no thread or timer.
    if bus_offline {
        let attach_state = Rc::clone(&state);
        let attach_handle = reactor.handle();
        let mut attached = false;
        reactor
            .register_timer(BUS_RETRY_INTERVAL, true, move || {
                if attached {
                    return;
                }
                // Failures stay silent: the one-time startup line already explains the degraded state.
                if attach_control_bus(&attach_state, &attach_handle).is_ok() {
                    attached = true;
                    eprintln!("flamewm-shell: session control bus attached");
                }
            })
            .map_err(|error| format!("register control bus retry: {error}"))?;
    }
    let tray_host =
        flamewm_shell::tray_host::TrayHost::connect().map(|host| Rc::new(RefCell::new(host)));
    state.borrow_mut().tray.clone_from(&tray_host);
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
    if let Some(watch) = state.borrow_mut().start_launcher_entry_provider() {
        let launcher_state = Rc::clone(&state);
        reactor
            .register_raw_fd_with_action(watch.fd, signal_interest(watch.events), move |_, _| {
                launcher_state.borrow_mut().launcher_entry_ready();
                flamewm_reactor::FdAction::Continue
            })
            .map_err(|error| format!("register Unity launcher progress source: {error}"))?;
    }
    if let Some(host) = tray_host.as_ref() {
        let watch = host.borrow().watch();
        let tray_state = Rc::clone(host);
        reactor
            .register_raw_fd_with_action(watch.fd, signal_interest(watch.events), move |_, _| {
                match tray_state.borrow_mut().on_ready() {
                    Ok(()) => flamewm_reactor::FdAction::Continue,
                    Err(error) => {
                        eprintln!("flamewm-shell: tray D-Bus source failed: {error}");
                        flamewm_reactor::FdAction::Remove
                    }
                }
            })
            .map_err(|error| format!("register tray source: {error}"))?;
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
    let tray_events = tray_host.clone();
    let tick_state = Rc::clone(&state);
    // Signal subscription into the same Reactor: snapshot signals apply
    // their payload directly in `mark_signal` (zero fetch); legacy
    // revision-only signals are ignored for normal refresh. The tick
    // drains applied domains once per turn.
    // The client was subscribed before the bootstrap load (see `load_bus_bootstrap`).
    if let Some(signal_client) = signal_client {
        register_signal_source(&reactor.handle(), &state, signal_client)?;
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
    // Weather and IP-lookup completions: one wake per finished worker; the tick applies the result.
    {
        let result_fd = state.borrow().result_wake.fd();
        let result_reader = state.borrow().result_wake.reader();
        reactor
            .register_raw_fd_with_action(result_fd, calloop::Interest::READ, move |_, _| {
                flamewm_shell::weather_host::ResultWake::drain(&result_reader);
                flamewm_reactor::FdAction::Continue
            })
            .map_err(|error| format!("register result wake source: {error}"))?;
    }
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
                if tray_events
                    .as_ref()
                    .is_some_and(|host| host.borrow_mut().handle_event(&event, runtime))
                {
                    return Ok(());
                }
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
                tick_state.borrow_mut().tick_tray(runtime);
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
/// Subscribes before the bootstrap load. A signal emitted between a load and a later subscribe is
/// never delivered, so the shell would keep a stale power/system snapshot until the next change.
fn subscribe_then_load<S, T>(
    subscribe: impl FnOnce() -> Result<S, String>,
    load: impl FnOnce() -> Result<T, String>,
) -> Result<(S, T), String> {
    let subscription = subscribe()?;
    Ok((subscription, load()?))
}

/// Control signal subscription first, then the bootstrap snapshot and startup settings.
fn load_bus_bootstrap(
) -> Result<(ShellSnapshot, Option<SettingsSnapshot>, ControlSignalClient), String> {
    let (signal_client, (snapshot, settings)) = subscribe_then_load(
        || {
            ControlSignalClient::connect(BusKind::Session)
                .map_err(|error| format!("subscribe control signals: {error}"))
        },
        load_bootstrap_and_settings,
    )?;
    Ok((snapshot, settings, signal_client))
}

#[cfg(test)]
mod bootstrap_order_tests {
    use super::subscribe_then_load;
    use std::cell::RefCell;

    #[test]
    fn subscribes_before_loading() {
        let log = RefCell::new(Vec::new());
        let result = subscribe_then_load(
            || {
                log.borrow_mut().push("subscribe");
                Ok::<_, String>(7)
            },
            || {
                log.borrow_mut().push("load");
                Ok::<_, String>("snapshot")
            },
        );
        assert_eq!(result, Ok((7, "snapshot")));
        assert_eq!(*log.borrow(), ["subscribe", "load"]);
    }

    #[test]
    fn failed_subscribe_skips_load() {
        let loaded = RefCell::new(false);
        let result: Result<(u8, u8), String> = subscribe_then_load(
            || Err("no bus".to_owned()),
            || {
                *loaded.borrow_mut() = true;
                Ok(1)
            },
        );
        assert_eq!(result, Err("no bus".to_owned()));
        assert!(!*loaded.borrow());
    }
}

/// Control bootstrap and startup settings. Settings are not part of the bootstrap aggregate; a
/// failed settings fetch is nonfatal and leaves defaults in effect.
fn load_bootstrap_and_settings() -> Result<(ShellSnapshot, Option<SettingsSnapshot>), String> {
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
    let settings = match client.call(&flamewm_control_core::ControlRequest::GetSettings) {
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
    Ok((snapshot, settings))
}

/// Late attach for a shell that started without the bus: bootstrap, signal source, then state.
fn attach_control_bus(
    state: &Rc<RefCell<ShellLoop>>,
    reactor: &flamewm_reactor::ReactorHandle,
) -> Result<(), String> {
    let (snapshot, settings, signal_client) = load_bus_bootstrap()?;
    register_signal_source(reactor, state, signal_client)?;
    state.borrow_mut().apply_bus_bootstrap(snapshot, settings);
    Ok(())
}

fn register_signal_source(
    reactor: &flamewm_reactor::ReactorHandle,
    state: &Rc<RefCell<ShellLoop>>,
    client: ControlSignalClient,
) -> Result<(), String> {
    // Leak the client for process lifetime: its pump fd must stay open for the reactor
    // registration, and the reactor never owns the descriptor.
    let client: &'static ControlSignalClient = Box::leak(Box::new(client));
    let watch = client.watch();
    let signal_state = Rc::clone(state);
    reactor
        .register_raw_fd_with_action(watch.fd, signal_interest(watch.events), move |_, _| {
            let _ = client.on_ready(|signal: ControlSignal| {
                signal_state.borrow_mut().note_signal(&signal);
            });
            flamewm_reactor::FdAction::Continue
        })
        .map(|_| ())
        .map_err(|error| format!("register control signal source: {error}"))
}

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
                0,
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

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_reactor::WallClockSource;

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
