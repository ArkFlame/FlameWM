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
use std::sync::{Arc, Mutex};
use std::time::Instant;

use flamewm_api::applications::ApplicationLaunchOptions;
use flamewm_api::background::BackgroundState;
use flamewm_api::blue_light::resolve_at;
use flamewm_api::display::{DisplayMode, DisplaySnapshot, OutputSnapshot};
use flamewm_api::input::PointerPosition;
use flamewm_api::ports::{
    ApplicationPort, BackgroundPort, DisplayPort, FdCallback, FdEvents, FdHandle, InputPort,
    MainLoopPort, SessionPort, ShortcutPort, TimerCallback, TimerHandle, TrayPort, WindowPort,
    WorkAreaPort, WorkspacePort,
};
use flamewm_api::session::{SessionAction, SessionCapabilities};
use flamewm_api::shortcuts::KeyBinding;
use flamewm_api::system::{SystemAction, SystemSnapshot};
use flamewm_api::window::{WindowSnapshot, WindowState};
use flamewm_api::workspace::{WorkspacePlan, WorkspacePlanKind, WorkspaceSnapshot};
use flamewm_api::{
    DesktopAppId, ErrorCode, FlameError, FlameResult, ModeId, OutputId, Rect, Size, TransactionId,
    WindowRef, WorkspaceRef,
};
use flamewm_control_dbus::{ControlServer, ControlSignal};
use flamewm_control_wire::SwitcherEntry;
use flamewm_integrations_linux::audio_helper::AudioHelper;
use flamewm_integrations_linux::backlight::BacklightProvider;
use flamewm_platform::host::PlatformHost;
use flamewm_platform::system::{MediaAudioKey, MediaBrightnessKey};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::RegistrationToken;
use smithay::reexports::calloop::ping::Ping;
use smithay::reexports::calloop::{Interest, Mode, PostAction};
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
    /// Revision of the workspace topology/active pair, advanced by the control service.
    pub workspace_revision: u64,
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
    /// Starts a program through `State::spawn`, so `XWayland` clients get the compositor's `DISPLAY`.
    Spawn(Vec<String>),
    /// Places a window at an absolute rect, from a shell geometry request.
    SetGeometry(WindowId, crate::model::Rect),
    /// Rebuilds the live shortcut table from the shell's committed bindings.
    SetShortcuts(BTreeMap<String, KeyBinding>),
    /// Carries a window to `target` and activates that workspace in one step.
    SwitchWithWindow {
        id: WindowId,
        target: usize,
    },
    /// Applies a validated workspace insert/remove topology change.
    ApplyWorkspacePlan(flamewm_api::workspace::WorkspacePlan),
    /// Ends the compositor from inside its event loop. The session wrapper supervises it, so its
    /// exit is what returns the VT to the greeter.
    Logout,
}

/// Whether this compositor may end the session itself.
///
/// Logout is only honest when the process chain is the one the session wrapper owns: the wrapper
/// started the compositor with `FLAMEWM_SESSION_LOGOUT_OWNER=1` and told it the wrapper's pid, and
/// the wrapper is still this process's parent. Anything else — a bare start, a reparented process,
/// an inherited variable — means there is no session to return, so the menu entry stays hidden
/// instead of offering an action that would do nothing.
fn logout_supported() -> bool {
    managed_logout(
        std::env::var_os("FLAMEWM_SESSION_LOGOUT_OWNER").as_deref(),
        std::env::var_os("FLAMEWM_SESSION_SUPERVISOR_PID").as_deref(),
        rustix::process::getppid(),
    )
}

/// The wrapper claims ownership and names its own pid; that pid must be our parent.
fn managed_logout(
    owner: Option<&std::ffi::OsStr>,
    supervisor: Option<&std::ffi::OsStr>,
    parent: Option<rustix::process::Pid>,
) -> bool {
    owner == Some(std::ffi::OsStr::new("1"))
        && supervisor
            .and_then(std::ffi::OsStr::to_str)
            .and_then(|pid| pid.parse::<i32>().ok())
            .and_then(rustix::process::Pid::from_raw)
            .is_some_and(|pid| Some(pid) == parent)
}

/// `systemctl`, when installed: suspend, reboot and shutdown go through logind rather than
/// reinventing session policy here.
fn power_program() -> Option<&'static str> {
    ["/usr/bin/systemctl", "/bin/systemctl"]
        .into_iter()
        .find(|program| std::path::Path::new(program).is_file())
}

fn unsupported(message: &'static str) -> FlameError {
    FlameError::new(ErrorCode::Unsupported, message)
}

/// Live media-key volume cap: the shared `audioRaiseMaximum` toggle, off unless explicitly set.
fn volume_cap(snapshot: &flamewm_api::settings::SettingsSnapshot) -> u8 {
    use flamewm_api::settings::SettingValue;
    let raise = matches!(
        snapshot
            .values
            .get(flamewm_platform::system::AUDIO_RAISE_MAXIMUM_KEY),
        Some(SettingValue::Boolean(true))
    );
    flamewm_platform::system::volume_cap_percent(raise)
}

/// Returns the system snapshot to announce when it differs from the last one published.
///
/// The compositor reads every system domain through one snapshot, so this comparison keeps a
/// quiet turn silent while still announcing a changed backlight or audio endpoint.
fn system_change(
    published: &mut Option<SystemSnapshot>,
    snapshot: SystemSnapshot,
) -> Option<SystemSnapshot> {
    if published.as_ref() == Some(&snapshot) {
        return None;
    }
    *published = Some(snapshot.clone());
    Some(snapshot)
}

/// The desktop engine behind `PlatformHost`. It holds only shared handles, never `State`.
pub struct WaylandDesktop {
    mirror: Rc<RefCell<Mirror>>,
    commands: Rc<RefCell<Vec<Command>>>,
    catalog: Option<Arc<flamewm_applications::ApplicationCatalog>>,
    /// Bindings prepared but not yet committed: the shell stages one edit set, then commits it.
    staged_shortcuts: RefCell<Option<BTreeMap<String, KeyBinding>>>,
    /// Per-output shell scale saved by Settings, keyed by this compositor's output id.
    ///
    /// The compositor has no runtime scale hook yet, so this is what `display_snapshot` reports
    /// rather than a hardcoded 100: a scale picked in Settings survives a restart instead of being
    /// erased on the next start.
    shell_scales: BTreeMap<String, u16>,
}

/// The per-output shell scales saved in the `FlameWM` settings file.
///
/// Entries that are not a supported scale are dropped rather than reported, so a corrupt or
/// hand-edited value cannot present a scale the rest of the product would reject.
fn saved_shell_scales() -> BTreeMap<String, u16> {
    shell_scales_at(&settings_path())
}

/// `titlebars` and `windowMargin` from a committed settings snapshot.
///
/// Read the way X11 `live_settings` reads them: a missing or ill-typed value keeps the product
/// default, and a margin outside the supported range falls back rather than being clamped silently.
fn chrome_settings(snapshot: &flamewm_api::settings::SettingsSnapshot) -> (bool, i32) {
    use flamewm_api::settings::SettingValue;
    use flamewm_platform::settings::ProductSettings;

    /// Upper bound `ProductSettings::validate` enforces for `windowMargin`.
    const MAX_WINDOW_MARGIN: i64 = 32;

    let defaults = ProductSettings::default();
    let titlebars = match snapshot.values.get("titlebars") {
        Some(SettingValue::Boolean(value)) => *value,
        _ => defaults.titlebars,
    };
    let window_margin = match snapshot.values.get("windowMargin") {
        Some(SettingValue::Integer(value)) if (0..=MAX_WINDOW_MARGIN).contains(value) => {
            i32::try_from(*value).unwrap_or(i32::from(defaults.window_margin))
        }
        _ => i32::from(defaults.window_margin),
    };
    (titlebars, window_margin)
}

/// Reads per-output scales from `path`. Split from `settings_path` so tests read a real file.
fn shell_scales_at(path: &std::path::Path) -> BTreeMap<String, u16> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| shell_scales_from(&text))
        .unwrap_or_default()
}

/// Parses settings text into per-output scales.
///
/// `parse_snapshot` validates every `scale.*` entry against the same 100..=200 in 5% steps rule
/// Settings enforces, and rejects the whole file when one is out of range. A rejected file
/// therefore yields no scales at all, which is why this needs no filter of its own.
fn shell_scales_from(text: &str) -> BTreeMap<String, u16> {
    flamewm_platform::settings::ConfigStore::default()
        .parse_snapshot(text)
        .map(|settings| settings.per_output_scale)
        .unwrap_or_default()
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

    fn set_outer_geometry(&mut self, window: WindowRef, geometry: Rect) -> FlameResult<()> {
        // Mirror X11: reject an invalid rect before touching the model, with the same message.
        // `is_valid` already requires a positive width and height.
        if !geometry.is_valid() {
            return Err(FlameError::invalid("window geometry is invalid"));
        }
        self.window(window)?;
        self.queue(Command::SetGeometry(
            window.id,
            crate::model::Rect::new(geometry.x, geometry.y, geometry.width, geometry.height),
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
            revision: mirror.workspace_revision,
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
        window: WindowRef,
        target: usize,
        expected_revision: u64,
    ) -> FlameResult<()> {
        // One command, so the window and the active workspace never disagree: both mutations
        // happen in the same turn and nothing is rendered between them.
        self.window(window)?;
        let _ = expected_revision;
        self.queue(Command::SwitchWithWindow {
            id: window.id,
            target,
        })
    }

    fn apply_workspace_plan(&mut self, plan: &WorkspacePlan) -> FlameResult<()> {
        plan.validate()?;
        if matches!(plan.kind, WorkspacePlanKind::Activate) {
            return self.activate_workspace(plan.new_active, plan.expected_revision);
        }
        self.queue(Command::ApplyWorkspacePlan(plan.clone()))
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
                shell_scale_percent: self.shell_scales.get(OUTPUT_NAME).copied().unwrap_or(100),
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
    fn prepare_shortcuts(&mut self, desired: &BTreeMap<String, KeyBinding>) -> FlameResult<()> {
        // Stage only: a prepare that is never committed must leave the live table alone.
        *self.staged_shortcuts.borrow_mut() = Some(desired.clone());
        Ok(())
    }

    fn commit_shortcuts(&mut self) -> FlameResult<()> {
        // The live table is rebuilt from the staged bindings on the next loop turn, because the
        // engine holds `State` and this port does not. Applying takes effect immediately after
        // that turn; nothing is silently dropped.
        let staged = self.staged_shortcuts.borrow_mut().take();
        match staged {
            Some(bindings) => self.queue(Command::SetShortcuts(bindings)),
            None => Err(FlameError::new(
                ErrorCode::EngineRejected,
                "commit_shortcuts without a prepared set",
            )),
        }
    }

    fn rollback_shortcuts(&mut self) {
        *self.staged_shortcuts.borrow_mut() = None;
    }
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
        let power = power_program().is_some();
        SessionCapabilities {
            // The bundled lock screen is an X11 grab, which cannot secure native Wayland clients.
            // Not advertised until a compositor-owned session lock exists.
            lock: false,
            logout: logout_supported(),
            suspend: power,
            reboot: power,
            shutdown: power,
        }
    }

    fn perform_session_action(&mut self, action: SessionAction) -> FlameResult<()> {
        match action {
            SessionAction::Logout if logout_supported() => self.queue(Command::Logout),
            SessionAction::Logout => Err(FlameError::new(
                ErrorCode::Unavailable,
                "logout requires a managed FlameWM session",
            )),
            SessionAction::Suspend | SessionAction::Reboot | SessionAction::Shutdown => {
                let program = power_program().ok_or_else(|| {
                    FlameError::new(ErrorCode::Unavailable, "systemctl is not installed")
                })?;
                let verb = match action {
                    SessionAction::Suspend => "suspend",
                    SessionAction::Reboot => "reboot",
                    _ => "poweroff",
                };
                // Never block the single-threaded compositor while logind authorizes the action:
                // a successful spawn reports dispatch, not completion of the power transition.
                std::process::Command::new(program)
                    .arg(verb)
                    .spawn()
                    .map(|_| ())
                    .map_err(|error| {
                        FlameError::new(
                            ErrorCode::IoFailure,
                            format!("session action launch failed: {error}"),
                        )
                    })
            }
            SessionAction::Lock
            | SessionAction::LockPreview
            | SessionAction::ScreensaverPreview => Err(unsupported(
                "the Wayland compositor has no session lock or screensaver yet",
            )),
        }
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
    /// Last workspace snapshot whose change was announced, so a busy loop cannot spam signals.
    published_workspaces: RefCell<Option<WorkspaceSnapshot>>,
    /// Last system snapshot announced, so a quiet turn cannot spam the OSD signal.
    published_system: RefCell<Option<SystemSnapshot>>,
    /// Audio snapshots queued by the helper's reader thread; drained by its calloop wake source.
    audio_updates: Arc<Mutex<Vec<flamewm_api::system::AudioSnapshot>>>,
    /// Keeps the audio helper's wake eventfd open for the lifetime of the service.
    _audio_ping: Option<Ping>,
    /// Panel backlight owner shared with the system action handler.
    backlight: Rc<RefCell<BacklightProvider>>,
    /// Monotonic workspace revision. X11 advances one per switch; the shell's pager expects the
    /// same discipline, because the activation it sends is revision-fenced.
    workspace_revision: std::cell::Cell<u64>,
    /// Last override-redirect popup given keyboard focus, so focus is taken once per popup.
    popup_focus: RefCell<Option<u32>>,
    switcher: RefCell<Option<Switcher>>,
    started: Instant,
    /// Set whenever the mirror changed, cleared by the publish in `after_dispatch`. A change seen by
    /// `turn` (which builds the mirror too) must still be announced, so the flag outlives `turn`.
    publish_pending: std::cell::Cell<bool>,
    /// Write-readiness source, present only while libdbus has outbound bytes it could not flush.
    write_watch: std::cell::Cell<Option<RegistrationToken>>,
}

pub(crate) fn settings_path() -> PathBuf {
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

/// Minutes after local midnight for the Night Shift schedule.
fn now_minutes() -> u32 {
    use chrono::{Local, Timelike};
    let now = Local::now();
    now.hour() * 60 + now.minute()
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
///
/// `workspace_revision` is passed in (not derived) so the engine can advance it when the topology
/// or the visible workspace moves; deriving it here would make the value depend on the observation
/// point instead of the change.
fn build_mirror(state: &State, workspace_revision: u64) -> Mirror {
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
                (false, WindowMode::Normal | WindowMode::Snapped(_)) => WindowState::Normal,
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
        workspace_revision,
        pointer,
    }
}

impl Service {
    /// Connects to the session bus, owns `com.arkflame.FlameWM1` and starts the platform host.
    pub fn connect(state: &State) -> Result<Self, Box<dyn Error>> {
        let server = ControlServer::connect_session().map_err(|error| format!("{error}"))?;
        let mirror = Rc::new(RefCell::new(build_mirror(state, 1)));
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
            staged_shortcuts: RefCell::new(None),
            shell_scales: saved_shell_scales(),
        };
        // Read the panel backlight before the host starts so the shell bootstrap already carries
        // the current level: its first OSD observation is a baseline, not a card.
        let backlight = Rc::new(RefCell::new(BacklightProvider::connect()));
        let audio_updates = Arc::new(Mutex::new(Vec::new()));
        let audio_link = match smithay::reexports::calloop::ping::make_ping() {
            Ok((ping, source)) => {
                let queue = Arc::clone(&audio_updates);
                let wake = ping.clone();
                match AudioHelper::connect(move |snapshot| {
                    if let Ok(mut pending) = queue.lock() {
                        pending.push(snapshot);
                    }
                    // Wake the compositor's loop; no polling thread and no busy turn.
                    wake.ping();
                }) {
                    Ok(helper) => Some((Rc::new(helper), ping, source)),
                    Err(error) => {
                        eprintln!("flamewm-wayland: audio helper unavailable: {error}");
                        None
                    }
                }
            }
            Err(error) => {
                eprintln!("flamewm-wayland: audio wake source unavailable: {error:?}");
                None
            }
        };
        let mut host = PlatformHost::new(engine, settings_path());
        if let Some(catalog) = catalog {
            let _ = host.replace_applications(catalog.all());
        }
        host.system_mut()
            .update_brightness(backlight.borrow_mut().snapshot());
        if let Some((helper, _, _)) = &audio_link {
            // Best effort: one eager request so the first volume key steps from a real snapshot.
            if let Err(error) = helper.request_snapshot() {
                eprintln!("flamewm-wayland: audio snapshot request failed: {error}");
            }
        }
        let backlight_for_actions = Rc::clone(&backlight);
        let audio_for_actions = audio_link.as_ref().map(|(helper, _, _)| Rc::clone(helper));
        host.set_system_action_handler(Box::new(move |action, _snapshot| match action {
            SystemAction::SetBrightness(action) => backlight_for_actions
                .borrow_mut()
                .set_percent(action.percent, action.generation),
            SystemAction::SetVolume(action) => audio_for_actions.as_ref().map_or_else(
                || {
                    Err(FlameError::new(
                        ErrorCode::Unavailable,
                        "audio helper is unavailable",
                    ))
                },
                |pulse| pulse.set_volume(action),
            ),
            SystemAction::SetMute(action) => audio_for_actions.as_ref().map_or_else(
                || {
                    Err(FlameError::new(
                        ErrorCode::Unavailable,
                        "audio helper is unavailable",
                    ))
                },
                |pulse| pulse.set_mute(action),
            ),
            _ => Err(unsupported(
                "the Wayland compositor does not integrate network or media providers yet",
            )),
        }));
        host.start()
            .map_err(|error| format!("platform host start failed: {error}"))?;
        // Without this the compositor keeps its compiled-in defaults, so a hotkey rebind saved in
        // Settings would be lost on the next start. X11 commits the same set from its runtime.
        match host.commit_startup_shortcuts() {
            Ok(()) => {}
            Err(error) => eprintln!("flamewm-wayland: startup shortcuts were not applied: {error}"),
        }
        // The helper's snapshots wake the loop through this source. Losing it only delays audio
        // updates to the next bus turn; startup never depends on the helper being installed.
        let audio_ping = audio_link.as_ref().map(|(_, ping, _)| ping.clone());
        if let Some((_, _, source)) = audio_link {
            let inserted = state.loop_handle.insert_source(source, |(), _, state| {
                if let Some(service) = state.control.clone() {
                    service.drain_audio();
                }
            });
            if let Err(error) = inserted {
                eprintln!("flamewm-wayland: audio wake source was not installed: {error}");
            }
        }
        Ok(Self {
            server,
            host: RefCell::new(host),
            mirror,
            commands,
            published: RefCell::new(Vec::new()),
            published_workspaces: RefCell::new(None),
            published_system: RefCell::new(None),
            audio_updates,
            _audio_ping: audio_ping,
            backlight,
            workspace_revision: std::cell::Cell::new(1),
            popup_focus: RefCell::new(None),
            switcher: RefCell::new(None),
            started: Instant::now(),
            publish_pending: std::cell::Cell::new(false),
            write_watch: std::cell::Cell::new(None),
        })
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// One panel-brightness media key: align the host fence with the device, step the backlight,
    /// then publish the observed level so the shell OSD shows the new card in the same turn.
    pub fn media_brightness_key(&self, up: bool) {
        let observed = self.backlight.borrow_mut().snapshot();
        self.host
            .borrow_mut()
            .system_mut()
            .update_brightness(observed);
        let key = if up {
            MediaBrightnessKey::Up
        } else {
            MediaBrightnessKey::Down
        };
        let applied = self.host.borrow_mut().media_brightness_key(key);
        if let Err(error) = applied {
            eprintln!("flamewm-wayland: brightness key {key:?} was not applied: {error:?}");
            // The pre-key refresh may still have observed a real level change.
            self.publish_system();
            return;
        }
        // Re-read instead of assuming the write landed: a device that clamps or refuses the step
        // is still reported honestly, and the OSD only moves when the level really changed.
        let observed = self.backlight.borrow_mut().snapshot();
        self.host
            .borrow_mut()
            .system_mut()
            .update_brightness(observed);
        self.publish_system();
    }

    /// One volume media key, capped by the shared "raise maximum volume" setting.
    pub fn media_audio_key(&self, key: MediaAudioKey) {
        let cap = volume_cap(&self.host.borrow().settings_snapshot());
        if let Err(error) = self.host.borrow_mut().media_audio_key(key, cap) {
            eprintln!("flamewm-wayland: audio key {key:?} was not applied: {error:?}");
        }
    }

    /// Applies audio snapshots queued by the helper's reader thread and announces a real change.
    fn drain_audio(&self) {
        let updates = {
            let mut pending = self
                .audio_updates
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut *pending)
        };
        if updates.is_empty() {
            return;
        }
        let changed = {
            let mut host = self.host.borrow_mut();
            let mut changed = false;
            for update in updates {
                changed |= host.system_mut().update_audio(update);
            }
            changed
        };
        if changed {
            self.publish_system();
        }
    }

    /// Emits `SystemSnapshotChanged` when the host's system state moved since the last signal.
    ///
    /// The host borrow is released before the signal is emitted, so a D-Bus callback cannot see a
    /// half-updated host through a re-entrant read.
    fn publish_system(&self) {
        let snapshot = self.host.borrow().system_snapshot();
        let Some(snapshot) = system_change(&mut self.published_system.borrow_mut(), snapshot)
        else {
            return;
        };
        if let Err(error) = self
            .server
            .emit_signal(&ControlSignal::SystemSnapshotChanged {
                snapshot: snapshot.clone(),
            })
        {
            eprintln!("flamewm-wayland: system signal failed: {error}");
        }
        if let Err(error) = self.server.emit_signal(&ControlSignal::SystemChanged {
            revision: snapshot.revision,
        }) {
            eprintln!("flamewm-wayland: system revision signal failed: {error}");
        }
    }

    /// Refreshes the mirror from `state` and reports whether it changed.
    ///
    /// The workspace revision advances here, in the same place the visible workspace and the
    /// topology are observed: a switch, a count change, or a plan that moves windows between
    /// workspaces is an announcement the pager must receive, so the revision is part of the mirror
    /// comparison instead of a value a reader derives on demand.
    fn sync(&self, state: &State) -> bool {
        let previous = self.mirror.borrow().clone();
        let next = build_mirror(state, self.workspace_revision.get());
        let workspace_moved = previous.workspace_count != next.workspace_count
            || previous.active_workspace != next.active_workspace;
        if workspace_moved {
            self.workspace_revision
                .set(self.workspace_revision.get() + 1);
        }
        let next = build_mirror(state, self.workspace_revision.get());
        let changed = *self.mirror.borrow() != next;
        if changed {
            *self.mirror.borrow_mut() = next;
            self.publish_pending.set(true);
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
        // Settings transactions land in the host above, so the model's copies are refreshed here:
        // this is the Wayland equivalent of the X11 settings sync point. It costs one map lookup on
        // an unchanged snapshot.
        self.sync_chrome_settings(state);
        // Helper snapshots normally arrive through their own wake source; draining here too means a
        // missed or unavailable wake source only delays audio by one bus turn.
        self.drain_audio();
        self.sync(state);
        self.publish_changes();
        self.publish_system();
        busy
    }

    /// Pushes `titlebars` and `windowMargin` from the committed settings into the model.
    ///
    /// The values are read the way X11 `live_settings` reads them: a missing or ill-typed value
    /// keeps the product default, so a hand-edited file cannot put the model in an odd state.
    fn sync_chrome_settings(&self, state: &mut State) {
        let snapshot = self.host.borrow().settings_snapshot();
        let (titlebars, window_margin) = chrome_settings(&snapshot);
        if state.model.titlebars() != titlebars || state.model.window_margin() != window_margin {
            state.model.configure(titlebars, window_margin);
            state.request_redraw();
        }
        // The window outline comes from the same shared owner X11 reads, so both backends show one
        // `windowOutlineColor` / `windowOutlineOpacity` / `outlines` ring style.
        let stroke = flamewm_ui_core::surface::window_outline_stroke(&snapshot);
        let outline_changed =
            state
                .decorations
                .borrow_mut()
                .set_outline(stroke.rgb, stroke.alpha, stroke.width);
        if outline_changed {
            state.request_redraw();
        }
        // The workspace topology is a product decision persisted beside the settings file, and X11
        // already honours it. Adopting it here is what makes the pager's cells match the session
        // instead of a compiled-in default.
        if let Some(count) = self.host.borrow().workspace_count() {
            let moved = state.model.set_count(count);
            state.request_redraw();
            if moved {
                state.refocus();
            }
        }
        // Blue light filter: the compositor has no DRM gamma ramp through Smithay, so it paints the
        // shared amber veil instead. One comparison per turn keeps this off the paint path.
        let blue_light = resolve_at(
            flamewm_api::blue_light::BlueLightConfig::from_snapshot(
                &self.host.borrow().settings_snapshot(),
            ),
            now_minutes(),
        )
        .intensity;
        if state.blue_light_intensity != blue_light {
            state.blue_light_intensity = blue_light;
            state.request_redraw();
        }
    }

    /// Called after every loop dispatch: emits `WindowsSnapshotChanged` when the windows moved.
    pub fn after_dispatch(&self, state: &mut State) {
        // Pointer motion repaints without touching the mirror: rebuilding it on every loop turn
        // was per-motion work on the input path. Every path that changes the mirror also requests a
        // redraw (`State::request_full_redraw` sets `mirror_dirty`), so this gate cannot miss a change.
        if state.take_mirror_dirty() {
            self.sync(state);
        }
        if self.publish_pending.replace(false) {
            self.publish_changes();
        }
        self.focus_new_popup(state);
        self.finish_switcher_if_released(state);
        self.sync_write_watch(state);
    }

    /// Arms write-readiness on the bus only while libdbus holds bytes it could not send, so an idle
    /// bus costs no wakeups. Called after every dispatch; a fired source removes itself and the next
    /// call re-arms it if the outbound queue is still not empty.
    fn sync_write_watch(&self, state: &State) {
        let wants = self.server.watch().events.contains(FdEvents::WRITABLE);
        match (wants, self.write_watch.get()) {
            (true, None) => {
                let fd = self.server.watch().fd;
                let source = Generic::new(
                    flamewm_reactor::BorrowedRawFd(fd),
                    Interest::WRITE,
                    Mode::Level,
                );
                match state.loop_handle.insert_source(source, |_, _, state| {
                    if let Some(service) = state.control.clone() {
                        service.write_watch.set(None);
                        service.turn(state);
                    }
                    Ok(PostAction::Remove)
                }) {
                    Ok(token) => self.write_watch.set(Some(token)),
                    Err(error) => eprintln!("flamewm-wayland: bus write watch failed: {error}"),
                }
            }
            (false, Some(token)) => {
                state.loop_handle.remove(token);
                self.write_watch.set(None);
            }
            _ => {}
        }
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
                            .map_or(
                                SwitcherEntry {
                                    title: String::new(),
                                    app_id: String::new(),
                                },
                                |window| SwitcherEntry {
                                    title: window.title.clone(),
                                    app_id: window.app_id.as_str().to_owned(),
                                },
                            )
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
        if *self.published.borrow() != windows {
            let revision = windows
                .iter()
                .map(|window| window.state_generation)
                .max()
                .unwrap_or(0);
            (*self.published.borrow_mut()).clone_from(&windows);
            if let Err(error) = self
                .server
                .emit_signal(&ControlSignal::WindowsSnapshotChanged { revision, windows })
            {
                eprintln!("flamewm-wayland: windows signal failed: {error}");
            }
            self.publish_panels();
        }
        self.publish_workspaces();
    }

    /// Announces the workspace topology/active pair the way the X11 runtime does.
    ///
    /// Without this the pager keeps the snapshot it read at bootstrap: clicking a workspace queues a
    /// switch, the compositor switches, and no client is ever told, so the click looks like a no-op.
    /// `last_index` is `None` because the compositor has no "previous workspace" source of its own.
    fn publish_workspaces(&self) {
        let (revision, count, active_index) = {
            let mirror = self.mirror.borrow();
            (
                mirror.workspace_revision,
                mirror.workspace_count,
                mirror.active_workspace,
            )
        };
        let snapshot = WorkspaceSnapshot {
            revision,
            count,
            active_index,
            last_index: None,
            names: (1..=count).map(|index| index.to_string()).collect(),
        };
        if self.published_workspaces.borrow().as_ref() == Some(&snapshot) {
            return;
        }
        *self.published_workspaces.borrow_mut() = Some(snapshot.clone());
        for signal in [
            ControlSignal::WorkspacesSnapshotChanged {
                snapshot: snapshot.clone(),
            },
            ControlSignal::WorkspacesChanged { revision },
        ] {
            if let Err(error) = self.server.emit_signal(&signal) {
                eprintln!("flamewm-wayland: workspace signal failed: {error}");
            }
        }
        // The task strip shows the active workspace's windows, so it moves with the pager.
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

/// Installs the service: connects and turns it on every bus read wake. Outbound bytes that libdbus
/// could not flush are driven by a write-readiness source armed in `after_dispatch`, so an idle
/// compositor has no bus timer at all.
pub fn install(state: &mut State) -> Result<(), Box<dyn Error>> {
    let service = Rc::new(Service::connect(state)?);
    let bus = flamewm_reactor::BorrowedRawFd(service.server.watch().fd);
    state.control = Some(service);
    state.loop_handle.insert_source(
        Generic::new(bus, Interest::READ, Mode::Level),
        |_, _, state| {
            if let Some(service) = state.control.clone() {
                service.turn(state);
            }
            Ok(PostAction::Continue)
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
            state.refresh_dock_struts();
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
        Command::SetGeometry(id, rect) => {
            state.model.set_rect(id, rect);
            state.apply_geometry(id);
        }
        Command::SetShortcuts(bindings) => {
            state.shortcuts = crate::model::Shortcuts::from_bindings(&bindings);
        }
        Command::ApplyWorkspacePlan(plan) => {
            if state.model.apply_workspace_plan(&plan) {
                state.refocus();
            }
            state.request_redraw();
        }
        Command::SwitchWithWindow { id, target } => {
            // Both halves move in one turn with nothing rendered between them, so the window and
            // the visible workspace never disagree.
            state.model.move_to_workspace(id, target);
            state.model.switch_workspace(target);
            state.apply_geometry(id);
            state.refocus();
        }
        // Stops the loop; the session wrapper reaps the compositor and tears the session down.
        Command::Logout => state.loop_signal.stop(),
    }
    state.request_redraw();
}

#[cfg(test)]
mod tests {
    use super::{
        Command, Mirror, WaylandDesktop, chrome_settings, managed_logout, shell_scales_at,
        shell_scales_from, system_change, volume_cap,
    };
    use flamewm_api::settings::{SettingValue, SettingsSnapshot};
    use flamewm_api::shortcuts::KeyBinding;
    use flamewm_api::system::{BrightnessSnapshot, ServiceAvailability, SystemSnapshot};
    use flamewm_api::window::WindowSnapshot;
    use flamewm_api::{DesktopAppId, OutputId, WindowRef};
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::rc::Rc;

    /// The shell's OSD treats its first observation as a baseline, so the media-key path must only
    /// announce a system snapshot that actually differs from the one already published.
    #[test]
    fn brightness_updates_are_announced_only_when_the_level_moves() {
        let level = |percent: u8| SystemSnapshot {
            brightness: BrightnessSnapshot {
                availability: ServiceAvailability::Available,
                generation: 1,
                percent,
            },
            ..SystemSnapshot::default()
        };
        let mut published = None;
        assert_eq!(
            system_change(&mut published, level(40)),
            Some(level(40)),
            "the first observed level is announced once"
        );
        assert_eq!(
            system_change(&mut published, level(40)),
            None,
            "an unchanged level is not announced again"
        );
        assert_eq!(
            system_change(&mut published, level(45)),
            Some(level(45)),
            "a brightness key step is announced"
        );
    }

    /// The media-key volume cap follows the same `audioRaiseMaximum` key the mixer reads.
    #[test]
    fn raise_maximum_sets_the_media_volume_cap() {
        let mut snapshot = SettingsSnapshot::new(1);
        assert_eq!(volume_cap(&snapshot), 100, "missing key keeps the base cap");
        snapshot.values.insert(
            flamewm_platform::system::AUDIO_RAISE_MAXIMUM_KEY.to_owned(),
            SettingValue::Boolean(true),
        );
        assert_eq!(volume_cap(&snapshot), 150);
        snapshot.values.insert(
            flamewm_platform::system::AUDIO_RAISE_MAXIMUM_KEY.to_owned(),
            SettingValue::Boolean(false),
        );
        assert_eq!(volume_cap(&snapshot), 100);
    }

    /// A saved scale is reported for the output it names.
    #[test]
    fn saved_scales_are_reported_per_output() {
        let scales = shell_scales_from("scale.wayland-0=125\nscale.hdmi-a-1=150\n");
        assert_eq!(scales.get("wayland-0"), Some(&125));
        assert_eq!(scales.get("hdmi-a-1"), Some(&150));
    }

    /// An out-of-range scale invalidates the file, so the compositor falls back to the default
    /// scale instead of reporting a value the rest of the product would reject.
    #[test]
    fn an_invalid_scale_is_not_reported() {
        assert!(shell_scales_from("scale.wayland-0=111\n").is_empty());
        assert!(shell_scales_from("scale.wayland-0=50\n").is_empty());
        assert!(shell_scales_from("scale.wayland-0=500\n").is_empty());
    }

    /// Unreadable settings must degrade to the default scale rather than failing the snapshot.
    #[test]
    fn unparsable_settings_yield_no_saved_scales() {
        assert!(shell_scales_from("").is_empty());
        assert!(shell_scales_from("not = [valid").is_empty());
    }
    /// Logout is offered only when the wrapper owns this process and is still its parent.
    #[test]
    fn logout_requires_the_wrapper_as_exact_parent() {
        use std::ffi::OsStr;
        let parent = rustix::process::Pid::from_raw(1234);
        assert!(managed_logout(
            Some(OsStr::new("1")),
            Some(OsStr::new("1234")),
            parent
        ));
        assert!(
            !managed_logout(Some(OsStr::new("1")), Some(OsStr::new("1235")), parent),
            "a different supervisor pid is not our parent"
        );
        assert!(
            !managed_logout(None, Some(OsStr::new("1234")), parent),
            "the wrapper must claim ownership"
        );
        assert!(
            !managed_logout(Some(OsStr::new("0")), Some(OsStr::new("1234")), parent),
            "any owner value other than 1 is not ownership"
        );
        assert!(
            !managed_logout(Some(OsStr::new("1")), Some(OsStr::new("bad")), parent),
            "an unparsable pid never matches"
        );
        assert!(
            !managed_logout(Some(OsStr::new("1")), Some(OsStr::new("1234")), None),
            "no parent means no owned session"
        );
    }
    /// The scale reader must reach a real file too, and a missing one must not fail the snapshot.
    #[test]
    fn saved_scales_come_from_the_settings_file_on_disk() {
        let dir =
            std::env::temp_dir().join(format!("flamewm-wayland-scale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.toml");
        std::fs::write(&path, "scale.wayland-0=150\n").unwrap();
        assert_eq!(shell_scales_at(&path).get("wayland-0"), Some(&150));
        assert!(shell_scales_at(&dir.join("absent.toml")).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
    /// A shell geometry request must reject the same rects X11 rejects, before it reaches the
    /// model, so a bad request cannot leave a window collapsed to zero size.
    #[test]
    fn geometry_requests_reject_invalid_rects() {
        for width in [0, -1] {
            let rect = flamewm_api::Rect::new(0, 0, width, 100);
            assert!(!rect.is_valid(), "width {width} must be invalid");
        }
        for height in [0, -5] {
            let rect = flamewm_api::Rect::new(0, 0, 100, height);
            assert!(!rect.is_valid(), "height {height} must be invalid");
        }
        // A negative origin is legal: windows may sit off the top-left corner, as in X11.
        assert!(flamewm_api::Rect::new(-4, 9, 640, 480).is_valid());
    }
    /// The staged/commit split must be safe on the real port: an uncommitted prepare changes
    /// nothing, a rollback drops the staged set, and committing with nothing prepared is refused
    /// instead of silently doing nothing.
    #[test]
    fn shortcut_staging_requires_a_commit() {
        use std::rc::Rc;

        use flamewm_api::ports::ShortcutPort;

        fn port() -> WaylandDesktop {
            WaylandDesktop {
                mirror: Rc::new(RefCell::new(Mirror::default())),
                commands: Rc::new(RefCell::new(Vec::new())),
                catalog: None,
                staged_shortcuts: RefCell::new(None),
                shell_scales: BTreeMap::new(),
            }
        }

        let mut desktop = port();
        let mut desired = BTreeMap::new();
        desired.insert("window.close".to_owned(), KeyBinding::assigned("super+q"));

        assert!(
            desktop.commit_shortcuts().is_err(),
            "committing with nothing prepared must be refused"
        );

        desktop.prepare_shortcuts(&desired).unwrap();
        assert!(
            desktop.commands.borrow().is_empty(),
            "prepare alone must not touch the live table"
        );

        desktop.rollback_shortcuts();
        assert!(
            desktop.commit_shortcuts().is_err(),
            "a rolled-back set must not be committable"
        );

        desktop.prepare_shortcuts(&desired).unwrap();
        desktop.commit_shortcuts().unwrap();
        let commands = desktop.commands.borrow();
        assert_eq!(
            commands.as_slice(),
            [Command::SetShortcuts(desired)],
            "commit must queue exactly the prepared bindings"
        );
        assert!(
            desktop.staged_shortcuts.borrow().is_none(),
            "commit must drain the staged set"
        );
    }
    /// A hotkey saved in the settings file must reach the compositor at startup. This drives the
    /// same `effective_registry` + `commit_startup` path the compositor runs, against a real file,
    /// so a rebind survives a restart instead of falling back to compiled-in defaults.
    #[test]
    fn saved_hotkeys_reach_the_compositor_at_startup() {
        use flamewm_platform::services::shortcuts::{ShortcutService, effective_registry};
        use flamewm_platform::settings::ConfigStore;

        let dir =
            std::env::temp_dir().join(format!("flamewm-wayland-hotkeys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.toml");
        std::fs::write(&path, "hotkey.WorkspaceLeft=Ctrl+Super+h\n").unwrap();

        let settings = ConfigStore::default()
            .parse_snapshot(&std::fs::read_to_string(&path).unwrap())
            .unwrap();
        let registry = effective_registry(&settings.hotkeys, settings.assistant_enabled).unwrap();
        assert_eq!(
            registry.bindings().get("WorkspaceLeft").cloned(),
            Some(KeyBinding::assigned("Ctrl+Super+h")),
            "the saved rebound chord must win over the default"
        );

        let mut desktop = WaylandDesktop {
            mirror: Rc::new(RefCell::new(Mirror::default())),
            commands: Rc::new(RefCell::new(Vec::new())),
            catalog: None,
            staged_shortcuts: RefCell::new(None),
            shell_scales: BTreeMap::new(),
        };
        ShortcutService::default()
            .commit_startup(&mut desktop, registry.clone())
            .unwrap();

        let commands = desktop.commands.borrow();
        match commands.as_slice() {
            [Command::SetShortcuts(bindings)] => {
                assert_eq!(
                    bindings.get("WorkspaceLeft"),
                    Some(&KeyBinding::assigned("Ctrl+Super+h")),
                    "the compositor must receive the rebind, not the default"
                );
                assert_eq!(
                    bindings.len(),
                    registry.bindings().len(),
                    "the whole effective set is delivered, not just the rebound action"
                );
            }
            other => panic!("expected one SetShortcuts command, got {other:?}"),
        }
        drop(commands);
        std::fs::remove_dir_all(&dir).ok();
    }
    /// `titlebars` and `windowMargin` are read out of the committed snapshot the way X11 reads its
    /// live copies: a missing value keeps the product default, and an out-of-range margin falls back
    /// instead of being applied.
    #[test]
    fn chrome_settings_follow_the_committed_snapshot() {
        use flamewm_api::settings::{SettingValue, SettingsSnapshot};
        use flamewm_platform::settings::ProductSettings;

        let defaults = ProductSettings::default();
        let mut snapshot = SettingsSnapshot {
            revision: 1,
            values: BTreeMap::new(),
        };
        assert_eq!(
            chrome_settings(&snapshot),
            (defaults.titlebars, i32::from(defaults.window_margin)),
            "an empty snapshot yields the product defaults"
        );

        snapshot
            .values
            .insert("titlebars".to_owned(), SettingValue::Boolean(false));
        snapshot
            .values
            .insert("windowMargin".to_owned(), SettingValue::Integer(8));
        assert_eq!(chrome_settings(&snapshot), (false, 8));

        // `validate` rejects a margin above 32, so a value that slipped through falls back.
        snapshot
            .values
            .insert("windowMargin".to_owned(), SettingValue::Integer(33));
        assert_eq!(
            chrome_settings(&snapshot).1,
            i32::from(defaults.window_margin),
            "an out-of-range margin falls back to the default"
        );
        snapshot
            .values
            .insert("windowMargin".to_owned(), SettingValue::Integer(-1));
        assert_eq!(
            chrome_settings(&snapshot).1,
            i32::from(defaults.window_margin)
        );

        // An ill-typed value must not be trusted either.
        snapshot
            .values
            .insert("titlebars".to_owned(), SettingValue::Text("yes".to_owned()));
        assert_eq!(chrome_settings(&snapshot).0, defaults.titlebars);
    }
    /// Carrying a window to another workspace must move it *and* reveal that workspace, with no
    /// intermediate state: the point of the combined call is that the two halves cannot disagree.
    #[test]
    fn carrying_a_window_switches_to_the_target_workspace() {
        fn port() -> WaylandDesktop {
            WaylandDesktop {
                mirror: Rc::new(RefCell::new(Mirror::default())),
                commands: Rc::new(RefCell::new(Vec::new())),
                catalog: None,
                staged_shortcuts: RefCell::new(None),
                shell_scales: BTreeMap::new(),
            }
        }

        use flamewm_api::ports::WorkspacePort;

        let mut desktop = port();
        // The window must be managed, so put a snapshot in the mirror the port reads.
        desktop.mirror.borrow_mut().windows.push(WindowSnapshot {
            reference: WindowRef {
                id: 1,
                generation: 1,
            },
            title: String::new(),
            app_id: DesktopAppId::new("test"),
            outer_geometry: flamewm_api::Rect::default(),
            restore_geometry: flamewm_api::Rect::default(),
            state: flamewm_api::window::WindowState::Normal,
            sticky: false,
            focused: false,
            workspace: flamewm_api::WorkspaceRef::default(),
            output: OutputId::new("wayland-0"),
            state_generation: 1,
        });
        desktop
            .switch_workspace_with_window(
                WindowRef {
                    id: 1,
                    generation: 1,
                },
                2,
                0,
            )
            .unwrap();
        assert_eq!(
            desktop.commands.borrow().as_slice(),
            [Command::SwitchWithWindow { id: 1, target: 2 }],
            "one command carries both halves"
        );
    }
}
