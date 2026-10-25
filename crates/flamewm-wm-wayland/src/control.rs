//! `com.arkflame.FlameWM1` control service served by the Wayland compositor.
//!
//! The shell and desktop clients talk to this service exactly as they talk to the X11 WM. The
//! engine (`WaylandDesktop`) answers reads from a mirror of the compositor state, which is refreshed
//! before every D-Bus turn. Writes made during a turn only queue `Command`s; they are applied to the
//! compositor after the turn, so no port implementation borrows `State` while the bus is dispatching.
//!
//! D-Bus I/O is driven by the compositor's calloop: a short timer runs one nonblocking bus turn, and
//! every loop dispatch runs one more. A calloop source on the bus fd needs `FdWrapper::new`, which is
//! `unsafe` and forbidden in this crate; `flamewm-reactor` owns raw fd sources.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::error::Error;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use flamewm_api::applications::ApplicationLaunchOptions;
use flamewm_api::background::BackgroundState;
use flamewm_api::display::{DisplayMode, DisplaySnapshot, OutputSnapshot};
use flamewm_api::input::PointerPosition;
use flamewm_api::ports::{
    ApplicationPort, BackgroundPort, DisplayPort, FdCallback, FdEvents, FdHandle, InputPort,
    MainLoopPort, SessionPort, ShortcutPort, TimerCallback, TimerHandle, TrayPort, WindowPort,
    WorkAreaPort, WorkspacePort,
};
use flamewm_api::session::{SessionAction, SessionCapabilities};
use flamewm_api::shortcuts::KeyBinding;
use flamewm_api::window::{WindowSnapshot, WindowState};
use flamewm_api::workspace::{WorkspacePlan, WorkspaceSnapshot};
use flamewm_api::{
    DesktopAppId, ErrorCode, FlameError, FlameResult, ModeId, OutputId, Rect, Size, TransactionId,
    WindowRef, WorkspaceRef,
};
use flamewm_control_dbus::{ControlServer, ControlSignal};
use flamewm_control_wire::SwitcherEntry;
use flamewm_platform::host::PlatformHost;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::wayland::compositor;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;

use crate::model::{Mode as WindowMode, WindowId};
use crate::state::State;

/// Output name the compositor advertises to the shell; the nested compositor has one output.
const OUTPUT_NAME: &str = "wayland-0";

/// Compositor facts the engine reads. Refreshed by `Service::sync` before each D-Bus turn.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Mirror {
    pub windows: Vec<WindowSnapshot>,
    pub output: Rect,
    pub work_area: Rect,
    pub workspace_count: usize,
    pub active_workspace: usize,
    pub pointer: (i32, i32),
}

/// Writes queued by the engine during a D-Bus turn and applied to `State` afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Activate(WindowId),
    Minimize(WindowId),
    Restore(WindowId),
    Maximize(WindowId),
    Close(WindowId),
    SwitchWorkspace(usize),
    MoveToWorkspace(WindowId, usize),
    /// Starts a program through `State::spawn`, so XWayland clients get the compositor's `DISPLAY`.
    Spawn(Vec<String>),
}

fn unsupported(message: &'static str) -> FlameError {
    FlameError::new(ErrorCode::Unsupported, message)
}

/// The desktop engine behind `PlatformHost`. It holds only shared handles, never `State`.
pub struct WaylandDesktop {
    mirror: Rc<RefCell<Mirror>>,
    commands: Rc<RefCell<Vec<Command>>>,
    catalog: Option<Arc<flamewm_applications::ApplicationCatalog>>,
}

impl WaylandDesktop {
    fn window(&self, window: WindowRef) -> FlameResult<WindowSnapshot> {
        self.mirror
            .borrow()
            .windows
            .iter()
            .find(|candidate| candidate.reference.id == window.id)
            .cloned()
            .ok_or_else(|| FlameError::new(ErrorCode::NotFound, "window is not managed"))
    }

    fn queue(&self, command: Command) -> FlameResult<()> {
        self.commands.borrow_mut().push(command);
        Ok(())
    }

    fn output_id() -> OutputId {
        OutputId::new(OUTPUT_NAME)
    }
}

impl WindowPort for WaylandDesktop {
    fn get(&self, window: WindowRef) -> FlameResult<WindowSnapshot> {
        self.window(window)
    }

    fn snapshot(&self) -> FlameResult<Vec<WindowSnapshot>> {
        Ok(self.mirror.borrow().windows.clone())
    }

    fn activate(&mut self, window: WindowRef) -> FlameResult<()> {
        self.window(window)?;
        self.queue(Command::Activate(window.id))
    }

    fn minimize(&mut self, window: WindowRef) -> FlameResult<()> {
        self.window(window)?;
        self.queue(Command::Minimize(window.id))
    }

    fn maximize(&mut self, window: WindowRef) -> FlameResult<()> {
        self.window(window)?;
        self.queue(Command::Maximize(window.id))
    }

    fn restore(&mut self, window: WindowRef) -> FlameResult<()> {
        self.window(window)?;
        self.queue(Command::Restore(window.id))
    }

    fn close(&mut self, window: WindowRef) -> FlameResult<()> {
        self.window(window)?;
        self.queue(Command::Close(window.id))
    }

    fn set_outer_geometry(&mut self, _window: WindowRef, _geometry: Rect) -> FlameResult<()> {
        Err(unsupported(
            "compositor does not accept client geometry requests from the shell yet",
        ))
    }

    fn work_area(&self, _window: WindowRef) -> FlameResult<Rect> {
        Ok(self.mirror.borrow().work_area)
    }

    fn output(&self, window: WindowRef) -> FlameResult<OutputId> {
        self.window(window).map(|snapshot| snapshot.output)
    }
}

impl WorkspacePort for WaylandDesktop {
    fn workspace_snapshot(&self) -> FlameResult<WorkspaceSnapshot> {
        let mirror = self.mirror.borrow();
        Ok(WorkspaceSnapshot {
            revision: 1,
            count: mirror.workspace_count,
            active_index: mirror.active_workspace,
            last_index: None,
            names: (1..=mirror.workspace_count)
                .map(|index| index.to_string())
                .collect(),
        })
    }

    fn activate_workspace(&mut self, index: usize, _expected_revision: u64) -> FlameResult<()> {
        self.queue(Command::SwitchWorkspace(index))
    }

    fn move_window_to_workspace(&mut self, window: WindowRef, target: usize) -> FlameResult<()> {
        self.window(window)?;
        self.queue(Command::MoveToWorkspace(window.id, target))
    }

    fn switch_workspace_with_window(
        &mut self,
        _window: WindowRef,
        _target: usize,
        _expected_revision: u64,
    ) -> FlameResult<()> {
        Err(unsupported(
            "compositor does not move a window and switch workspace together yet",
        ))
    }

    fn apply_workspace_plan(&mut self, _plan: &WorkspacePlan) -> FlameResult<()> {
        Err(unsupported(
            "compositor cannot add or remove workspaces yet",
        ))
    }
}

impl DisplayPort for WaylandDesktop {
    fn display_snapshot(&self) -> FlameResult<DisplaySnapshot> {
        let mirror = self.mirror.borrow();
        let mode = ModeId(1);
        let size = Size::new(mirror.output.width, mirror.output.height);
        Ok(DisplaySnapshot {
            generation: 1,
            outputs: vec![OutputSnapshot {
                id: Self::output_id(),
                connector: OUTPUT_NAME.to_owned(),
                edid_identity: String::new(),
                connected: true,
                primary: true,
                geometry: mirror.output,
                current_mode: mode,
                modes: vec![DisplayMode {
                    id: mode,
                    resolution: size,
                    refresh_millihz: 60_000,
                    preferred: true,
                }],
                shell_scale_percent: 100,
            }],
            pending: None,
        })
    }

    fn apply_mode(&mut self, _output: &OutputId, _mode: ModeId) -> FlameResult<TransactionId> {
        Err(unsupported(
            "nested compositor output mode is fixed by the host window",
        ))
    }

    fn keep_mode(&mut self, _transaction: TransactionId) -> FlameResult<()> {
        Err(unsupported(
            "nested compositor output mode is fixed by the host window",
        ))
    }

    fn revert_mode(&mut self, _transaction: TransactionId) -> FlameResult<()> {
        Err(unsupported(
            "nested compositor output mode is fixed by the host window",
        ))
    }
}

impl ShortcutPort for WaylandDesktop {
    fn prepare_shortcuts(&mut self, _desired: &BTreeMap<String, KeyBinding>) -> FlameResult<()> {
        Err(unsupported(
            "compositor bindings are compiled in; shell bindings are not applied",
        ))
    }

    fn commit_shortcuts(&mut self) -> FlameResult<()> {
        Err(unsupported(
            "compositor bindings are compiled in; shell bindings are not applied",
        ))
    }

    fn rollback_shortcuts(&mut self) {}
}

impl WorkAreaPort for WaylandDesktop {
    fn base_work_areas(&self) -> FlameResult<Vec<(OutputId, Rect)>> {
        let mirror = self.mirror.borrow();
        Ok(vec![(Self::output_id(), mirror.output)])
    }

    /// Panels reserve space through layer-shell and `_NET_WM_STRUT*`, which the compositor already
    /// folds into the work area. The shell's own reservations are kept so they can be reported back.
    fn apply_flame_reservations(&mut self, _reservations: &[(OutputId, Rect)]) -> FlameResult<()> {
        Ok(())
    }

    fn request_recompute(&mut self) {}
}

impl MainLoopPort for WaylandDesktop {
    fn add_poll(
        &mut self,
        _fd: i32,
        _events: FdEvents,
        _callback: FdCallback,
    ) -> FlameResult<FdHandle> {
        Err(unsupported(
            "extra descriptors are driven by the compositor loop, not the engine",
        ))
    }

    fn remove_poll(&mut self, _handle: FdHandle) {}

    fn add_timer(
        &mut self,
        _delay_ms: u64,
        _callback: TimerCallback,
        _repeat: bool,
    ) -> FlameResult<TimerHandle> {
        Err(unsupported(
            "engine timers are driven by the compositor loop, not the engine",
        ))
    }

    fn remove_timer(&mut self, _handle: TimerHandle) {}

    fn defer(&mut self, _callback: TimerCallback) -> FlameResult<TimerHandle> {
        Err(unsupported(
            "engine deferred callbacks are not available on the Wayland compositor",
        ))
    }
}

impl InputPort for WaylandDesktop {
    fn root_pointer(&self) -> FlameResult<PointerPosition> {
        let mirror = self.mirror.borrow();
        Ok(PointerPosition {
            root: flamewm_api::Point {
                x: mirror.pointer.0,
                y: mirror.pointer.1,
            },
            output: Some(Self::output_id()),
        })
    }
}

impl ApplicationPort for WaylandDesktop {
    fn launch(
        &mut self,
        app: &DesktopAppId,
        options: &ApplicationLaunchOptions,
    ) -> FlameResult<()> {
        let catalog = self.catalog.clone().ok_or_else(|| {
            FlameError::new(
                ErrorCode::Unavailable,
                "application catalog is not injected",
            )
        })?;
        let argv = catalog.find(app)?.argv(options)?;
        self.queue(Command::Spawn(argv))
    }

    fn launch_uri(&mut self, uri: &str) -> FlameResult<()> {
        self.queue(Command::Spawn(vec!["xdg-open".to_owned(), uri.to_owned()]))
    }
}

impl SessionPort for WaylandDesktop {
    fn session_capabilities(&self) -> SessionCapabilities {
        SessionCapabilities {
            lock: std::path::Path::new("/usr/bin/loginctl").exists(),
            logout: false,
            suspend: false,
            reboot: false,
            shutdown: false,
        }
    }

    fn perform_session_action(&mut self, _action: SessionAction) -> FlameResult<()> {
        Err(unsupported(
            "session actions are not wired on the Wayland compositor yet",
        ))
    }
}

impl BackgroundPort for WaylandDesktop {
    fn project_background(&mut self, _state: &BackgroundState) -> FlameResult<()> {
        Err(unsupported(
            "the Wayland compositor does not project wallpapers yet",
        ))
    }

    fn reload_background(&mut self) -> FlameResult<()> {
        Err(unsupported(
            "the Wayland compositor does not project wallpapers yet",
        ))
    }
}

impl TrayPort for WaylandDesktop {
    fn set_tray_owner(&mut self, _output: &OutputId) -> FlameResult<()> {
        Err(unsupported(
            "the Wayland compositor does not implement system tray ownership yet",
        ))
    }

    fn clear_tray_owner(&mut self) -> FlameResult<()> {
        Err(unsupported(
            "the Wayland compositor does not implement system tray ownership yet",
        ))
    }

    fn tray_owner(&self) -> FlameResult<Option<OutputId>> {
        Ok(None)
    }
}

/// Alt+Tab state while the switcher is open: the window order and the highlighted entry.
struct Switcher {
    order: Vec<WindowId>,
    selected: usize,
}

/// Owns the bus connection and the host for the compositor's lifetime.
pub struct Service {
    server: ControlServer,
    host: RefCell<PlatformHost<WaylandDesktop>>,
    mirror: Rc<RefCell<Mirror>>,
    commands: Rc<RefCell<Vec<Command>>>,
    published: RefCell<Vec<WindowSnapshot>>,
    /// Last override-redirect popup given keyboard focus, so focus is taken once per popup.
    popup_focus: RefCell<Option<u32>>,
    switcher: RefCell<Option<Switcher>>,
    started: Instant,
}

fn settings_path() -> PathBuf {
    if let Some(path) = std::env::var_os("FLAMEWM_SETTINGS_PATH").filter(|value| !value.is_empty())
    {
        return PathBuf::from(path);
    }
    if let Some(dir) = std::env::var_os("FLAMEWM_CONFIG_DIR").filter(|value| !value.is_empty()) {
        return PathBuf::from(dir).join("settings.toml");
    }
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("/root/.config"));
    config_home.join("flamewm").join("settings.toml")
}

fn toplevel_metadata(state: &State, id: WindowId) -> (String, String) {
    if let Some(top) = state.toplevel(id) {
        return compositor::with_states(top.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .map(|data| {
                    let data = data
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    (
                        data.title.clone().unwrap_or_default(),
                        data.app_id.clone().unwrap_or_default(),
                    )
                })
                .unwrap_or_default()
        });
    }
    state
        .x11
        .iter()
        .find(|(wid, _)| *wid == id)
        .map(|(_, window)| (window.title(), window.class()))
        .unwrap_or_default()
}

fn to_api_rect(rect: crate::model::Rect) -> Rect {
    Rect {
        x: rect.x,
        y: rect.y,
        width: rect.w,
        height: rect.h,
    }
}

/// Reads the compositor state into the engine's mirror.
fn build_mirror(state: &State) -> Mirror {
    let focused = state.focused;
    let windows = state
        .model
        .managed()
        .iter()
        .map(|window| {
            let (title, app_id) = toplevel_metadata(state, window.id);
            let minimized = window.minimized;
            let window_state = match (minimized, window.mode) {
                (true, _) => WindowState::Minimized,
                (false, WindowMode::Maximized) => WindowState::Maximized,
                (false, WindowMode::Fullscreen) => WindowState::Fullscreen,
                (false, WindowMode::Normal) => WindowState::Normal,
            };
            WindowSnapshot {
                reference: WindowRef {
                    id: window.id,
                    generation: 1,
                },
                title,
                app_id: DesktopAppId::new(app_id),
                outer_geometry: to_api_rect(window.frame()),
                restore_geometry: to_api_rect(window.restore),
                state: window_state,
                sticky: false,
                focused: focused == Some(window.id),
                workspace: WorkspaceRef {
                    index: window.workspace as i32,
                    revision: 1,
                },
                output: WaylandDesktop::output_id(),
                state_generation: 1,
            }
        })
        .collect();
    let pointer = (state.pointer.x as i32, state.pointer.y as i32);
    Mirror {
        windows,
        output: to_api_rect(state.output_rect()),
        work_area: to_api_rect(state.work_area()),
        workspace_count: state.model.workspace_count(),
        active_workspace: state.model.current_workspace(),
        pointer,
    }
}

impl Service {
    /// Connects to the session bus, owns `com.arkflame.FlameWM1` and starts the platform host.
    pub fn connect(state: &State) -> Result<Self, Box<dyn Error>> {
        let server = ControlServer::connect_session().map_err(|error| format!("{error}"))?;
        let mirror = Rc::new(RefCell::new(build_mirror(state)));
        let commands = Rc::new(RefCell::new(Vec::new()));
        let catalog = match flamewm_applications::ApplicationCatalog::discover() {
            Ok(catalog) => Some(Arc::new(catalog)),
            Err(error) => {
                eprintln!("flamewm-wayland: application catalog unavailable: {error}");
                None
            }
        };
        let engine = WaylandDesktop {
            mirror: Rc::clone(&mirror),
            commands: Rc::clone(&commands),
            catalog: catalog.clone(),
        };
        let mut host = PlatformHost::new(engine, settings_path());
        if let Some(catalog) = catalog {
            let _ = host.replace_applications(catalog.all());
        }
        host.start()
            .map_err(|error| format!("platform host start failed: {error}"))?;
        Ok(Self {
            server,
            host: RefCell::new(host),
            mirror,
            commands,
            published: RefCell::new(Vec::new()),
            popup_focus: RefCell::new(None),
            switcher: RefCell::new(None),
            started: Instant::now(),
        })
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Refreshes the mirror from `state` and reports whether it changed.
    fn sync(&self, state: &State) -> bool {
        let next = build_mirror(state);
        let changed = *self.mirror.borrow() != next;
        if changed {
            *self.mirror.borrow_mut() = next;
        }
        changed
    }

    /// Dispatches pending bus traffic, applies the queued commands and keeps signals current.
    /// Runs one bus turn. Returns whether anything happened (a message was dispatched or a command
    /// applied), which drives the idle back-off in `install`.
    pub fn turn(&self, state: &mut State) -> bool {
        self.sync(state);
        let mut busy = false;
        loop {
            let visited = {
                let mut host = self.host.borrow_mut();
                match self.server.on_ready(&mut *host, self.now_ms()) {
                    Ok(visited) => visited,
                    Err(error) => {
                        eprintln!("flamewm-wayland: control dispatch failed: {error}");
                        0
                    }
                }
            };
            if visited == 0 {
                break;
            }
            busy = true;
        }
        let queued: Vec<Command> = self.commands.borrow_mut().drain(..).collect();
        busy |= !queued.is_empty();
        for command in queued {
            apply_command(state, command);
        }
        self.sync(state);
        self.publish_changes();
        busy
    }

    /// Called after every loop dispatch: emits `WindowsSnapshotChanged` when the windows moved.
    pub fn after_dispatch(&self, state: &mut State) {
        if self.sync(state) {
            self.publish_changes();
        }
        self.focus_new_popup(state);
        self.finish_switcher_if_released(state);
    }

    /// Alt+Tab: the first press opens the switcher on the visible windows with the second one
    /// highlighted; each further press advances the highlight. Nothing is focused until Alt is released.
    pub fn switcher_step(&self, state: &mut State) {
        let mut slot = self.switcher.borrow_mut();
        match slot.as_mut() {
            Some(switcher) if !switcher.order.is_empty() => {
                switcher.selected = (switcher.selected + 1) % switcher.order.len();
            }
            Some(_) => {}
            None => {
                let order = state.model.visible_top_first();
                if order.len() < 2 {
                    return;
                }
                *slot = Some(Switcher { order, selected: 1 });
            }
        }
        let view = slot
            .as_ref()
            .map(|switcher| (switcher.order.clone(), switcher.selected));
        drop(slot);
        self.emit_switcher(view);
    }

    /// Alt released: commits the highlighted window and closes the switcher overlay.
    fn finish_switcher_if_released(&self, state: &mut State) {
        if state
            .seat
            .get_keyboard()
            .is_some_and(|keyboard| keyboard.modifier_state().alt)
        {
            return;
        }
        let Some(switcher) = self.switcher.borrow_mut().take() else {
            return;
        };
        self.emit_switcher(None);
        if let Some(id) = switcher.order.get(switcher.selected).copied() {
            apply_command(state, Command::Activate(id));
        }
    }

    fn emit_switcher(&self, view: Option<(Vec<WindowId>, usize)>) {
        let signal = match view {
            Some((order, selected)) => {
                let windows = self.mirror.borrow();
                let entries = order
                    .iter()
                    .map(|id| {
                        windows
                            .windows
                            .iter()
                            .find(|window| window.reference.id == *id)
                            .map(|window| SwitcherEntry {
                                title: window.title.clone(),
                                app_id: window.app_id.as_str().to_owned(),
                            })
                            .unwrap_or(SwitcherEntry {
                                title: String::new(),
                                app_id: String::new(),
                            })
                    })
                    .collect();
                ControlSignal::WindowSwitcherChanged {
                    open: true,
                    immediate: false,
                    selected: u32::try_from(selected).unwrap_or(0),
                    entries,
                }
            }
            None => ControlSignal::WindowSwitcherChanged {
                open: false,
                immediate: false,
                selected: 0,
                entries: Vec::new(),
            },
        };
        if let Err(error) = self.server.emit_signal(&signal) {
            eprintln!("flamewm-wayland: switcher signal failed: {error}");
        }
    }

    /// The shell's launcher and OSD are override-redirect X windows, which the compositor never
    /// focuses. With no managed window focused, the newest popup takes the keyboard once it maps, so
    /// typing reaches it. Docks and the desktop background (strut or below) are never focused.
    fn focus_new_popup(&self, state: &mut State) {
        if state.focused.is_some() {
            return;
        }
        let top = state
            .unmanaged
            .iter()
            .rev()
            .find(|entry| !entry.below && entry.strut.is_none() && entry.surface.is_mapped())
            .and_then(|entry| {
                entry
                    .surface
                    .wl_surface()
                    .map(|surface| (entry.surface.window_id(), surface))
            });
        let Some((window, surface)) = top else { return };
        if self.popup_focus.borrow().is_some_and(|last| last == window) {
            return;
        }
        *self.popup_focus.borrow_mut() = Some(window);
        if let Some(keyboard) = state.seat.get_keyboard() {
            keyboard.set_focus(state, Some(surface), crate::state::next_serial());
        }
    }

    fn publish_changes(&self) {
        let windows = self.mirror.borrow().windows.clone();
        if *self.published.borrow() == windows {
            return;
        }
        let revision = windows
            .iter()
            .map(|window| window.state_generation)
            .max()
            .unwrap_or(0);
        *self.published.borrow_mut() = windows.clone();
        if let Err(error) = self
            .server
            .emit_signal(&ControlSignal::WindowsSnapshotChanged { revision, windows })
        {
            eprintln!("flamewm-wayland: windows signal failed: {error}");
        }
        self.publish_panels();
    }

    /// The taskbar task strip is reconciled from the platform window cache. The X11 runtime refreshes
    /// that cache every loop turn; here it is refreshed only when the windows change, and the panels
    /// snapshot is published when the strip moved (otherwise windows opened later have no task button).
    fn publish_panels(&self) {
        let panels = {
            let mut host = self.host.borrow_mut();
            let before = host.panels_snapshot();
            if let Err(error) = host.refresh_windows() {
                eprintln!("flamewm-wayland: window refresh failed: {error}");
                return;
            }
            let after = host.panels_snapshot();
            (before != after).then_some(after)
        };
        if let Some(snapshot) = panels {
            if let Err(error) = self
                .server
                .emit_signal(&ControlSignal::PanelsSnapshotChanged { snapshot })
            {
                eprintln!("flamewm-wayland: panels signal failed: {error}");
            }
        }
    }
}

/// Bus turn timer: polls every `BUS_TURN_MIN_MS` while busy and backs off to `BUS_TURN_MAX_MS` while
/// idle, so an idle compositor does not wake 100 times a second. Every loop dispatch also runs a
/// turn, so an idle compositor still answers within the back-off bound.
const BUS_TURN_MIN_MS: u64 = 10;
const BUS_TURN_MAX_MS: u64 = 100;

/// Installs the service: connects, schedules the bus turns and stores the service on `State`.
pub fn install(state: &mut State) -> Result<(), Box<dyn Error>> {
    let service = Rc::new(Service::connect(state)?);
    state.control = Some(service);
    let mut interval = BUS_TURN_MIN_MS;
    state.loop_handle.insert_source(
        Timer::from_duration(Duration::from_millis(interval)),
        move |_, _, state| {
            let busy = state
                .control
                .clone()
                .is_some_and(|service| service.turn(state));
            interval = if busy {
                BUS_TURN_MIN_MS
            } else {
                (interval * 2).min(BUS_TURN_MAX_MS)
            };
            TimeoutAction::ToDuration(Duration::from_millis(interval))
        },
    )?;
    Ok(())
}

fn apply_command(state: &mut State, command: Command) {
    match command {
        Command::Activate(id) => {
            if state.model.get(id).is_some_and(|window| window.minimized) {
                state.model.restore(id);
            }
            state.apply_geometry(id);
            state.set_focus(Some(id));
        }
        Command::Minimize(id) => {
            state.model.minimize(id);
            state.apply_geometry(id);
            if state.focused == Some(id) {
                state.refocus();
            }
        }
        Command::Restore(id) => {
            state.model.restore(id);
            state.apply_geometry(id);
            state.set_focus(Some(id));
        }
        Command::Maximize(id) => {
            let work = state.work_area();
            state.model.toggle_maximize(id, work);
            state.apply_geometry(id);
        }
        Command::Close(id) => state.close_window(id),
        Command::Spawn(argv) => {
            if let Some((program, args)) = argv.split_first() {
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                state.spawn(program, &args);
            }
        }
        Command::SwitchWorkspace(index) => {
            if state.model.switch_workspace(index) {
                state.refocus();
            }
        }
        Command::MoveToWorkspace(id, target) => {
            state.model.move_to_workspace(id, target);
            state.apply_geometry(id);
            state.refocus();
        }
    }
    state.request_redraw();
}
