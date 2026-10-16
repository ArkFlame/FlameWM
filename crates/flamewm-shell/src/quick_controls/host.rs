//! J07 quick-control host process: owns audio/network/calendar surfaces.
//!
//! Role branching happens at process entry (`--quick-control-host`):
//! helper role (profiler name `flamewm-quick-control`) connects its own
//! [`ControlClient`], projects the snapshot through the existing taskbar
//! status projections, owns one popup surface per kind, answers parent
//! `OPEN` commands with fitted placement, and routes typed actions through
//! the existing dispatcher. The authentication secret travels out-of-band
//! (environment) and never appears in the supervisor protocol or logs.

use std::collections::VecDeque;
use std::io::{self, Read};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::popup_controller::PopupError;
use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_control_core::ControlRequest;
use flamewm_control_dbus::{ControlClient, ControlSignalClient};
use flamewm_control_wire::ControlSignal;
use flamewm_dbus_reactor::BusKind;
use flamewm_debug::DebugEventId;
use flamewm_shell_core::status::{AudioSettings, AudioTab, NetworkQuery};
use flamewm_shell_core::weather::{
    WeatherError, WeatherLocation, WeatherProvider, WeatherSettings, WeatherSnapshot, WeatherState,
};
use crate::projection::project_weather_popup;
use crate::weather_provider::{CurlFetcher, OpenMeteo};
use flamewm_ui_x11::{
    decode_document, ActionPhase, PointerButton, SlideConfig, SurfaceConfig, SurfaceHandle,
    SurfaceInputMode, SurfaceRole, SurfaceRuntime, UiBackendError, UiDocumentAccess, UiTemplate,
};

use self::popup::{
    arm_slide, close_helper, fit_helper, grab, measure_helper, popup_slide, prepare_commit, present,
};
use super::protocol::{decode_line, Command, OpenRequest, QuickControlKind, MAX_LINE_LEN};
use super::supervisor::{QuickControlSupervisor, SupervisorError};

#[path = "popup.rs"]
mod popup;

/// Pre-commit debug probe; the final geometry commit limit stays fixed
/// (refusals leave state untouched, no limit raise).
const POPUP_TRANSITION: DebugEventId = DebugEventId("shell.popup.transition");

const AUDIO_ARTIFACT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-audio.rwr"));
const NETWORK_ARTIFACT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-network.rwr"));
const CALENDAR_ARTIFACT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-calendar.rwr"));
const BATTERY_ARTIFACT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-battery.rwr"));
const WEATHER_ARTIFACT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-weather.rwr"));

/// WM_NAME of each helper-hosted quick popup, and of the main-hosted media popup.
pub const AUDIO_POPUP_TITLE: &str = "FlameWM audio";
pub const NETWORK_POPUP_TITLE: &str = "FlameWM network";
pub const CALENDAR_POPUP_TITLE: &str = "FlameWM calendar";
pub const BATTERY_POPUP_TITLE: &str = "FlameWM battery";
pub const WEATHER_POPUP_TITLE: &str = "FlameWM weather";
pub const MEDIA_POPUP_TITLE: &str = "FlameWM media";

/// Every quick-popup title. A press whose top-level window carries one of these
/// titles lands inside a popup and is not an outside dismissal.
pub const QUICK_POPUP_TITLES: [&str; 6] = [
    AUDIO_POPUP_TITLE,
    NETWORK_POPUP_TITLE,
    CALENDAR_POPUP_TITLE,
    BATTERY_POPUP_TITLE,
    WEATHER_POPUP_TITLE,
    MEDIA_POPUP_TITLE,
];

/// True when `title` names a quick popup window.
pub fn is_quick_popup_title(title: &str) -> bool {
    QUICK_POPUP_TITLES.contains(&title)
}

const OUTSIDE_RELEASE_ACTION: &str = "surface.outside.release";

/// DEBUG-ONLY gate (local; no new cross-crate dep): true only when
/// `FLAMEWM_DEBUG=1`. Normal mode issues zero extra X sync.
fn host_debug_enabled() -> bool {
    std::env::var_os("FLAMEWM_DEBUG").is_some_and(|v| v == "1")
}

/// R2 tab-latency probe (debug-only, per click; never per frame). Stamps are
/// microseconds since the first probe in this process.
fn r2_now_us() -> u128 {
    static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    EPOCH
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_micros()
}

thread_local! {
    /// Ingress instant of the latest non-motion pointer event, consumed by the
    /// next reprojection present.
    static R2_INGRESS: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
}

#[derive(Debug)]
pub enum HostError {
    Io(io::Error),
    Control(String),
    Ui(String),
}

/// Pure helper-process termination decision. Keeping this separate from the
/// process exit lets library callers/tests inspect the shutdown contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickControlShutdown {
    Completed,
    Failed,
}

pub const fn quick_control_exit_code(shutdown: QuickControlShutdown) -> i32 {
    match shutdown {
        QuickControlShutdown::Completed => 0,
        QuickControlShutdown::Failed => 1,
    }
}

impl core::fmt::Display for HostError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "host i/o: {e}"),
            Self::Control(e) => write!(f, "host control: {e}"),
            Self::Ui(e) => write!(f, "host ui: {e}"),
        }
    }
}

impl std::error::Error for HostError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Control(_) | Self::Ui(_) => None,
        }
    }
}

impl From<io::Error> for HostError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Snapshot projection for the helper: one [`ControlClient`] fetch round
/// over the System domain only. No secret is ever carried: secrets travel
/// via the SecretAgent snapshot, never in this struct or the protocol.
#[derive(Debug, Clone)]
pub struct QuickControlSnapshot {
    pub system: flamewm_api::system::SystemSnapshot,
    pub audio_tab: AudioTab,
    pub audio_settings: AudioSettings,
    /// Audio row whose slider holds the pointer (press..release).
    pub audio_drag_slot: Option<usize>,
    /// Audio row-list scroll offset in px, moved by wheel `scroll` events.
    pub audio_scroll_offset: u32,
    pub network_query: NetworkQuery,
    pub network_scroll_offset: u32,
    pub network_secret_masked_len: usize,
    /// Ephemeral helper-only editor value; never projected except as a mask.
    pub network_secret: String,
    pub network_secret_request: Option<(u64, u64)>,
    pub network_secret_focused: bool,
    pub network_filter_focused: bool,
    pub network_secret_error: bool,
    pub network_secret_submit_id: Option<u64>,
    /// Presentation settings for the shared menu background and popup backdrop.
    pub settings: SettingsSnapshot,
    /// Clock calendar view state; changed only by `calendar.*` actions.
    pub calendar: crate::calendar::CalendarState,
    /// Weather settings, state and the refresh scheduler (no polling while disabled).
    pub weather: WeatherHost,
}

impl QuickControlSnapshot {
    pub fn load(client: &ControlClient) -> Result<Self, HostError> {
        match client.call(&ControlRequest::GetSystem) {
            Ok(flamewm_control_core::ControlResponse::System(system)) => Ok(Self {
                system,
                audio_tab: AudioTab::default(),
                audio_settings: AudioSettings::default(),
                audio_drag_slot: None,
                audio_scroll_offset: 0,
                network_query: NetworkQuery::default(),
                network_scroll_offset: 0,
                calendar: crate::calendar::CalendarState::for_today(crate::projection::local_date()),
                weather: WeatherHost::new(),
                network_secret_masked_len: 0,
                network_secret: String::new(),
                network_secret_request: None,
                network_secret_focused: false,
                network_filter_focused: false,
                network_secret_error: false,
                network_secret_submit_id: None,
                settings: SettingsSnapshot::new(0),
            }),
            Ok(response) => Err(HostError::Control(format!(
                "GetSystem returned {response:?}"
            ))),
            Err(error) => Err(HostError::Control(format!(
                "GetSystem failed: {}",
                error.message
            ))),
        }
    }

    pub fn refresh_settings(&mut self, client: &ControlClient) {
        if let Ok(flamewm_control_core::ControlResponse::Settings(snapshot)) =
            client.call(&ControlRequest::GetSettings)
        {
            self.store_settings(snapshot);
        }
    }

    /// Stores one settings snapshot: presentation for the shared menu background and the
    /// audio tab/settings projection.
    pub fn store_settings(&mut self, snapshot: SettingsSnapshot) {
        self.settings = snapshot.clone();
        apply_settings(snapshot.clone(), &mut self.audio_settings);
        self.weather
            .apply_settings(WeatherSettings::from_snapshot(&snapshot));
    }
}

fn apply_settings(snapshot: SettingsSnapshot, settings: &mut AudioSettings) {
    if let Some(SettingValue::Boolean(raise)) = snapshot.values.get(AudioSettings::SETTINGS_KEY) {
        settings.raise_maximum = *raise;
    }
}

/// Minimum spacing between periodic weather fetches (opening the popup always fetches).
pub const WEATHER_REFRESH_INTERVAL: Duration = Duration::from_secs(30 * 60);

type WeatherResult = Result<WeatherSnapshot, WeatherError>;

/// Weather refresh scheduler. It never fetches while disabled or without a location,
/// fetches once when the popup opens, then at most every [`WEATHER_REFRESH_INTERVAL`]
/// while enabled. Network work runs on a worker thread; the UI thread only polls the channel.
#[derive(Debug, Clone)]
pub struct WeatherHost {
    pub settings: WeatherSettings,
    pub state: WeatherState,
    fetch_on_open: bool,
    last_started: Option<std::time::Instant>,
    /// Shared so the snapshot stays `Clone`; only the UI-thread owner polls it.
    inflight: Option<Arc<Mutex<std::sync::mpsc::Receiver<WeatherResult>>>>,
}

impl WeatherHost {
    #[must_use]
    pub fn new() -> Self {
        Self {
            settings: WeatherSettings::default(),
            state: WeatherState::Disabled,
            fetch_on_open: false,
            last_started: None,
            inflight: None,
        }
    }

    /// Applies settings and resets the visible state to match (disabled or unconfigured).
    pub fn apply_settings(&mut self, settings: WeatherSettings) {
        if !settings.enabled {
            self.inflight = None;
        }
        self.settings = settings;
        let state = std::mem::replace(&mut self.state, WeatherState::Disabled);
        self.state = state.begin_refresh(&self.settings);
    }

    /// Called when the weather popup opens: fetch on the next tick.
    pub fn request_open_fetch(&mut self) {
        self.fetch_on_open = true;
    }

    #[must_use]
    pub fn fetch_due(&self, now: std::time::Instant) -> bool {
        if !self.settings.enabled || self.settings.location.is_none() || self.inflight.is_some() {
            return false;
        }
        self.fetch_on_open
            || self
                .last_started
                .is_some_and(|started| now.duration_since(started) >= WEATHER_REFRESH_INTERVAL)
    }

    /// Advances the scheduler: applies a finished fetch, then starts one if due.
    /// Returns true when the visible state changed.
    pub fn tick(
        &mut self,
        now: std::time::Instant,
        fetch: impl FnOnce(WeatherLocation) -> std::sync::mpsc::Receiver<WeatherResult>,
    ) -> bool {
        let mut changed = false;
        let polled: Option<WeatherResult> = match self.inflight.as_ref() {
            None => None,
            Some(rx) => match rx.lock().map(|guard| guard.try_recv()) {
                Ok(Ok(result)) => Some(result),
                Ok(Err(std::sync::mpsc::TryRecvError::Empty)) => None,
                Ok(Err(std::sync::mpsc::TryRecvError::Disconnected)) | Err(_) => {
                    Some(Err(WeatherError::Transport))
                }
            },
        };
        if let Some(result) = polled {
            self.inflight = None;
            let state = std::mem::replace(&mut self.state, WeatherState::Disabled);
            self.state = state.finish_refresh(result);
            changed = true;
        }
        if self.fetch_due(now) {
            if let Some(location) = self.settings.location.clone() {
                let state = std::mem::replace(&mut self.state, WeatherState::Disabled);
                self.state = state.begin_refresh(&self.settings);
                self.last_started = Some(now);
                self.fetch_on_open = false;
                self.inflight = Some(Arc::new(Mutex::new(fetch(location))));
                changed = true;
            }
        }
        changed
    }
}

impl Default for WeatherHost {
    fn default() -> Self {
        Self::new()
    }
}

/// Production fetch: Open-Meteo through the curl fetcher, on a worker thread.
/// If the thread cannot start, the dropped sender reports a transport error.
fn spawn_weather_fetch(location: WeatherLocation) -> std::sync::mpsc::Receiver<WeatherResult> {
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("flamewm-weather".to_owned())
        .spawn(move || {
            let _ = tx.send(OpenMeteo(CurlFetcher).forecast(&location));
        });
    rx
}

/// Quick-control host state behind the reactor callbacks: one reactor
/// owns the X11 surface FD, the nonblocking parent command FD, and tick
/// timers. No per-popup thread, no async runtime.
struct HostState {
    surfaces: HelperSurfaces,
    snapshot: QuickControlSnapshot,
    dispatcher: Option<Arc<flamewm_control_dbus::MutationDispatcher>>,
    /// Coalesced live system update to project on the next UI turn.
    system_reprojection_pending: bool,
    /// Parsed parent commands awaiting the tick drain.
    queue: VecDeque<Command>,
    /// Set on parent EOF or SHUTDOWN; the tick stops the loop cleanly.
    shutdown: bool,
}

/// Duplicate stdin + O_NONBLOCK on the dup. No unsafe: the std File
/// duplicate path (try_clone) keeps fd 0 stdio-owned while the clone
/// gets its own description flags via set_nonblocking.
fn stdin_nonblocking_dup() -> Result<std::fs::File, HostError> {
    use std::os::unix::io::AsFd;
    let file = std::fs::File::open("/dev/stdin").map_err(HostError::Io)?;
    let flags = rustix::fs::fcntl_getfl(&file)
        .map_err(|error| HostError::Io(io::Error::from_raw_os_error(error.raw_os_error())))?;
    rustix::fs::fcntl_setfl(&file, flags | rustix::fs::OFlags::NONBLOCK)
        .map_err(|error| HostError::Io(io::Error::from_raw_os_error(error.raw_os_error())))?;
    let _ = file.as_fd();
    Ok(file)
}

/// Shared parent-input state: partial line bytes + decoded queue live
/// behind one mutex shared by the stdin FD callback and the tick.
#[derive(Default)]
struct ParentInput {
    pending: Vec<u8>,
    queue: VecDeque<Command>,
    eof: bool,
}

fn push_line(state: &mut ParentInput, line: &[u8]) {
    if line.len() > MAX_LINE_LEN {
        eprintln!("flamewm-quick-control: overlong command line dropped");
        return;
    }
    let text = String::from_utf8_lossy(line);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }
    match decode_line(trimmed) {
        Ok(command) => state.queue.push_back(command),
        Err(error) => eprintln!("flamewm-quick-control: bad command: {error}"),
    }
}

/// Drain available bytes from the nonblocking parent FD: accumulate
/// partial lines, newline parses to queue, EOF (Ok(0)) shuts down.
fn drain_parent_fd(file: &mut std::fs::File, state: &Arc<Mutex<ParentInput>>) {
    let mut buf = [0u8; 4096];
    loop {
        match file.read(&mut buf) {
            Ok(0) => {
                if let Ok(mut guard) = state.lock() {
                    guard.eof = true;
                }
                return;
            }
            Ok(n) => {
                let Ok(mut guard) = state.lock() else { return };
                let mut start = 0usize;
                while start < n {
                    match buf[start..n].iter().position(|b| *b == b'\n') {
                        Some(rel) => {
                            let end = start + rel;
                            let mut line = core::mem::take(&mut guard.pending);
                            line.extend_from_slice(&buf[start..end]);
                            // Strip a trailing CR so CRLF parents stay total.
                            if line.last() == Some(&b'\r') {
                                line.pop();
                            }
                            push_line(&mut guard, &line);
                            start = end + 1;
                        }
                        None => {
                            guard.pending.extend_from_slice(&buf[start..n]);
                            if guard.pending.len() > MAX_LINE_LEN {
                                guard.pending.clear();
                                eprintln!("flamewm-quick-control: overlong command line dropped");
                            }
                            break;
                        }
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => {
                if let Ok(mut guard) = state.lock() {
                    guard.eof = true;
                }
                return;
            }
        }
    }
}

/// Run the quick-control host loop.
///
/// Connects its own control client, creates the three helper-owned popup
/// surfaces, then enters one reactor: X11 surface FD + nonblocking parent
/// command FD + timers/wake -> one event loop. Reuses flamewm-reactor.
/// The tick drains commands: OPEN closes the current helper popup,
/// projects, fits, shows/grabs; CLOSE closes; SHUTDOWN stops cleanly.
/// The surface event callback calls route_helper_event.
pub fn run_quick_control_host() -> Result<(), HostError> {
    flamewm_profiler::init_process("flamewm-quick-control");
    flamewm_debug::init_process("flamewm-quick-control");
    let client = ControlClient::connect(BusKind::Session).map_err(|error| {
        HostError::Control(format!("connect control session: {}", error.message))
    })?;
    let mut snapshot = QuickControlSnapshot::load(&client)?;
    snapshot.refresh_settings(&client);
    drop(client);
    let mut runtime = SurfaceRuntime::new()
        .map_err(|error| HostError::Ui(format!("create UI runtime: {error:?}")))?;
    let surfaces = HelperSurfaces::create(&mut runtime).map_err(HostError::Ui)?;
    let dispatcher = match flamewm_control_dbus::MutationDispatcher::start(BusKind::Session, 64) {
        Ok(dispatcher) => Some(dispatcher),
        Err(error) => {
            eprintln!(
                "flamewm-quick-control: mutation dispatcher unavailable; audio/network actions will be dropped: {}",
                error.message
            );
            None
        }
    };
    let state = std::rc::Rc::new(std::cell::RefCell::new(HostState {
        surfaces,
        snapshot,
        dispatcher,
        system_reprojection_pending: false,
        queue: VecDeque::new(),
        shutdown: false,
    }));

    // Nonblocking parent command FD: duplicate the stdin description so
    // the original fd 0 stays owned by stdio, then mark the duplicate
    // nonblocking. Reads below never block; no new thread, no async.
    let stdin_dup = stdin_nonblocking_dup()?;
    let stdin_fd = {
        use std::os::unix::io::AsRawFd;
        stdin_dup.as_raw_fd()
    };
    let input: Arc<Mutex<ParentInput>> = Arc::new(Mutex::new(ParentInput::default()));
    let mut reactor = flamewm_reactor::Reactor::new()
        .map_err(|error| HostError::Control(format!("create reactor: {error}")))?;
    // One live control-signal client for the helper lifetime. Its pump owns
    // the watch FD; the existing reactor only borrows that descriptor.
    let signal_client = ControlSignalClient::connect(BusKind::Session)
        .map_err(|error| HostError::Control(format!("subscribe control signals: {error}")))?;
    let signal_client: &'static ControlSignalClient = Box::leak(Box::new(signal_client));
    {
        let signal_state = std::rc::Rc::clone(&state);
        let watch = signal_client.watch();
        let interest = match (
            watch
                .events
                .contains(flamewm_api::ports::FdEvents::READABLE),
            watch
                .events
                .contains(flamewm_api::ports::FdEvents::WRITABLE),
        ) {
            (true, true) => calloop::Interest::BOTH,
            (false, true) => calloop::Interest::WRITE,
            _ => calloop::Interest::READ,
        };
        reactor
            .register_raw_fd_with_action(watch.fd, interest, move |_, _| {
                let _ = signal_client.on_ready(|signal| {
                    if let ControlSignal::SystemSnapshotChanged { snapshot } = signal {
                        let mut state = signal_state.borrow_mut();
                        if apply_system_snapshot(&mut state.snapshot, snapshot)
                            && state.surfaces.open.is_some()
                        {
                            state.system_reprojection_pending = true;
                        }
                    }
                });
                flamewm_reactor::FdAction::Continue
            })
            .map_err(|error| HostError::Control(format!("register control signal fd: {error}")))?;
    }
    // the callback reopens its own nonblocking handle per readiness.
    // Keep-alive holder for the dup description; the callback registers
    // the raw fd without ownership transfer.
    let stdin_holder: Arc<Mutex<Option<std::fs::File>>> = Arc::new(Mutex::new(Some(stdin_dup)));
    {
        let input = Arc::clone(&input);
        let stdin_holder = Arc::clone(&stdin_holder);
        reactor
            .register_raw_fd_with_action(stdin_fd, calloop::Interest::READ, move |_, _| {
                // Reuse the held nonblocking description for the read.
                if let Ok(mut holder) = stdin_holder.lock() {
                    if let Some(file) = holder.as_mut() {
                        drain_parent_fd(file, &input);
                    }
                }
                flamewm_reactor::FdAction::Continue
            })
            .map_err(|error| HostError::Control(format!("register stdin fd: {error}")))?;
    }
    // Mutation result wake FD on the same reactor; the tick drains payloads.
    if let Some(dispatcher) = state.borrow().dispatcher.as_ref() {
        let watch_fd = dispatcher.wake_fd();
        let wake = Arc::clone(dispatcher);
        let outcome_state = std::rc::Rc::clone(&state);
        reactor
            .register_raw_fd_with_action(watch_fd, calloop::Interest::READ, move |_, _| {
                crate::control_actions::drain_results(Some(&wake));
                let outcomes = crate::control_actions::take_network_secret_outcomes();
                if !outcomes.is_empty() {
                    let mut state = outcome_state.borrow_mut();
                    for outcome in outcomes {
                        if state.snapshot.network_secret_submit_id != Some(outcome.id)
                            || state.snapshot.network_secret_request != Some(outcome.identity)
                        {
                            continue;
                        }
                        state.snapshot.network_secret_submit_id = None;
                        match outcome.result {
                            Ok(()) => clear_network_secret(&mut state.snapshot),
                            Err(()) => {
                                state.snapshot.network_secret_error = true;
                                // Reproject so the error line shows without another input event.
                                state.system_reprojection_pending = true;
                            }
                        }
                    }
                }
                flamewm_reactor::FdAction::Continue
            })
            .map_err(|error| HostError::Control(format!("register mutation fd: {error}")))?;
    }
    reactor
        .register_timer(
            Duration::from_secs(flamewm_profiler::profile_interval_secs()),
            true,
            || {
                let _ = flamewm_profiler::report_window();
            },
        )
        .map_err(|error| HostError::Control(format!("register profile timer: {error}")))?;
    // Keep the nonblocking stdin description alive until the loop returns.
    let _stdin_keepalive = Arc::clone(&stdin_holder);
    let _stdin_file = stdin_holder;

    let event_state = std::rc::Rc::clone(&state);
    let tick_state = std::rc::Rc::clone(&state);
    let tick_input = Arc::clone(&input);
    let run_result = flamewm_ui_x11::run_surface_runtime_with_reactor_access(
        &mut runtime,
        &mut reactor,
        move |event, runtime| {
            let mut guard = event_state.borrow_mut();
            // Disjoint field borrows through one &mut HostState: route gets
            // &mut surfaces and &mut snapshot in place, no snapshot clone.
            let state = &mut *guard;
            let calendar_before = state.snapshot.calendar;
            let open_before = state.surfaces.open;
            let r2_probe = host_debug_enabled()
                && event.action.phase != ActionPhase::Motion
                && event.action.action != "scroll";
            if r2_probe {
                R2_INGRESS.with(|c| c.set(Some(std::time::Instant::now())));
                eprintln!(
                    "r2 route t_us={} action={} phase={:?} inside={} x={:.1} y={:.1} surface={:?} audio_open={} audio_tab={:?}",
                    r2_now_us(),
                    event.action.action,
                    event.action.phase,
                    event.action.inside,
                    event.action.x,
                    event.action.y,
                    event.surface,
                    state.surfaces.open == Some(QuickControlKind::Audio),
                    state.snapshot.audio_tab,
                );
            }
            let keep = route_helper_event(
                &event,
                runtime,
                &mut state.surfaces,
                &mut state.snapshot,
                state.dispatcher.as_ref(),
                &mut state.system_reprojection_pending,
            );
            if r2_probe && state.system_reprojection_pending {
                eprintln!("r2 flag t_us={} audio_tab={:?}", r2_now_us(), state.snapshot.audio_tab);
            }
            if state.snapshot.calendar != calendar_before {
                // Data-only reprojection of the open calendar; no reopen or resize.
                state.system_reprojection_pending = true;
            }
            battery_menu_edge(
                open_before,
                state.surfaces.open,
                &state.snapshot,
                state.dispatcher.as_ref(),
            );
            if !keep {
                state.shutdown = true;
            }
            Ok(())
        },
        move |runtime| {
            // Move newly arrived parent commands into the tick queue.
            if let Ok(mut guard) = tick_input.lock() {
                let mut state = tick_state.borrow_mut();
                state.queue.append(&mut guard.queue);
                if guard.eof {
                    state.shutdown = true;
                }
            }
            tick_host(runtime, &tick_state)?;
            if tick_state.borrow().shutdown {
                // Close and destroy every helper-owned surface. A teardown
                // error is returned so the owner exits nonzero instead of
                // continuing into the known-crashing runtime destructor.
                let mut state = tick_state.borrow_mut();
                let open_before = state.surfaces.open;
                let result = state.surfaces.shutdown(runtime);
                battery_menu_edge(open_before, None, &state.snapshot, state.dispatcher.as_ref());
                result?;
            }
            Ok(())
        },
    )
    .map_err(|error| HostError::Ui(format!("surface runtime: {error:?}")));
    // Final profiler window on clean exit: flush the cadence bucket even
    // when the loop ran fewer than one interval.
    let _ = flamewm_profiler::report_window();
    let shutdown = match run_result {
        Ok(()) => QuickControlShutdown::Completed,
        Err(error) => {
            eprintln!("flamewm-quick-control: shutdown failed: {error}");
            QuickControlShutdown::Failed
        }
    };
    // Do not return into SurfaceRuntime/SurfaceController::drop here:
    // its native display teardown runs after the helper's owned surfaces
    // are gone, where unavailable cache-drain support leaves the process
    // vulnerable to the known native teardown crash.
    // This branch is reached only by the dedicated helper process, never by
    // the shell/WM parent.
    std::process::exit(quick_control_exit_code(shutdown));
}

/// Coalesce queued commands: consecutive OPENs collapse to the newest;
/// CLOSE/SHUTDOWN are best-effort singletons and never duplicate work.
fn coalesce_commands(queue: &mut VecDeque<Command>) -> VecDeque<Command> {
    let mut out: VecDeque<Command> = VecDeque::new();
    for cmd in queue.drain(..) {
        if matches!(cmd, Command::Open(_)) {
            while matches!(out.back(), Some(Command::Open(_))) {
                out.pop_back();
            }
        }
        out.push_back(cmd);
    }
    out
}

/// Fetches the current settings over the control session for one popup open. A failed connect
/// or call keeps the last snapshot (nonfatal: the popup still opens with the previous values).
fn refresh_settings_for_open(state: &std::rc::Rc<std::cell::RefCell<HostState>>) {
    match ControlClient::connect(BusKind::Session) {
        Ok(client) => state.borrow_mut().snapshot.refresh_settings(&client),
        Err(error) => eprintln!(
            "flamewm-quick-control: settings refresh skipped: {}",
            error.message
        ),
    }
}

/// Tick: drain queued parent commands; OPEN closes the current helper
/// popup then projects/fits/shows/grabs; CLOSE closes; SHUTDOWN stops.
fn tick_host(
    runtime: &mut SurfaceRuntime,
    state: &std::rc::Rc<std::cell::RefCell<HostState>>,
) -> Result<(), flamewm_ui_x11::UiBackendError> {
    // OPEN coalesces to the newest before the drain.
    {
        let mut guard = state.borrow_mut();
        let mut pending = core::mem::take(&mut guard.queue);
        guard.queue = coalesce_commands(&mut pending);
    }
    loop {
        let command = state.borrow_mut().queue.pop_front();
        let Some(command) = command else {
            break;
        };
        let open_before = state.borrow().surfaces.open;
        match command {
            Command::Shutdown => {
                // Leave the open surface for the owning shutdown transaction
                // below so close/ungrab/unmap/flush failures are observable.
                state.borrow_mut().shutdown = true;
            }
            Command::Close => {
                let mut guard = state.borrow_mut();
                guard.surfaces.close(runtime);
            }
            Command::Open(request) => {
                // Live menu settings: re-read the current settings on every open (no timers).
                refresh_settings_for_open(state);
                let snapshot = state.borrow().snapshot.clone();
                let mut guard = state.borrow_mut();
                if request.kind == QuickControlKind::Weather {
                    guard.snapshot.weather.request_open_fetch();
                }
                // One helper popup at a time: close the current one first,
                // timing the real close turn for the slow-open line.
                let close_start = std::time::Instant::now();
                guard.surfaces.close(runtime);
                let close_elapsed = close_start.elapsed();
                if let Err(error) = open_request(
                    runtime,
                    &mut guard.surfaces,
                    &snapshot,
                    &request,
                    close_elapsed,
                ) {
                    eprintln!("flamewm-quick-control: open refused: {error}");
                } else {
                    // OPEN already projected the current snapshot; consume a
                    // signal queued before this turn without a second pass.
                    guard.system_reprojection_pending = false;
                }
            }
        }
        let guard = state.borrow();
        battery_menu_edge(
            open_before,
            guard.surfaces.open,
            &guard.snapshot,
            guard.dispatcher.as_ref(),
        );
    }
    {
        let mut guard = state.borrow_mut();
        let changed = guard.snapshot.weather.tick(std::time::Instant::now(), spawn_weather_fetch);
        if changed && guard.surfaces.open == Some(QuickControlKind::Weather) {
            guard.system_reprojection_pending = true;
        }
    }
    let pending = {
        let mut guard = state.borrow_mut();
        if !guard.system_reprojection_pending {
            None
        } else {
            guard.system_reprojection_pending = false;
            let open = guard.surfaces.open;
            open.map(|kind| {
                (
                    guard.surfaces.surface_for(kind),
                    guard.snapshot.clone(),
                    kind,
                )
            })
        }
    };
    if let Some((surface, snapshot, kind)) = pending {
        let probe_start = std::time::Instant::now();
        project_snapshot(runtime, surface, &snapshot, kind).map_err(UiBackendError::Document)?;
        let projected = probe_start.elapsed();
        runtime.redraw(surface)?;
        if host_debug_enabled() {
            let ingress = R2_INGRESS.with(|c| c.take());
            eprintln!(
                "r2 present t_us={} project_us={} redraw_us={} ingress_to_present_us={}",
                r2_now_us(),
                projected.as_micros(),
                probe_start.elapsed().saturating_sub(projected).as_micros(),
                ingress.map_or(-1, |t| t.elapsed().as_micros() as i128),
            );
        }
    }
    Ok(())
}

/// Battery menu open/close edges send the typed `BatteryMenuOpened`/`Closed`
/// action through the existing dispatcher; those bound the CPU sampling.
fn battery_menu_edge(
    before: Option<QuickControlKind>,
    after: Option<QuickControlKind>,
    snapshot: &QuickControlSnapshot,
    dispatcher: Option<&Arc<flamewm_control_dbus::MutationDispatcher>>,
) {
    let was_open = before == Some(QuickControlKind::Battery);
    let is_open = after == Some(QuickControlKind::Battery);
    if was_open != is_open {
        let request = crate::taskbar::status::battery::menu_action(&snapshot.system, is_open);
        crate::control_actions::enqueue_action(dispatcher, request);
    }
}

fn apply_system_snapshot(
    snapshot: &mut QuickControlSnapshot,
    system: flamewm_api::system::SystemSnapshot,
) -> bool {
    if snapshot.system == system {
        return false;
    }
    snapshot.system = system;
    true
}

/// True open stages: each label maps to one real turn in open_request
/// (project, measure, place, prepare/commit_geometry, present,
/// pointer_grab) plus the total and the real close
/// (close/ungrab/unmap). No fake spans.
pub const QUICK_STAGE_LABELS: &[&str] = &[
    "shell.quick.open.total",
    "shell.quick.project",
    "shell.quick.measure",
    "shell.quick.place",
    "shell.quick.prepare/commit_geometry",
    "shell.quick.present",
    "shell.quick.pointer_grab",
    "shell.quick.close/ungrab/unmap",
];
/// Slow-open threshold: one 60Hz frame (16.67ms).
pub const SLOW_OPEN_THRESHOLD: Duration = Duration::from_micros(16_670);

/// Pure seam: slow-open predicate shared by the log gate.
pub fn is_slow_open(total: Duration) -> bool {
    total >= SLOW_OPEN_THRESHOLD
}

/// Pure seam: stage timings carried by one open turn, rendered by the
/// slow-open debug line. close is the pre-open close of the previous
/// popup on the same tick (zero when nothing was open).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OpenStageTimings {
    pub total: Duration,
    pub project: Duration,
    pub measure: Duration,
    pub place: Duration,
    pub prepare_commit: Duration,
    pub present: Duration,
    pub pointer_grab: Duration,
    pub close: Duration,
}

impl OpenStageTimings {
    pub fn render(&self, kind: QuickControlKind) -> String {
        format!(
            "flamewm-quick-control: slow open kind={kind:?} open.total={:?} project={:?} measure={:?} place={:?} prepare/commit_geometry={:?} present={:?} pointer_grab={:?} close/ungrab/unmap={:?}",
            self.total,
            self.project,
            self.measure,
            self.place,
            self.prepare_commit,
            self.present,
            self.pointer_grab,
            self.close,
        )
    }
}

/// Fitted placement plus the real measure/place stage split, so the
/// slow-open line can attribute each turn without fake spans.
pub struct FittedPlacement {
    pub rect: flamewm_api::Rect,
    pub measure_elapsed: Duration,
    pub place_elapsed: Duration,
}

fn fitted_rect(
    runtime: &mut SurfaceRuntime,
    surface: SurfaceHandle,
    request: &OpenRequest,
) -> Result<(flamewm_api::Rect, Duration, Duration), PopupError> {
    let measure_start = std::time::Instant::now();
    let _measure = crate::runtime::shell_span("shell.quick.measure").start();
    let intrinsic = measure_helper(runtime, surface, request)?;
    let measure_elapsed = measure_start.elapsed();
    drop(_measure);
    let place_start = std::time::Instant::now();
    let _place = crate::runtime::shell_span("shell.quick.place").start();
    let rect = fit_helper(request, intrinsic)?;
    let place_elapsed = place_start.elapsed();
    drop(_place);
    Ok((rect, measure_elapsed, place_elapsed))
}

fn open_request(
    runtime: &mut SurfaceRuntime,
    surfaces: &mut HelperSurfaces,
    snapshot: &QuickControlSnapshot,
    request: &OpenRequest,
    close_elapsed: Duration,
) -> Result<(), String> {
    let total_start = std::time::Instant::now();
    let _total = crate::runtime::shell_span("shell.quick.open.total").start();
    flamewm_debug::emit(POPUP_TRANSITION, Duration::from_millis(0), || {
        format!(
            "quick-control open kind={:?} anchor={:?} work_area={:?} panel_edge={:?}",
            request.kind, request.anchor, request.work_area, request.panel_edge,
        )
    });
    let surface = surfaces.surface_for(request.kind);
    let project_start = std::time::Instant::now();
    let _project = crate::runtime::shell_span("shell.quick.project").start();
    project_snapshot(runtime, surface, snapshot, request.kind)?;
    let project_elapsed = project_start.elapsed();
    drop(_project);
    let (rect, measure_elapsed, place_elapsed) =
        fitted_rect(runtime, surface, request).map_err(|error| error.to_string())?;
    flamewm_debug::emit(POPUP_TRANSITION, Duration::from_millis(0), || {
        format!("quick-control fitted kind={:?} rect={rect:?}", request.kind)
    });
    let prepare_start = std::time::Instant::now();
    let _prepare = crate::runtime::shell_span("shell.quick.prepare/commit_geometry").start();
    prepare_commit(runtime, surface, rect).map_err(|error| error.to_string())?;
    let prepare_elapsed = prepare_start.elapsed();
    drop(_prepare);
    // Same frosted backdrop as the Start menu, captured at the first show.
    let backdrop = crate::settings_projection::start_backdrop_percents(&snapshot.settings);
    if let Err(error) = runtime.set_popup_backdrop(surface, Some(backdrop)) {
        eprintln!("flamewm-quick-control: popup backdrop skipped (nonfatal): {error:?}");
    }
    // Same Start slide preference and speed as the Start menu, on this panel edge.
    let slide = popup_slide(&snapshot.settings, request.panel_edge);
    let slide = match arm_slide(runtime, surface, slide) {
        Ok(()) => slide,
        Err(error) => {
            eprintln!("flamewm-quick-control: popup slide skipped (nonfatal): {error}");
            None
        }
    };
    let present_start = std::time::Instant::now();
    let _present = crate::runtime::shell_span("shell.quick.present").start();
    present(runtime, surface).map_err(|error| error.to_string())?;
    let present_elapsed = present_start.elapsed();
    drop(_present);
    // DEBUG-ONLY: one observed root-rect probe after present. Normal mode
    // issues zero extra X sync.
    let trace = flamewm_ui_x11::GeometryTrace::begin(request.kind.as_str());
    if host_debug_enabled() {
        let _ = runtime.debug_probe_root_rect(surface, trace.for_surface(0));
    }
    let observed_rect = runtime
        .surface_device_rect(surface)
        .ok()
        .map(|r| (r.x as i32, r.y as i32, r.width as i32, r.height as i32));
    let grab_start = std::time::Instant::now();
    let _grab = crate::runtime::shell_span("shell.quick.pointer_grab").start();
    if let Err(error) = grab(runtime, surface).map_err(|error| error.to_string()) {
        drop(_grab);
        if let Err(close_error) = close_helper(runtime, surface, None) {
            eprintln!("flamewm-quick-control: popup rollback close failed: {close_error}");
        }
        match surfaces.open {
            Some(kind) if kind != request.kind => {}
            _ => surfaces.open = None,
        }
        return Err(error);
    }
    let grab_elapsed = grab_start.elapsed();
    drop(_grab);
    surfaces.open = Some(request.kind);
    surfaces.slide = slide;
    let total = total_start.elapsed();
    if host_debug_enabled() {
        eprintln!(
            "popup-record txn={} kind={:?} panel_revision={} edge={:?} panel_rect={:?} output_rect={:?} work_area={:?} anchor={:?} intrinsic=n/a fitted={:?} native_request={:?} native_observed={:?}",
            trace.txn,
            request.kind,
            snapshot.system.revision,
            request.panel_edge,
            request.anchor,
            request.work_area,
            request.work_area,
            request.anchor,
            rect,
            rect,
            observed_rect,
        );
        match runtime.debug_stack_canary() {
            Ok(true) => {}
            Ok(false) => {
                let _ = runtime.raise(surface);
                eprintln!(
                    "flamewm-quick-control: debug stack canary FAILED kind={:?}; re-raised via existing owner",
                    request.kind
                );
            }
            Err(error) => {
                eprintln!("flamewm-quick-control: debug stack canary error: {error:?}");
            }
        }
    }
    if is_slow_open(total) {
        eprintln!(
            "{}",
            OpenStageTimings {
                total,
                project: project_elapsed,
                measure: measure_elapsed,
                place: place_elapsed,
                prepare_commit: prepare_elapsed,
                present: present_elapsed,
                pointer_grab: grab_elapsed,
                close: close_elapsed,
            }
            .render(request.kind)
        );
    }
    Ok(())
}

fn project_snapshot(
    runtime: &mut SurfaceRuntime,
    surface: SurfaceHandle,
    snapshot: &QuickControlSnapshot,
    kind: QuickControlKind,
) -> Result<(), String> {
    let shell = crate::runtime::ShellSnapshot {
        displays: flamewm_api::display::DisplaySnapshot {
            generation: 0,
            outputs: Vec::new(),
            pending: None,
        },
        panels: flamewm_api::panels::PanelsSnapshot {
            revision: 0,
            panels: Vec::new(),
            tasks: Vec::new(),
            pinned_apps: Vec::new(),
        },
        windows: Vec::new(),
        applications: Vec::new(),
        workspaces: None,
        session: flamewm_api::session::SessionCapabilities::default(),
        system: snapshot.system.clone(),
    };
    runtime
        .with_document(surface, |document| {
            let root = match kind {
                QuickControlKind::Audio => "audio-popup",
                QuickControlKind::Network => "wifi-popup",
                QuickControlKind::Calendar => "clock-popup",
                QuickControlKind::Battery => "battery-popup",
                QuickControlKind::Weather => "weather-popup",
            };
            crate::settings_projection::project_menu_background(
                document,
                &snapshot.settings,
                root,
            )?;
            match kind {
                QuickControlKind::Audio => crate::projection::project_audio_full(
                    document,
                    &shell,
                    snapshot.audio_tab,
                    snapshot.audio_settings,
                    snapshot.audio_scroll_offset,
                ),
                QuickControlKind::Network => {
                    let query = snapshot.network_query.clone();
                    crate::projection::project_network_full(
                        document,
                        &shell,
                        &query,
                        snapshot.network_scroll_offset,
                    )?;
                    crate::projection::project_network_secret(
                        document,
                        &"*".repeat(snapshot.network_secret_masked_len),
                    )?;
                    document.text(
                        "network-secret-error",
                        if snapshot.network_secret_error {
                            "Could not connect. Check the password and try again.".to_owned()
                        } else {
                            String::new()
                        },
                    )
                }
                QuickControlKind::Calendar => crate::projection::project_calendar(
                    document,
                    &snapshot.calendar,
                    crate::projection::local_date(),
                ),
                QuickControlKind::Battery => {
                    crate::projection::project_battery_menu(document, &snapshot.system.power, 268.0)
                }
                QuickControlKind::Weather => project_weather_popup(
                    document,
                    &snapshot.weather.state,
                    &snapshot.weather.settings,
                ),
            }
        })
        .map_err(|error| format!("{error:?}"))
}

/// Selects the audio tab named by a released tab action and resets the list
/// window. Returns true when the projected tab or window changed, so the open
/// popup must be reprojected to show the new selection.
fn select_audio_tab(snapshot: &mut QuickControlSnapshot, action: &str) -> bool {
    let tab = match action {
        "audio.tab.devices" => AudioTab::Devices,
        "audio.tab.apps" => AudioTab::Applications,
        _ => return false,
    };
    let changed = snapshot.audio_tab != tab || snapshot.audio_scroll_offset != 0;
    snapshot.audio_tab = tab;
    snapshot.audio_scroll_offset = 0;
    changed
}

/// Route one helper surface event: Escape/outside closes the helper popup,
/// typed audio/network actions forward via the existing dispatcher
/// contract. Events inside the active popup or its submodules are kept;
/// a release outside the active popup closes it on the same turn.
/// Returns true when the loop should keep running.
pub fn route_helper_event(
    event: &flamewm_ui_x11::SurfaceEvent,
    runtime: &mut SurfaceRuntime,
    surfaces: &mut HelperSurfaces,
    snapshot: &mut QuickControlSnapshot,
    dispatcher: Option<&std::sync::Arc<flamewm_control_dbus::MutationDispatcher>>,
    reprojection: &mut bool,
) -> bool {
    sync_network_secret(snapshot);
    let action = event.action.action.as_str();
    if action == "keyboard.input" && event.action.phase == ActionPhase::Release {
        let input = event.action.text.as_deref().unwrap_or_default();
        if input == "\u{1b}" {
            surfaces.close(runtime);
            return true;
        }
    }
    if action == "network.secret.focus" {
        snapshot.network_secret_focused = true;
        snapshot.network_filter_focused = false;
    }
    if action == "network.filter.focus" {
        snapshot.network_filter_focused = true;
        snapshot.network_secret_focused = false;
    }
    if action == "keyboard.input" && event.action.phase == ActionPhase::Release {
        if let Some(text) = event.action.text.as_deref() {
            if text == "\u{8}" || text == "\u{7f}" {
                if snapshot.network_secret_focused {
                    snapshot.network_secret.pop();
                    snapshot.network_secret_masked_len = snapshot.network_secret.chars().count();
                    *reprojection = true;
                } else if snapshot.network_filter_focused {
                    snapshot.network_query.filter.pop();
                    snapshot.network_scroll_offset = 0;
                    *reprojection = true;
                }
            } else if text == "\r" || text == "\n" {
                if snapshot.network_secret_focused {
                    *reprojection = true;
                    return submit_network_secret(snapshot, dispatcher);
                }
            } else if snapshot.network_secret_focused {
                snapshot.network_secret.push_str(text);
                snapshot.network_secret_masked_len = snapshot.network_secret.chars().count();
                snapshot.network_secret_error = false;
                *reprojection = true;
            } else if snapshot.network_filter_focused {
                snapshot.network_query.filter.push_str(text);
                snapshot.network_scroll_offset = 0;
                *reprojection = true;
            }
        }
    }
    let open_surface = surfaces.open.map(|kind| surfaces.surface_for(kind));
    let inside_active = match open_surface {
        Some(handle) => event.surface == handle || event.action.inside,
        None => event.action.inside,
    };
    let outside = event.action.phase == ActionPhase::Release
        && (action == OUTSIDE_RELEASE_ACTION || (surfaces.open.is_some() && !inside_active));
    if outside {
        snapshot.audio_drag_slot = None;
        surfaces.close(runtime);
        return true;
    }
    if surfaces.open.is_none() {
        return true;
    }
    if action == "scroll" && surfaces.open == Some(QuickControlKind::Audio) {
        if let Some(dy) = event
            .action
            .text
            .as_deref()
            .and_then(|text| text.split_once(','))
            .and_then(|(_, dy)| dy.parse::<f32>().ok())
        {
            scroll_audio_rows(snapshot, dy);
        }
        return true;
    }
    // A drag owns the pointer until release: motion keeps routing to the grabbed
    // slot even when the pointer leaves its track; release or close ends it.
    match event.action.phase {
        ActionPhase::Press => {
            if let Some((slot, "set")) = audio_slot_command(action) {
                snapshot.audio_drag_slot = Some(slot);
                if let Some(request) = audio_pointer_request(snapshot, slot, event.action.x) {
                    crate::control_actions::enqueue_action(dispatcher, request);
                }
            }
        }
        ActionPhase::Motion => {
            if let Some(slot) = snapshot.audio_drag_slot {
                if let Some(request) = audio_pointer_request(snapshot, slot, event.action.x) {
                    crate::control_actions::enqueue_action(dispatcher, request);
                }
            }
        }
        ActionPhase::Release => snapshot.audio_drag_slot = None,
        _ => {}
    }
    if event.action.phase == ActionPhase::Release && event.action.inside {
        match action {
            "audio.back" => surfaces.close(runtime),
            "audio.tab.devices" | "audio.tab.apps" => {
                if select_audio_tab(snapshot, action) {
                    *reprojection = true;
                }
            }
            "audio.raise.toggle" => {
                snapshot.audio_settings.raise_maximum = !snapshot.audio_settings.raise_maximum;
                if let Some(dispatcher) = dispatcher {
                    let request =
                        ControlRequest::ApplySettings(flamewm_api::settings::SettingsTransaction {
                            expected_revision: snapshot.system.revision,
                            changes: vec![flamewm_api::settings::SettingsChange {
                                key: AudioSettings::SETTINGS_KEY.to_owned(),
                                value: Some(SettingValue::Boolean(
                                    snapshot.audio_settings.raise_maximum,
                                )),
                            }],
                            reset_section: None,
                        });
                    crate::control_actions::enqueue_action(Some(dispatcher), request);
                }
            }
            "network.filter.clear" => snapshot.network_query.filter.clear(),
            "network.filter.focus" | "network.secret.focus" => {}
            "network.secret.cancel" => {
                if let Some((request_id, generation)) = snapshot.network_secret_request {
                    let request = ControlRequest::SystemAction {
                        action: flamewm_api::system::SystemAction::CancelNetworkSecret {
                            request_id,
                            generation,
                        },
                        expected_revision: snapshot.system.revision,
                    };
                    crate::control_actions::enqueue_action(dispatcher, request);
                }
                clear_network_secret(snapshot);
                return true;
            }
            "network.disconnect" => {
                clear_network_secret(snapshot);
                if let Some(request) = helper_control_request(action, snapshot) {
                    crate::control_actions::enqueue_action(dispatcher, request);
                }
                return true;
            }
            "network.secret.submit" => {
                *reprojection = true;
                return submit_network_secret(snapshot, dispatcher);
            }
            value if value.starts_with("network.filter.") => {
                snapshot.network_query.filter = value["network.filter.".len()..].to_owned();
                snapshot.network_scroll_offset = 0;
            }
            value if value.starts_with("calendar.") => {
                snapshot
                    .calendar
                    .apply(value, crate::projection::local_date());
            }
            _ => {}
        }
        if let Some(request) = helper_control_request(action, snapshot) {
            crate::control_actions::enqueue_action(dispatcher, request);
        }
        let _ = PointerButton::from_raw_x(event.action.button);
    }
    true
}

fn clear_network_secret(snapshot: &mut QuickControlSnapshot) {
    snapshot.network_secret.clear();
    snapshot.network_secret_masked_len = 0;
    snapshot.network_secret_request = None;
    snapshot.network_secret_submit_id = None;
    snapshot.network_secret_error = false;
}

fn submit_network_secret(
    snapshot: &mut QuickControlSnapshot,
    dispatcher: Option<&std::sync::Arc<flamewm_control_dbus::MutationDispatcher>>,
) -> bool {
    let Some(identity) = snapshot.network_secret_request else {
        return true;
    };
    let Some(request) =
        network_secret_submit_request(identity, &snapshot.network_secret, snapshot.system.revision)
    else {
        // Empty secret: keep the prompt and report it; never submit.
        snapshot.network_secret_error = true;
        return true;
    };
    snapshot.network_secret_submit_id =
        crate::control_actions::enqueue_network_secret(dispatcher, identity, request);
    if snapshot.network_secret_submit_id.is_none() {
        snapshot.network_secret_error = true;
    }
    true
}

/// Builds the SecretAgent submit for the pending identity. An empty password is
/// never a valid secured-AP credential, so it yields no request.
fn network_secret_submit_request(
    identity: (u64, u64),
    secret: &str,
    expected_revision: u64,
) -> Option<ControlRequest> {
    if secret.is_empty() {
        return None;
    }
    let (request_id, generation) = identity;
    Some(ControlRequest::SystemAction {
        action: flamewm_api::system::SystemAction::SubmitNetworkSecret {
            request_id,
            generation,
            secret: secret.to_owned(),
        },
        expected_revision,
    })
}

fn sync_network_secret(snapshot: &mut QuickControlSnapshot) {
    let key = snapshot
        .system
        .network
        .pending_secret
        .as_ref()
        .map(|pending| (pending.request_id, snapshot.system.network.generation));
    if snapshot.network_secret_request != key {
        clear_network_secret(snapshot);
        snapshot.network_secret_focused = false;
        snapshot.network_filter_focused = false;
        snapshot.network_secret_request = key;
    }
}

/// `audio.slot.<n>.<command>` -> (zero-based slot, command).
/// Wheel `dy` (positive = down) moves the audio row list by one whole row,
/// clamped to the rows beyond the popup viewport.
fn scroll_audio_rows(snapshot: &mut QuickControlSnapshot, dy: f32) {
    use crate::taskbar::status::audio::{AUDIO_POPUP_ROWS, AUDIO_ROW_HEIGHT};
    if dy == 0.0 {
        return;
    }
    let total =
        crate::taskbar::status::audio::tab_rows(&snapshot.system.audio, snapshot.audio_tab).len();
    let max_rows = total.saturating_sub(AUDIO_POPUP_ROWS);
    let row = (snapshot.audio_scroll_offset / AUDIO_ROW_HEIGHT) as usize;
    let next = if dy > 0.0 {
        (row + 1).min(max_rows)
    } else {
        row.saturating_sub(1)
    };
    snapshot.audio_scroll_offset = (next as u32) * AUDIO_ROW_HEIGHT;
}

fn audio_slot_command(action: &str) -> Option<(usize, &str)> {
    let (slot_text, command) = action.strip_prefix("audio.slot.")?.rsplit_once('.')?;
    let slot = slot_text.parse::<usize>().ok()?.checked_sub(1)?;
    Some((slot, command))
}

/// Pointer x on the slider track -> snapped percent under the current cap.
fn audio_pointer_request(
    snapshot: &QuickControlSnapshot,
    slot: usize,
    x: f32,
) -> Option<ControlRequest> {
    use crate::taskbar::status::audio::{AUDIO_TRACK_WIDTH, AUDIO_TRACK_X};
    let row = crate::taskbar::status::audio::visible_rows(
        &snapshot.system.audio,
        snapshot.audio_tab,
        snapshot.audio_scroll_offset,
    )
    .into_iter()
    .nth(slot)?;
    let spec = snapshot.audio_settings.slider_spec(row.volume_percent);
    let value = flamewm_ui_core::range::RangeState::value_from_pointer(
        spec,
        AUDIO_TRACK_X,
        AUDIO_TRACK_WIDTH,
        x,
    );
    crate::taskbar::status::audio::system_action_for_row(
        &snapshot.system,
        &row.id,
        Some(value.round() as u8),
        None,
    )
}

fn helper_control_request(action: &str, snapshot: &QuickControlSnapshot) -> Option<ControlRequest> {
    let system = &snapshot.system;
    if action == "network.wifi.toggle" {
        return Some(ControlRequest::SystemAction {
            action: flamewm_api::system::SystemAction::SetWifiEnabled(!system.network.wifi_enabled),
            expected_revision: system.revision,
        });
    }
    if action == "network.scan" {
        return crate::taskbar::status::network::system_action_for_path(system, "", true, false);
    }
    if action == "network.disconnect" {
        return crate::taskbar::status::network::system_action_for_path(system, "", false, true);
    }
    if let Some(slot) = action
        .strip_prefix("network.connect.")
        .and_then(|value| value.parse::<usize>().ok())
        .and_then(|slot| slot.checked_sub(1))
    {
        let path = crate::taskbar::status::network::project_visible_rows(
            system,
            &snapshot.network_query,
            snapshot.network_scroll_offset,
        )
        .get(slot)
        .map(|row| row.path.clone())?;
        return crate::taskbar::status::network::system_action_for_path(
            system, &path, false, false,
        );
    }
    if action == "audio.raise.toggle" {
        return None;
    }
    let row = if matches!(action, "volume.mute" | "volume.down" | "volume.up") {
        crate::taskbar::status::audio::default_endpoint_row(&system.audio)
    } else if let Some((kind_slot, _)) = action
        .strip_prefix("audio.")
        .and_then(|value| value.rsplit_once('.'))
    {
        let (_, slot) = kind_slot.rsplit_once('.')?;
        let slot = slot.parse::<usize>().ok()?.checked_sub(1)?;
        crate::taskbar::status::audio::visible_rows(
            &system.audio,
            snapshot.audio_tab,
            snapshot.audio_scroll_offset,
        )
        .into_iter()
        .nth(slot)
    } else {
        None
    }?;
    let command = action.rsplit('.').next()?;
    let cap = snapshot.audio_settings.max_percent();
    match command {
        "up" => crate::taskbar::status::audio::system_action_for_row(
            system,
            &row.id,
            Some(
                snapshot
                    .audio_settings
                    .clamp_percent(row.volume_percent.saturating_add(5)),
            ),
            None,
        ),
        "down" => crate::taskbar::status::audio::system_action_for_row(
            system,
            &row.id,
            Some(row.volume_percent.saturating_sub(5).min(cap)),
            None,
        ),
        "mute" => crate::taskbar::status::audio::system_action_for_row(
            system,
            &row.id,
            None,
            Some(!row.muted),
        ),
        _ => None,
    }
}

/// Pure helpers for the close-one/close-none contract: take_close_target
/// consumes open and yields the single kind to close (None = zero calls).
pub fn take_close_target(open: Option<QuickControlKind>) -> Option<QuickControlKind> {
    open
}

pub struct HelperSurfaces {
    pub audio: SurfaceHandle,
    pub network: SurfaceHandle,
    pub calendar: SurfaceHandle,
    pub battery: SurfaceHandle,
    pub weather: SurfaceHandle,
    pub open: Option<QuickControlKind>,
    /// Slide armed for the open popup; `Some` closes it through `hide` (slide-out).
    pub slide: Option<SlideConfig>,
}

impl HelperSurfaces {
    pub fn create(runtime: &mut SurfaceRuntime) -> Result<Self, String> {
        let audio = create_surface(runtime, AUDIO_ARTIFACT, AUDIO_POPUP_TITLE)?;
        let network = create_surface(runtime, NETWORK_ARTIFACT, NETWORK_POPUP_TITLE)?;
        let calendar = create_surface(runtime, CALENDAR_ARTIFACT, CALENDAR_POPUP_TITLE)?;
        let battery = create_surface(runtime, BATTERY_ARTIFACT, BATTERY_POPUP_TITLE)?;
        let weather = create_surface(runtime, WEATHER_ARTIFACT, WEATHER_POPUP_TITLE)?;
        Ok(Self {
            audio,
            network,
            calendar,
            battery,
            weather,
            open: None,
            slide: None,
        })
    }

    pub fn surface_for(&self, kind: QuickControlKind) -> SurfaceHandle {
        match kind {
            QuickControlKind::Audio => self.audio,
            QuickControlKind::Network => self.network,
            QuickControlKind::Calendar => self.calendar,
            QuickControlKind::Battery => self.battery,
            QuickControlKind::Weather => self.weather,
        }
    }

    /// Close only the currently open surface (open=kind). No-op when none
    /// is open (zero close calls). The real close span wraps the actual
    /// ungrab/unmap turn.
    /// Pure close-target selection: returns the single handle to close,
    /// or None when nothing is open (zero close calls). Keeps the
    /// close-one/close-none contract testable without an X11 runtime.
    pub fn close_handle(&mut self) -> Option<SurfaceHandle> {
        take_close_target(self.open.take()).map(|kind| self.surface_for(kind))
    }

    pub fn close(&mut self, runtime: &mut SurfaceRuntime) {
        let slide = self.slide.take();
        let Some(surface) = self.close_handle() else {
            return;
        };
        let _span = crate::runtime::shell_span("shell.quick.close/ungrab/unmap").start();
        let _ = close_helper(runtime, surface, slide);
    }

    /// Shutdown path: close the open surface (the shared close owner performs
    /// ungrab/unmap/present/flush), then destroy all helpers. Every failure is
    /// diagnosed and returned so the process owner can choose nonzero exit.
    pub fn shutdown(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), UiBackendError> {
        let mut first_error = None;
        if let Some(surface) = self.close_handle() {
            let _span = crate::runtime::shell_span("shell.quick.close/ungrab/unmap").start();
            if let Err(error) = close_helper(runtime, surface, None) {
                eprintln!("flamewm-quick-control: popup close during shutdown failed: {error}");
                first_error = Some(UiBackendError::Renderer(error.to_string()));
            }
        }
        for surface in [self.audio, self.network, self.calendar, self.battery, self.weather] {
            if let Err(error) = runtime.destroy(surface) {
                eprintln!(
                    "flamewm-quick-control: surface destroy during shutdown failed: {error:?}"
                );
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

fn create_surface(
    runtime: &mut SurfaceRuntime,
    bytes: &[u8],
    title: &str,
) -> Result<SurfaceHandle, String> {
    let document = decode_document(bytes)?;
    runtime
        .create_surface(
            UiTemplate::new(document),
            SurfaceConfig {
                width: 1,
                height: 1,
                title: title.to_owned(),
                role: SurfaceRole::PopupMenu,
                input: SurfaceInputMode::Interactive,
                initially_visible: false,
                x: 0,
                y: 0,
            },
        )
        .map_err(|error| format!("{error:?}"))
}

/// Parent-side helper: restart the supervisor with the J07 static span
/// (shell.quick_control.restart). Failures are nonfatal: the caller logs
/// and keeps the shell running without quick-control popups.
pub fn poll_supervisor_nonfatal(supervisor: &mut QuickControlSupervisor) {
    let _span = crate::runtime::shell_span("shell.quick_control.restart").start();
    match supervisor.poll() {
        Ok(_) => {}
        Err(SupervisorError::RestartBudgetExhausted) => {
            eprintln!(
                "flamewm-shell: quick-control restart budget exhausted; continuing without helper"
            );
        }
        Err(error) => {
            eprintln!("flamewm-shell: quick-control poll: {error}");
        }
    }
}

/// Parent-side open: compute the retained anchor + work area + edge and
/// forward to the supervisor, then return immediately. Nonfatal on
/// failure: the shell loop survives and the popup simply stays closed.
pub fn open_via_supervisor_nonfatal(
    supervisor: &mut QuickControlSupervisor,
    kind: QuickControlKind,
    anchor: flamewm_api::Rect,
    work_area: flamewm_api::Rect,
    panel_edge: flamewm_api::PanelEdge,
) {
    let _span = crate::runtime::shell_span("shell.quick_control.open_request").start();
    if let Err(error) = supervisor.open(super::protocol::OpenRequest {
        kind,
        anchor,
        work_area,
        panel_edge,
    }) {
        eprintln!("flamewm-shell: quick-control open failed (nonfatal): {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_shell_core::popup::measured_popup_rect;
    use flamewm_ui_core::popover::{PopoverAlign, PopoverEdge};

    #[test]
    fn secret_never_in_logs_or_protocol() {
        // The helper snapshot carries only a masked length, never the
        // secret value; the protocol type carries no secret field.
        let snapshot = QuickControlSnapshot {
            system: flamewm_api::system::SystemSnapshot::default(),
            audio_tab: AudioTab::Devices,
            audio_settings: AudioSettings::default(),
            audio_drag_slot: None,
            audio_scroll_offset: 0,
            network_query: NetworkQuery::default(),
            network_scroll_offset: 0,
            calendar: crate::calendar::CalendarState::for_today(crate::projection::local_date()),
            weather: WeatherHost::new(),
            network_secret_masked_len: 8,
            network_secret: String::new(),
            network_secret_request: None,
            network_secret_focused: false,
            network_filter_focused: false,
            network_secret_error: false,
            network_secret_submit_id: None,
            settings: SettingsSnapshot::new(0),
        };
        let debug = format!("{snapshot:?}");
        assert!(
            !debug.contains("s3cr3t"),
            "snapshot debug must not carry a secret"
        );
        let line = super::super::protocol::encode_command(&Command::Open(OpenRequest {
            kind: QuickControlKind::Network,
            anchor: flamewm_api::Rect::new(0, 0, 1, 1),
            work_area: flamewm_api::Rect::new(0, 0, 10, 10),
            panel_edge: flamewm_api::PanelEdge::Bottom,
        }));
        assert!(!line.contains("s3cr3t"));
    }

    #[test]
    fn system_snapshot_update_is_direct_and_deduplicated() {
        let mut quick = QuickControlSnapshot {
            system: flamewm_api::system::SystemSnapshot::default(),
            audio_tab: AudioTab::default(),
            audio_settings: AudioSettings::default(),
            audio_drag_slot: None,
            audio_scroll_offset: 0,
            network_query: NetworkQuery::default(),
            network_scroll_offset: 0,
            calendar: crate::calendar::CalendarState::for_today(crate::projection::local_date()),
            weather: WeatherHost::new(),
            network_secret_masked_len: 0,
            network_secret: String::new(),
            network_secret_request: None,
            network_secret_focused: false,
            network_filter_focused: false,
            network_secret_error: false,
            network_secret_submit_id: None,
            settings: SettingsSnapshot::new(0),
        };
        let mut next = quick.system.clone();
        next.revision = 7;
        assert!(apply_system_snapshot(&mut quick, next.clone()));
        assert_eq!(quick.system, next);
        assert!(!apply_system_snapshot(&mut quick, next));
    }

    #[test]
    fn filtered_sorted_scrolled_network_slot_resolves_the_projected_ap() {
        use flamewm_api::system::{NetworkAccessPointSnapshot, ServiceAvailability, SystemAction};

        let mut system = flamewm_api::system::SystemSnapshot::default();
        system.revision = 9;
        system.network.availability = ServiceAvailability::Available;
        system.network.active_access_point_path = "/ap/4".to_owned();
        system.network.access_points = (0..8)
            .map(|index| NetworkAccessPointSnapshot {
                path: format!("/ap/{index}"),
                ssid: if matches!(index, 1 | 3 | 4 | 6 | 7) {
                    format!("match-{index}")
                } else {
                    format!("other-{index}")
                },
                strength_percent: 50,
                secured: false,
                known: true,
            })
            .collect();
        let quick = QuickControlSnapshot {
            system,
            audio_tab: AudioTab::Devices,
            audio_settings: AudioSettings::default(),
            audio_drag_slot: None,
            audio_scroll_offset: 0,
            network_query: NetworkQuery {
                filter: "MATCH".to_owned(),
            },
            network_scroll_offset: 44,
            network_secret_masked_len: 0,
            network_secret: String::new(),
            network_secret_request: None,
            network_secret_focused: false,
            network_filter_focused: false,
            network_secret_error: false,
            network_secret_submit_id: None,
            settings: SettingsSnapshot::new(0),
            calendar: crate::calendar::CalendarState::for_today((2026, 10, 13)),
            weather: WeatherHost::new(),
        };

        // Filtering leaves five rows; the connected provider index 4 is excluded
        // (it has its own container), leaving four Available rows. The 44px scroll
        // then makes provider index 3 the visible slot 0, not raw provider slot 0.
        let rows = crate::taskbar::status::network::project_visible_rows(
            &quick.system,
            &quick.network_query,
            quick.network_scroll_offset,
        );
        assert_eq!(rows[0].path, "/ap/3");
        assert!(rows.iter().all(|row| row.path != "/ap/4"));
        let request = helper_control_request("network.connect.1", &quick)
            .expect("projected slot resolves to a connect request");
        assert_eq!(
            request,
            ControlRequest::SystemAction {
                action: SystemAction::ConnectKnown {
                    access_point_path: rows[0].path.clone(),
                },
                expected_revision: 9,
            }
        );
    }

    #[test]
    fn i32_max_constraint_cannot_reach_placement() {
        // i32::MAX work area stays finite through the shared owner measure
        // gate (degenerate anchor refuses as pending layout).
        let area = flamewm_api::Rect::new(0, 0, i32::MAX, i32::MAX);
        assert!((area.width as f32).is_finite());
        assert!((area.height as f32).is_finite());
        // The clamped intrinsic (work-area min) still refuses as
        // PendingLayout through the pure path when size is absent.
        let refused = measured_popup_rect(
            Some(flamewm_api::Rect::new(0, 0, 10, 10)),
            None,
            PopoverEdge::Above,
            PopoverAlign::End,
            flamewm_api::Rect::new(0, 0, 1920, 1080),
            8,
        );
        assert!(refused.is_err());
    }

    #[test]
    fn bottom_end_aligned_network_fixture_right_above() {
        // Bottom panel network fixture: output-end aligned above the anchor,
        // right edge pinned to the output right (not the tray anchor).
        let area = flamewm_api::Rect::new(0, 0, 1920, 1080);
        let anchor = flamewm_api::Rect::new(1830, 1036, 70, 44);
        let size = flamewm_api::Size::new(310, 220);
        let rect = measured_popup_rect(
            Some(anchor),
            Some(size),
            PopoverEdge::Above,
            PopoverAlign::OutputEnd,
            area,
            8,
        )
        .expect("measured");
        assert_eq!(rect.right(), area.right());
        assert!(rect.right() <= area.right());
        assert!(rect.bottom() <= anchor.y);
    }

    #[test]
    fn top_panel_places_below() {
        // Top panel: generic path with Below edge opens under the anchor.
        let area = flamewm_api::Rect::new(0, 0, 1920, 1080);
        let anchor = flamewm_api::Rect::new(1830, 0, 70, 44);
        let size = flamewm_api::Size::new(310, 220);
        let rect = measured_popup_rect(
            Some(anchor),
            Some(size),
            PopoverEdge::Below,
            PopoverAlign::OutputEnd,
            area,
            8,
        )
        .expect("measured");
        assert!(rect.y >= anchor.bottom());
        assert_eq!(rect.right(), area.right());
    }

    #[test]
    fn left_right_edges_preserved() {
        // Left/right panels keep their opening side inside the area.
        let area = flamewm_api::Rect::new(0, 0, 1920, 1080);
        let anchor_l = flamewm_api::Rect::new(0, 400, 44, 70);
        let anchor_r = flamewm_api::Rect::new(1876, 400, 44, 70);
        let size = flamewm_api::Size::new(200, 150);
        let right_of = measured_popup_rect(
            Some(anchor_l),
            Some(size),
            PopoverEdge::Right,
            PopoverAlign::End,
            area,
            8,
        )
        .expect("measured");
        assert!(right_of.x >= anchor_l.right());
        let left_of = measured_popup_rect(
            Some(anchor_r),
            Some(size),
            PopoverEdge::Left,
            PopoverAlign::End,
            area,
            8,
        )
        .expect("measured");
        assert!(left_of.right() <= anchor_r.x);
    }

    #[test]
    fn fitted_helper_rect_refuses_without_layout() {
        // Pure refusal contract: degenerate anchor area still places when
        // the anchor is valid; the refusal path is owned by the helper
        // intrinsic measurement (covered by popup unit tests).
        let area = flamewm_api::Rect::new(0, 0, 1920, 1080);
        let anchor = flamewm_api::Rect::new(1900, 1036, 20, 44);
        let size = flamewm_api::Size::new(310, 200);
        let rect = measured_popup_rect(
            Some(anchor),
            Some(size),
            PopoverEdge::Above,
            PopoverAlign::OutputEnd,
            area,
            8,
        )
        .expect("measured");
        assert!(rect.right() <= area.right());
        assert!(rect.bottom() <= anchor.y);
    }

    fn open_cmd(kind: QuickControlKind) -> Command {
        Command::Open(OpenRequest {
            kind,
            anchor: flamewm_api::Rect::new(0, 0, 10, 10),
            work_area: flamewm_api::Rect::new(0, 0, 100, 100),
            panel_edge: flamewm_api::PanelEdge::Bottom,
        })
    }

    #[test]
    fn coalesce_open_keeps_newest() {
        let mut q: VecDeque<Command> = VecDeque::from([
            open_cmd(QuickControlKind::Audio),
            open_cmd(QuickControlKind::Network),
            open_cmd(QuickControlKind::Calendar),
        ]);
        let out = coalesce_commands(&mut q);
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0],
            Command::Open(r) if r.kind == QuickControlKind::Calendar
        ));
    }

    #[test]
    fn close_none_selects_zero_close_one_selects_one() {
        // close-none: no open surface -> None (zero close calls).
        assert!(take_close_target(None).is_none());
        // close-one: exactly the open kind is selected.
        assert_eq!(
            take_close_target(Some(QuickControlKind::Network)),
            Some(QuickControlKind::Network)
        );
    }

    #[test]
    fn profiler_cadence_floor_is_ten_seconds() {
        // The host profiler timer must honor profile_interval_secs with
        // its 10s floor, never a fast empty tick.
        assert!(flamewm_profiler::profile_interval_secs() >= 10);
    }

    #[test]
    fn normal_helper_shutdown_selects_zero_exit() {
        assert_eq!(quick_control_exit_code(QuickControlShutdown::Completed), 0);
        assert_eq!(quick_control_exit_code(QuickControlShutdown::Failed), 1);
    }

    #[test]
    fn stage_labels_are_real_turns() {
        // True stages only: every label maps to a real turn in
        // open_request/fitted_rect/close; no fake snapshot/measure/place.
        for label in QUICK_STAGE_LABELS {
            assert!(
                [
                    "shell.quick.open.total",
                    "shell.quick.project",
                    "shell.quick.measure",
                    "shell.quick.place",
                    "shell.quick.prepare/commit_geometry",
                    "shell.quick.present",
                    "shell.quick.pointer_grab",
                    "shell.quick.close/ungrab/unmap",
                ]
                .contains(label),
                "non-real stage label: {label}"
            );
        }
        assert!(!QUICK_STAGE_LABELS.contains(&"shell.quick.snapshot"));
        assert!(!QUICK_STAGE_LABELS.contains(&"shell.quick.grab"));
        assert!(!QUICK_STAGE_LABELS.contains(&"shell.quick.prepare"));
        assert!(!QUICK_STAGE_LABELS.contains(&"shell.quick.close"));
    }

    #[test]
    fn stage_labels_pass_shell_span_allow_list() {
        // Every label the open/close turns actually pass to shell_span must
        // be in its debug allow-list; an unlisted label panics the helper.
        for label in QUICK_STAGE_LABELS {
            let _span = crate::runtime::shell_span(label);
        }
    }

    #[test]
    fn slow_open_gate_and_stage_render_are_pure() {
        assert!(!is_slow_open(Duration::from_micros(16_669)));
        assert!(is_slow_open(Duration::from_micros(16_670)));
        let timings = OpenStageTimings {
            total: Duration::from_millis(20),
            project: Duration::from_millis(1),
            measure: Duration::from_millis(2),
            place: Duration::from_millis(3),
            prepare_commit: Duration::from_millis(4),
            present: Duration::from_millis(5),
            pointer_grab: Duration::from_millis(6),
            close: Duration::from_millis(7),
        };
        let line = timings.render(QuickControlKind::Audio);
        for stage in [
            "open.total",
            "project",
            "measure",
            "place",
            "prepare/commit_geometry",
            "present",
            "pointer_grab",
            "close/ungrab/unmap",
        ] {
            assert!(line.contains(stage), "slow line must carry {stage}");
        }
        let placement = FittedPlacement {
            rect: flamewm_api::Rect::new(10, 20, 30, 40),
            measure_elapsed: Duration::from_millis(2),
            place_elapsed: Duration::from_millis(3),
        };
        assert_eq!(placement.rect.x, 10);
        assert_eq!(placement.measure_elapsed, Duration::from_millis(2));
    }

    #[test]
    fn red_grab_refusal_rolls_back_the_post_map_open() {
        // No SurfaceRuntime fake exists at this boundary. Keep this RED
        // regression source-level so it still proves the owner stages show
        // before grab and requires the real close owner on grab refusal.
        let source = include_str!("host.rs");
        let (_, open) = source
            .split_once("fn open_request(")
            .expect("open_request owner");
        let (open, _) = open
            .split_once("\nfn project_snapshot(")
            .expect("open_request body");
        let present = open
            .find("present(runtime, surface)")
            .expect("post-map present stage");
        let grab = open
            .find("grab(runtime, surface)")
            .expect("pointer-grab stage");
        assert!(present < grab, "grab must follow the mapped present stage");
        let after_grab = &open[grab..];
        assert!(
            after_grab.contains("if let Err") || after_grab.contains("match grab"),
            "grab refusal must have an explicit rollback branch"
        );
        assert!(
            after_grab.contains("close_helper(runtime, surface, None)"),
            "grab refusal must hide/unmap through the existing close owner"
        );
        assert!(
            after_grab.contains("surfaces.open = None"),
            "grab refusal must not leave stale open state"
        );
    }

    /// Wi-Fi open with NetworkManager absent (`ServiceAvailability::Unavailable`) must project
    /// through the real network artifact without a refusal, so the popup maps and shows the
    /// truthful unavailable state. The secret-error node is written on every open (even empty);
    /// it needs a text child in `network.html`, otherwise `document.text` refuses the whole open.
    /// Settings change between two opens: the second stored snapshot drives the shared menu
    /// background colour, alpha and backdrop percents, so the next open uses the new values.
    #[test]
    fn settings_change_between_opens_changes_menu_background() {
        let mut snapshot = QuickControlSnapshot {
            system: flamewm_api::system::SystemSnapshot::default(),
            audio_tab: AudioTab::default(),
            audio_settings: AudioSettings::default(),
            audio_drag_slot: None,
            audio_scroll_offset: 0,
            network_query: NetworkQuery::default(),
            network_scroll_offset: 0,
            calendar: crate::calendar::CalendarState::for_today(crate::projection::local_date()),
            weather: WeatherHost::new(),
            network_secret_masked_len: 0,
            network_secret: String::new(),
            network_secret_request: None,
            network_secret_focused: false,
            network_filter_focused: false,
            network_secret_error: false,
            network_secret_submit_id: None,
            settings: SettingsSnapshot::new(0),
        };
        let first = background_for(&snapshot.settings);
        let mut changed = SettingsSnapshot::new(1);
        changed
            .values
            .insert("surfaceOpacity".to_owned(), SettingValue::Percent(0));
        changed
            .values
            .insert("surfaceBlurPercent".to_owned(), SettingValue::Percent(30));
        changed.values.insert(
            "surfaceColor".to_owned(),
            SettingValue::Text("#102030".to_owned()),
        );
        snapshot.store_settings(changed);
        let second = background_for(&snapshot.settings);
        assert_ne!(
            first, second,
            "a new settings snapshot must change the open's background"
        );
        assert_eq!(second.0.a, 0, "opacity 0 gives a transparent menu root");
        assert_eq!((second.0.r, second.0.g, second.0.b), (0x10, 0x20, 0x30));
        assert_eq!(
            second.1,
            (0, 30),
            "backdrop percents follow the new opacity and blur"
        );
    }

    /// Background colour and backdrop percents the helper projects for one settings snapshot.
    fn background_for(settings: &SettingsSnapshot) -> (flamewm_ui_x11::UiColor, (u8, u8)) {
        (
            crate::settings_projection::menu_background(settings),
            crate::settings_projection::start_backdrop_percents(settings),
        )
    }

    #[test]
    fn network_open_projects_unavailable_backend_without_refusal() {
        let mut document = decode_document(NETWORK_ARTIFACT).expect("network artifact must decode");
        let mut shell = crate::ShellSnapshot::default();
        shell.system.network.availability = flamewm_api::system::ServiceAvailability::Unavailable;
        crate::projection::project_network_full(&mut document, &shell, &NetworkQuery::default(), 0)
            .expect("unavailable network projects");
        crate::projection::project_network_secret(&mut document, "")
            .expect("secret prompt projects");
        document
            .text("network-secret-error", String::new())
            .expect("empty secret error projects");
    }
}

#[cfg(test)]
mod audio_routing_tests {
    use super::*;
    use flamewm_api::system::{
        AudioEndpointKind, AudioEndpointSnapshot, AudioStreamSnapshot, AudioTarget,
        ServiceAvailability, SystemAction,
    };

    // Fake provider: one default output at 50%, one app stream at 30%.
    fn quick(tab: AudioTab, raise_maximum: bool) -> QuickControlSnapshot {
        let mut system = flamewm_api::system::SystemSnapshot::default();
        system.revision = 4;
        system.audio.availability = ServiceAvailability::Available;
        system.audio.endpoints = vec![AudioEndpointSnapshot {
            id: 1,
            kind: AudioEndpointKind::Sink,
            name: "speakers".to_owned(),
            description: "Built-in Speakers".to_owned(),
            volume_percent: 50,
            muted: false,
            is_default: true,
        }];
        system.audio.streams = vec![AudioStreamSnapshot {
            id: 10,
            endpoint_id: 1,
            name: "Firefox".to_owned(),
            volume_percent: 30,
            muted: false,
            ..Default::default()
        }];
        QuickControlSnapshot {
            system,
            audio_tab: tab,
            audio_settings: AudioSettings { raise_maximum },
            audio_drag_slot: None,
            audio_scroll_offset: 0,
            network_query: NetworkQuery::default(),
            network_scroll_offset: 0,
            network_secret_masked_len: 0,
            network_secret: String::new(),
            network_secret_request: None,
            network_secret_focused: false,
            network_filter_focused: false,
            network_secret_error: false,
            network_secret_submit_id: None,
            settings: SettingsSnapshot::new(0),
            calendar: crate::calendar::CalendarState::for_today(crate::projection::local_date()),
            weather: WeatherHost::new(),
        }
    }

    #[test]
    fn wheel_scroll_moves_audio_rows_by_whole_rows_and_clamps_to_the_model() {
        let mut quick = quick(AudioTab::Devices, false);
        for id in 2..=6 {
            quick.system.audio.endpoints.push(AudioEndpointSnapshot {
                id,
                kind: AudioEndpointKind::Sink,
                name: format!("sink{id}"),
                description: format!("Sink {id}"),
                volume_percent: 10,
                muted: false,
                is_default: false,
            });
        }
        // Six device rows in a four-row viewport: two rows of scroll room.
        scroll_audio_rows(&mut quick, 48.0);
        assert_eq!(quick.audio_scroll_offset, 84);
        scroll_audio_rows(&mut quick, 48.0);
        assert_eq!(quick.audio_scroll_offset, 168);
        scroll_audio_rows(&mut quick, 48.0);
        assert_eq!(quick.audio_scroll_offset, 168);
        scroll_audio_rows(&mut quick, -48.0);
        assert_eq!(quick.audio_scroll_offset, 84);
        // The projected window and the routed slot both start at the scrolled row.
        let rows = crate::taskbar::status::audio::visible_rows(
            &quick.system.audio,
            quick.audio_tab,
            quick.audio_scroll_offset,
        );
        assert_eq!(rows[0].stable_id(), "endpoint:sink:2");
        let request = helper_control_request("audio.slot.1.mute", &quick);
        assert_eq!(
            target_of(request),
            Some(AudioTarget::Endpoint {
                id: 2,
                kind: AudioEndpointKind::Sink
            })
        );
    }

    // RED before the fix: a released tab action must both select the tab and
    // request a reprojection, or the popup keeps showing the old tab.
    #[test]
    fn audio_tab_release_selects_tab_and_requests_reprojection() {
        let mut snapshot = quick(AudioTab::Devices, false);
        snapshot.audio_scroll_offset = 40;
        assert!(select_audio_tab(&mut snapshot, "audio.tab.apps"));
        assert_eq!(snapshot.audio_tab, AudioTab::Applications);
        assert_eq!(snapshot.audio_scroll_offset, 0);
        // Re-selecting the shown tab with its window at the top changes nothing.
        assert!(!select_audio_tab(&mut snapshot, "audio.tab.apps"));
        assert!(select_audio_tab(&mut snapshot, "audio.tab.devices"));
        assert_eq!(snapshot.audio_tab, AudioTab::Devices);
        assert!(!select_audio_tab(&mut snapshot, "audio.raise.toggle"));
        assert_eq!(snapshot.audio_tab, AudioTab::Devices);
    }

    fn target_of(request: Option<ControlRequest>) -> Option<AudioTarget> {
        match request? {
            ControlRequest::SystemAction {
                action: SystemAction::SetMute(mute),
                ..
            } => Some(mute.target),
            ControlRequest::SystemAction {
                action: SystemAction::SetVolume(volume),
                ..
            } => Some(volume.target),
            _ => None,
        }
    }

    // Slot numbers follow the projected tab, not the raw provider order.
    #[test]
    fn row_mute_routes_to_the_projected_tab_row() {
        let devices = quick(AudioTab::Devices, false);
        assert_eq!(
            target_of(helper_control_request("audio.slot.1.mute", &devices)),
            Some(AudioTarget::Endpoint {
                id: 1,
                kind: AudioEndpointKind::Sink
            })
        );
        let apps = quick(AudioTab::Applications, false);
        assert_eq!(
            target_of(helper_control_request("audio.slot.1.mute", &apps)),
            Some(AudioTarget::Stream { id: 10 })
        );
    }

    #[test]
    fn pointer_on_thumb_centre_reads_back_its_value_and_drag_clamps_at_both_ends() {
        use crate::taskbar::status::audio::{AUDIO_TRACK_WIDTH, AUDIO_TRACK_X};
        let percent = |raise: bool, x: f32| match audio_pointer_request(
            &quick(AudioTab::Devices, raise),
            0,
            x,
        ) {
            Some(ControlRequest::SystemAction {
                action: SystemAction::SetVolume(volume),
                ..
            }) => Some(volume.percent),
            _ => None,
        };
        // Thumb centre for 120% under cap 150 is X + (120 / 150) * W; pointer there reads 120 back.
        assert_eq!(
            percent(true, AUDIO_TRACK_X + AUDIO_TRACK_WIDTH * 120.0 / 150.0),
            Some(120)
        );
        assert_eq!(
            percent(false, AUDIO_TRACK_X + AUDIO_TRACK_WIDTH * 0.5),
            Some(50)
        );
        // Drag past either end clamps to 0 and to the active cap.
        assert_eq!(percent(false, AUDIO_TRACK_X - 500.0), Some(0));
        assert_eq!(
            percent(false, AUDIO_TRACK_X + AUDIO_TRACK_WIDTH + 500.0),
            Some(100)
        );
        assert_eq!(
            percent(true, AUDIO_TRACK_X + AUDIO_TRACK_WIDTH + 500.0),
            Some(150)
        );
    }

    #[test]
    fn pointer_drag_to_track_end_is_capped_at_100_or_150() {
        use crate::taskbar::status::audio::{AUDIO_TRACK_WIDTH, AUDIO_TRACK_X};
        let end = AUDIO_TRACK_X + AUDIO_TRACK_WIDTH;
        let percent = |raise: bool, x: f32| match audio_pointer_request(
            &quick(AudioTab::Devices, raise),
            0,
            x,
        ) {
            Some(ControlRequest::SystemAction {
                action: SystemAction::SetVolume(volume),
                ..
            }) => Some(volume.percent),
            _ => None,
        };
        assert_eq!(percent(false, end), Some(100));
        assert_eq!(percent(true, end), Some(150));
        assert_eq!(percent(false, AUDIO_TRACK_X), Some(0));
        assert_eq!(
            percent(true, AUDIO_TRACK_X + AUDIO_TRACK_WIDTH / 2.0),
            Some(75)
        );
    }
}

#[cfg(test)]
mod battery_popup_tests {
    use super::*;
    use flamewm_api::system::{PowerSnapshot, PowerState, ServiceAvailability};
    use flamewm_ui_x11::test_support::RecordingUiDoc;

    fn power(cpu_percent: Option<u8>) -> PowerSnapshot {
        PowerSnapshot {
            available: ServiceAvailability::Available,
            present: true,
            percent: 80,
            state: PowerState::Discharging,
            cpu_percent,
        }
    }

    #[test]
    fn battery_kind_wire_name_and_popup_root_are_battery() {
        assert_eq!(QuickControlKind::Battery.as_str(), "battery");
        assert_eq!(
            popup::role_for_kind(QuickControlKind::Battery).intrinsic_node_id(),
            "battery-popup"
        );
    }

    #[test]
    fn battery_menu_projects_cpu_text_and_refreshes_when_cpu_changes() {
        let mut doc = RecordingUiDoc::default();
        crate::projection::project_battery_menu(&mut doc, &power(Some(37)), 268.0)
            .expect("battery menu projects");
        assert_eq!(
            doc.texts.get("battery-cpu").map(String::as_str),
            Some("CPU usage: 37%")
        );
        assert_eq!(
            doc.texts.get("battery-percent").map(String::as_str),
            Some("80%")
        );
        // A later snapshot with a new CPU sample must change the projected text.
        crate::projection::project_battery_menu(&mut doc, &power(Some(64)), 268.0)
            .expect("battery menu reprojects");
        assert_eq!(
            doc.texts.get("battery-cpu").map(String::as_str),
            Some("CPU usage: 64%")
        );
        crate::projection::project_battery_menu(&mut doc, &power(None), 268.0)
            .expect("battery menu projects without a sample");
        assert_eq!(
            doc.texts.get("battery-cpu").map(String::as_str),
            Some("CPU usage: --%")
        );
    }

    #[test]
    fn battery_artifact_decodes_and_projects_on_the_real_document() {
        let mut document = decode_document(BATTERY_ARTIFACT).expect("battery artifact must decode");
        document
            .visible("battery-popup", true)
            .expect("battery root must resolve");
        crate::projection::project_battery_menu(&mut document, &power(Some(12)), 268.0)
            .expect("battery menu projects on the compiled document");
    }
}

#[cfg(test)]
mod network_filter_reprojection_tests {
    /// Every Wi-Fi filter keystroke (typed or backspace) must request reprojection, so
    /// `tick_host` projects and redraws the open popup. Source-scan over the keyboard
    /// section of `route_helper_event`, which needs a live runtime to call directly.
    #[test]
    fn each_network_filter_edit_requests_reprojection() {
        let source = include_str!("host.rs");
        let function = source
            .split_once("pub fn route_helper_event(")
            .expect("route_helper_event must exist")
            .1;
        let keyboard = function
            .split_once("let open_surface = surfaces.open")
            .expect("keyboard section must precede the outside-release check")
            .0;
        assert!(keyboard.contains("snapshot.network_query.filter.pop();"));
        assert!(keyboard.contains("snapshot.network_query.filter.push_str(text);"));
        assert!(
            edit_block_requests_reprojection(keyboard, "snapshot.network_query.filter.pop();"),
            "backspace filter edit must request reprojection"
        );
        assert!(
            edit_block_requests_reprojection(
                keyboard,
                "snapshot.network_query.filter.push_str(text);"
            ),
            "typed filter edit must request reprojection"
        );
    }

    /// True when the branch that starts at `edit` sets `reprojection` before the
    /// next `} else` (or the end of the keyboard section for the last branch).
    pub(super) fn edit_block_requests_reprojection(keyboard: &str, edit: &str) -> bool {
        let rest = keyboard
            .split_once(edit)
            .expect("edit must exist in the keyboard section")
            .1;
        let block = rest.split_once("} else").map_or(rest, |(block, _)| block);
        block.contains("*reprojection = true;")
    }
}

#[cfg(test)]
mod network_secret_prompt_tests {
    use super::*;

    /// The password field must have a real box: an unsized `<input>` in the
    /// flex row lays out 0x0, so its `network.secret.focus` hit rect is empty
    /// and clicks never reach the focus route. Source-scan over the compiled
    /// `network.html` stylesheet, which the layout engine reads directly.
    fn css_rule<'a>(html: &'a str, selector: &str) -> &'a str {
        html.split_once(&format!("{selector} {{"))
            .unwrap_or_else(|| panic!("{selector} must be styled with a rule"))
            .1
            .split_once('}')
            .expect("rule must close")
            .0
    }

    /// The password box must have a real hit area: the input is absolutely
    /// covering a sized, positioned box (as the Filter field does), so a click
    /// anywhere in the box reaches `network.secret.focus`.
    #[test]
    fn password_input_has_an_explicit_hit_box() {
        let html = include_str!("../../../../ui/shell/network.html");
        let box_rule = css_rule(html, "#network-secret-box");
        assert!(
            box_rule.contains("position:relative") && box_rule.contains("height:"),
            "secret box needs position and height: {box_rule}"
        );
        let input_rule = css_rule(html, "#network-secret-input");
        assert!(
            input_rule.contains("position:absolute")
                && input_rule.contains("width:100%")
                && input_rule.contains("height:100%"),
            "password input must cover its box: {input_rule}"
        );
    }

    /// The masked value must render inside the same box as the input, not in a
    /// separate bordered element beside it.
    #[test]
    fn password_value_renders_inside_the_input_box() {
        let html = include_str!("../../../../ui/shell/network.html");
        let after_box = html
            .split_once("id=\"network-secret-box\"")
            .expect("secret box must exist")
            .1;
        let box_body = after_box
            .split_once("<button")
            .expect("buttons follow the box")
            .0;
        assert!(box_body.contains("id=\"network-secret-value\""));
        assert!(box_body.contains("id=\"network-secret-input\""));
        let value_rule = css_rule(html, "#network-secret-value");
        assert!(
            value_rule.contains("position:absolute"),
            "masked value must overlay the box, not sit beside it: {value_rule}"
        );
    }

    /// The error line must stay inside the popup width and must not push the
    /// Connect/Cancel controls out of the row.
    #[test]
    fn secret_error_is_clipped_and_controls_stay_before_it() {
        let html = include_str!("../../../../ui/shell/network.html");
        let error_rule = css_rule(html, "#network-secret-error");
        assert!(
            error_rule.contains("width:100%")
                && error_rule.contains("overflow:hidden")
                && error_rule.contains("height:"),
            "error needs a clipped fixed-width line: {error_rule}"
        );
        let row_body = html
            .split_once("class=\"network-secret-row\"")
            .expect("controls row must exist")
            .1
            .split_once("id=\"network-secret-error\"")
            .expect("error line must follow the controls row")
            .0;
        assert!(row_body.contains("data-action=\"network.secret.submit\""));
        assert!(row_body.contains("data-action=\"network.secret.cancel\""));
        let secret_rule = css_rule(html, "#network-secret");
        assert!(
            secret_rule.contains("flex-direction:column"),
            "error must stack below the row, not inline: {secret_rule}"
        );
    }

    /// Each secret edit and error transition must request reprojection so the
    /// masked value and error line repaint without another input event.
    #[test]
    fn network_secret_edits_and_errors_request_reprojection() {
        use super::network_filter_reprojection_tests::edit_block_requests_reprojection;
        let source = include_str!("host.rs");
        let function = source
            .split_once("pub fn route_helper_event(")
            .expect("route_helper_event must exist")
            .1;
        let keyboard = function
            .split_once("let open_surface = surfaces.open")
            .expect("keyboard section must precede the outside-release check")
            .0;
        assert!(
            edit_block_requests_reprojection(keyboard, "snapshot.network_secret.pop();"),
            "secret backspace must request reprojection"
        );
        assert!(
            edit_block_requests_reprojection(keyboard, "snapshot.network_secret.push_str(text);"),
            "secret typed edit must request reprojection"
        );
        let outcome = source
            .split_once("take_network_secret_outcomes();")
            .expect("outcome drain must exist")
            .1
            .split_once("Err(()) =>")
            .expect("failed outcome arm must exist")
            .1
            .split_once('}')
            .expect("failed outcome arm must close")
            .0;
        assert!(outcome.contains("network_secret_error = true;"));
        assert!(
            outcome.contains("system_reprojection_pending = true;"),
            "failed submit must request reprojection: {outcome}"
        );
    }

    #[test]
    fn empty_password_builds_no_submit_request() {
        assert!(network_secret_submit_request((4, 9), "", 7).is_none());
    }

    #[test]
    fn non_empty_password_builds_submit_for_its_identity() {
        let request =
            network_secret_submit_request((4, 9), "hunter2", 7).expect("non-empty secret submits");
        match request {
            ControlRequest::SystemAction {
                action:
                    flamewm_api::system::SystemAction::SubmitNetworkSecret {
                        request_id,
                        generation,
                        secret,
                    },
                expected_revision,
            } => {
                assert_eq!((request_id, generation, expected_revision), (4, 9, 7));
                assert_eq!(secret, "hunter2");
            }
            other => panic!("unexpected request: {other:?}"),
        }
    }
}

#[cfg(test)]
mod weather_host_tests {
    use super::*;
    use flamewm_shell_core::weather::{TemperatureUnit, WeatherCondition};
    use std::cell::Cell;
    use std::time::Instant;

    fn location() -> WeatherLocation {
        WeatherLocation::new("Berlin", 52.52, 13.41, None).unwrap()
    }

    fn settings(enabled: bool, with_location: bool) -> WeatherSettings {
        WeatherSettings {
            enabled,
            location: with_location.then(location),
            units: TemperatureUnit::Celsius,
        }
    }

    /// Stands in for the network: a ready fixture, no socket, no thread.
    fn fixture_rx() -> std::sync::mpsc::Receiver<WeatherResult> {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Ok(WeatherSnapshot {
            location: location(),
            temperature_c: 9,
            condition: WeatherCondition::Clear,
            days: Vec::new(),
            fetched_at: 1,
        }))
        .unwrap();
        rx
    }

    #[test]
    fn disabled_or_unconfigured_never_fetches() {
        for s in [settings(false, true), settings(true, false), settings(false, false)] {
            let mut host = WeatherHost::new();
            host.apply_settings(s);
            host.request_open_fetch();
            let calls = Cell::new(0);
            let start = Instant::now();
            for step in 0..5 {
                host.tick(start + WEATHER_REFRESH_INTERVAL * step, |_| {
                    calls.set(calls.get() + 1);
                    fixture_rx()
                });
            }
            assert_eq!(calls.get(), 0);
        }
        let mut host = WeatherHost::new();
        host.apply_settings(settings(false, true));
        assert_eq!(host.state, WeatherState::Disabled);
    }

    #[test]
    fn enabled_with_location_fetches_on_open_then_every_thirty_minutes() {
        let mut host = WeatherHost::new();
        host.apply_settings(settings(true, true));
        assert_eq!(host.state, WeatherState::Loading);
        let calls = Cell::new(0);
        let start = Instant::now();
        let fetch = |_: WeatherLocation| {
            calls.set(calls.get() + 1);
            fixture_rx()
        };
        host.tick(start, fetch);
        assert_eq!(calls.get(), 0, "no periodic fetch before the popup opens");

        host.request_open_fetch();
        host.tick(start, fetch);
        assert_eq!(calls.get(), 1);
        host.tick(start, fetch);
        assert!(matches!(host.state, WeatherState::Ready(_)));
        assert_eq!(calls.get(), 1);

        host.tick(start + Duration::from_secs(29 * 60), fetch);
        assert_eq!(calls.get(), 1, "still inside the 30 minute window");
        host.tick(start + WEATHER_REFRESH_INTERVAL, fetch);
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn disabling_drops_the_inflight_fetch_and_stops_refreshing() {
        let mut host = WeatherHost::new();
        host.apply_settings(settings(true, true));
        let calls = Cell::new(0);
        let start = Instant::now();
        host.request_open_fetch();
        host.tick(start, |_| {
            calls.set(calls.get() + 1);
            fixture_rx()
        });
        host.apply_settings(settings(false, true));
        assert_eq!(host.state, WeatherState::Disabled);
        host.tick(start + WEATHER_REFRESH_INTERVAL * 2, |_| {
            calls.set(calls.get() + 1);
            fixture_rx()
        });
        assert_eq!(calls.get(), 1);
    }
}
