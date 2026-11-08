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
use flamewm_api::panels::PanelsSnapshot;
use flamewm_api::ports::{
    ApplicationPort, BackgroundPort, DisplayPort, FdCallback, FdEvents, FdHandle, InputPort,
    MainLoopPort, SessionPort, ShortcutPort, TimerCallback, TimerHandle, TrayPort, WindowPort,
    WorkAreaPort, WorkspacePort,
};
use flamewm_api::session::{SessionAction, SessionCapabilities};
use flamewm_api::shortcuts::KeyBinding;
use flamewm_api::system::{AudioSnapshot, ServiceAvailability, SystemAction, SystemSnapshot};
use flamewm_api::window::{WindowSnapshot, WindowState};
use flamewm_api::workspace::{WorkspacePlan, WorkspacePlanKind, WorkspaceSnapshot};
use flamewm_api::{
    DesktopAppId, ErrorCode, FlameError, FlameResult, ModeId, OutputId, Rect, Size, TransactionId,
    WindowRef, WorkspaceRef,
};
use flamewm_control_dbus::{ControlServer, ControlSignal};
use flamewm_control_wire::SwitcherEntry;
use flamewm_integrations_linux::NetworkManagerSlot;
use flamewm_integrations_linux::audio_helper::AudioHelper;
use flamewm_integrations_linux::backlight::BacklightProvider;
use flamewm_platform::host::PlatformHost;
use flamewm_platform::system::{MediaAudioKey, MediaBrightnessKey};
use smithay::reexports::calloop::RegistrationToken;
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::ping::Ping;
use smithay::reexports::calloop::{Interest, Mode, PostAction};
use smithay::wayland::compositor;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;

use crate::model::{Mode as WindowMode, WindowId};
use crate::state::State;

mod apply;
mod media;
mod mirror;
mod ports;
mod publish;
mod service;
mod session;
mod settings;
mod switcher;
#[cfg(test)]
mod tests;
use self::apply::*;
use self::media::*;
use self::mirror::*;
use self::session::*;
pub(crate) use self::settings::nested_output_scale_percent;
use self::settings::*;
use self::switcher::*;

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

impl Mirror {
    /// Managed-window count per workspace, as the shared pager occupancy contract expects.
    #[must_use]
    pub fn workspace_occupancy(&self) -> Vec<usize> {
        flamewm_api::workspace::occupancy_counts(
            self.windows.iter().map(|window| window.workspace.index),
            self.workspace_count,
        )
    }
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
    /// Last panels snapshot announced, so an edge or size change is signalled once.
    published_panels: RefCell<Option<PanelsSnapshot>>,
    /// Audio snapshots queued by the helper's reader thread; drained by its calloop wake source.
    audio_updates: Arc<Mutex<Vec<flamewm_api::system::AudioSnapshot>>>,
    /// Keeps the audio helper's wake eventfd open for the lifetime of the service.
    _audio_ping: Option<Ping>,
    /// Panel backlight owner shared with the system action handler.
    backlight: Rc<RefCell<BacklightProvider>>,
    /// `NetworkManager` system-bus provider; an absent bus publishes `Unavailable`, never devices.
    network: Rc<RefCell<NetworkManagerSlot>>,
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

/// Installs the service: connects and turns it on every bus read wake. Outbound bytes that libdbus
/// could not flush are driven by a write-readiness source armed in `after_dispatch`, so an idle
/// compositor has no bus timer at all.
pub fn install(state: &mut State) -> Result<(), Box<dyn Error>> {
    let service = Rc::new(Service::connect(state)?);
    let bus = flamewm_reactor::BorrowedRawFd(service.server.watch().fd);
    // The NetworkManager fd is fixed for the provider's lifetime (owner changes arrive as bus
    // messages on it), so one level-triggered source is enough.
    let network = flamewm_reactor::BorrowedRawFd(service.network.borrow().watch().fd);
    state.control = Some(service.clone());
    state.loop_handle.insert_source(
        Generic::new(bus, Interest::READ, Mode::Level),
        |_, _, state| {
            if let Some(service) = state.control.clone() {
                service.turn(state);
            }
            Ok(PostAction::Continue)
        },
    )?;
    state.loop_handle.insert_source(
        Generic::new(network, Interest::READ, Mode::Level),
        |_, _, state| {
            if let Some(service) = state.control.clone() {
                service.drain_network();
            }
            Ok(PostAction::Continue)
        },
    )?;
    // Starts the first reconcile, which publishes the initial network state.
    service.drain_network();
    Ok(())
}
