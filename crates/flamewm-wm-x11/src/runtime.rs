use std::cell::{Cell, RefCell};
use std::env;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{self, TryRecvError};
use std::time::Instant;

use calloop::{Interest, RegistrationToken};

use crate::X11Desktop;
use flamewm_api::ports::FdEvents;
use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_api::system::{AudioSnapshot, SystemAction};
use flamewm_applications::ApplicationCatalog;
use flamewm_control_dbus::ControlServer;
use flamewm_control_wire::ControlSignal;
use flamewm_integrations_linux::{
    LinuxIntegrationRuntime, LinuxIntegrationSource, pulse::PulseAdapter,
};
use flamewm_platform::host::PlatformHost;
use flamewm_reactor::{FdAction, Reactor};

use crate::atoms::AnyError;
use crate::wm::{WmChangeSet, WmConfig, run_with_hook};

struct WmBuilderFields {
    discover_counter: &'static flamewm_profiler::CounterPoint,
}

fn wm_builder_fields() -> &'static WmBuilderFields {
    use std::sync::OnceLock;
    static FIELDS: OnceLock<WmBuilderFields> = OnceLock::new();
    FIELDS.get_or_init(|| WmBuilderFields {
        discover_counter: Box::leak(Box::new(flamewm_profiler::CounterPoint::new(
            "wm.catalog.discover.count",
        ))),
    })
}

/// Discover-once composition root: single `ApplicationCatalog::discover`
/// per WM process, counted under `wm.catalog.discover.count`. The returned
/// `Arc` is injected into the launch adapter, `DecorationManager`, and the
/// host snapshot; launch/repaint paths never rediscover.
fn discover_catalog_once() -> Result<std::sync::Arc<ApplicationCatalog>, AnyError> {
    wm_builder_fields().discover_counter.increment();
    Ok(std::sync::Arc::new(ApplicationCatalog::discover()?))
}

pub fn run(config: WmConfig) -> Result<(), AnyError> {
    flamewm_profiler::init_process("flamewm-wm");
    flamewm_debug::init_process("wm");
    let settings_path = settings_path();
    let control = Rc::new(ControlServer::connect_session()?);
    let reactor = Rc::new(RefCell::new(Reactor::new()?));
    let integrations = Rc::new(RefCell::new(LinuxIntegrationRuntime::connect()?));
    if let Err(error) = integrations.borrow_mut().start() {
        integrations.borrow_mut().stop();
        return Err(error.into());
    }
    let provider_error = Rc::new(RefCell::new(None));
    let mut network_registration = match register_provider(
        &reactor,
        &integrations,
        &provider_error,
        LinuxIntegrationSource::NetworkManager,
    ) {
        Ok(registration) => registration,
        Err(error) => {
            integrations.borrow_mut().stop();
            return Err(error);
        }
    };
    let mut mpris_registration = match register_provider(
        &reactor,
        &integrations,
        &provider_error,
        LinuxIntegrationSource::Mpris,
    ) {
        Ok(registration) => registration,
        Err(error) => {
            integrations.borrow_mut().stop();
            return Err(error);
        }
    };
    let (audio_sender, audio_receiver) = mpsc::sync_channel::<AudioSnapshot>(32);
    let (audio_reader, mut audio_writer) = match UnixStream::pair() {
        Ok(pair) => pair,
        Err(error) => {
            integrations.borrow_mut().stop();
            return Err(error.into());
        }
    };
    if let Err(error) = audio_reader.set_nonblocking(true) {
        integrations.borrow_mut().stop();
        return Err(error.into());
    }
    if let Err(error) = audio_writer.set_nonblocking(true) {
        integrations.borrow_mut().stop();
        return Err(error.into());
    }
    let pulse = match PulseAdapter::connect(move |snapshot| {
        if audio_sender.try_send(snapshot).is_ok() {
            let _ = audio_writer.write(&[1]);
        }
    }) {
        Ok(pulse) => Rc::new(pulse),
        Err(error) => {
            integrations.borrow_mut().stop();
            return Err(error.into());
        }
    };
    let audio_updates = Rc::new(RefCell::new(Vec::<AudioSnapshot>::new()));
    let audio_updates_for_callback = Rc::clone(&audio_updates);
    let _audio_registration = match reactor.borrow_mut().register_fd_with_source_action(
        audio_reader,
        Interest::READ,
        move |_, source, _| {
            let mut wake_buffer = [0_u8; 128];
            loop {
                match source.read(&mut wake_buffer) {
                    Ok(0) => return FdAction::Remove,
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(_) => return FdAction::Remove,
                }
            }
            let mut updates = audio_updates_for_callback.borrow_mut();
            loop {
                match audio_receiver.try_recv() {
                    Ok(snapshot) => updates.push(snapshot),
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
                }
            }
            FdAction::Continue
        },
    ) {
        Ok(token) => token,
        Err(error) => {
            drop(pulse);
            integrations.borrow_mut().stop();
            return Err(error.into());
        }
    };
    let host = Rc::new(RefCell::new(None::<PlatformHost<X11Desktop>>));
    let control_now_ms = Rc::new(Cell::new(0_u64));
    let control_error = Rc::new(RefCell::new(None::<AnyError>));
    let mut control_registration =
        match register_control(&reactor, &control, &host, &control_now_ms, &control_error) {
            Ok(registration) => registration,
            Err(error) => {
                drop(pulse);
                integrations.borrow_mut().stop();
                return Err(error);
            }
        };
    // Profiler report timer on the reactor; interval from
    // `FLAMEWM_PROFILE_INTERVAL` (floored at 10s, default 60s), never
    // hardcoded here. No timer thread.
    let _profiler_registration = reactor
        .borrow_mut()
        .register_timer(profile_report_interval(), true, || {
            if flamewm_profiler::enabled() {
                let report = flamewm_profiler::report_window();
                if !report.is_empty() {
                    eprintln!("{report}");
                }
            }
        })
        .map_err(|error| {
            drop(pulse.clone());
            integrations.borrow_mut().stop();
            AnyError::from(io::Error::new(io::ErrorKind::Other, error.to_string()))
        })?;
    let _startup_guard = flamewm_profiler::start("wm.startup.catalog");
    let catalog = discover_catalog_once()?;
    drop(_startup_guard);
    let catalog_for_hook = std::sync::Arc::clone(&catalog);
    let started = Instant::now();
    let mut host_started = false;
    let integrations_for_hook = Rc::clone(&integrations);
    let provider_error_for_hook = Rc::clone(&provider_error);
    let pulse_for_hook = Rc::clone(&pulse);
    let audio_updates_for_hook = Rc::clone(&audio_updates);
    let reactor_for_hook = Rc::clone(&reactor);
    let host_for_hook = Rc::clone(&host);
    let control_for_hook = Rc::clone(&control);
    let control_now_ms_for_hook = Rc::clone(&control_now_ms);
    let control_error_for_hook = Rc::clone(&control_error);

    let result = run_with_hook(config, &catalog, &reactor, move |conn, screen, changes| {
        if let Some(error) = control_error_for_hook.borrow_mut().take() {
            return Err(error);
        }
        refresh_control(
            &reactor_for_hook,
            &mut control_registration,
            &control_for_hook,
            &host_for_hook,
            &control_now_ms_for_hook,
            &control_error_for_hook,
        )?;
        let mut host = host_for_hook.borrow_mut();
        // Scoped per-turn span: the whole-process `wm.loop` guard above has
        // been removed so loop-turn CPU is attributed per turn, not once
        // across the full process lifetime.
        let _turn_guard = flamewm_profiler::start("wm.loop");
        if host.is_none() {
            let mut new_host = PlatformHost::new(
                X11Desktop::from_connection(
                    conn.clone(),
                    screen,
                    std::sync::Arc::clone(&catalog_for_hook),
                )?,
                settings_path.clone(),
            );
            let _ = new_host.replace_applications(catalog_for_hook.all());
            let integrations = Rc::clone(&integrations_for_hook);
            let pulse = Rc::clone(&pulse_for_hook);
            new_host.set_system_action_handler(Box::new(move |action, snapshot| match action {
                SystemAction::SetVolume(action) => pulse.set_volume(action),
                SystemAction::SetMute(action) => pulse.set_mute(action),
                SystemAction::SetWifiEnabled(_)
                | SystemAction::ConnectKnown { .. }
                | SystemAction::ConnectWifi { .. }
                | SystemAction::SubmitNetworkSecret { .. }
                | SystemAction::CancelNetworkSecret { .. }
                | SystemAction::Disconnect
                | SystemAction::Scan
                | SystemAction::Play { .. }
                | SystemAction::Pause { .. }
                | SystemAction::PlayPause { .. }
                | SystemAction::Next { .. }
                | SystemAction::Previous { .. } => integrations.borrow_mut().perform_action(
                    action,
                    &snapshot.network,
                    &snapshot.media,
                ),
            }));
            *host = Some(new_host);
        }
        let host = host.as_mut().expect("host initialized");
        if !host_started {
            let _host_guard = flamewm_profiler::start("wm.startup.host");
            host.start()?;
            sync_snap_preview_opacity(&host.settings_snapshot());
            host_started = true;
        }
        // ProviderDirty audio domain: drain only snapshots the audio fd delivered.
        {
            let mut audio_updates = audio_updates_for_hook.borrow_mut();
            for snapshot in audio_updates.drain(..) {
                if host.system_mut().update_audio(snapshot) {
                    emit_system_signals(host, control_for_hook.as_ref())?;
                }
            }
        }
        if let Some(error) = provider_error_for_hook.borrow_mut().take() {
            return Err(error.into());
        }
        // ProviderDirty network/media domains: reconcile event-coalesced state
        // without polling; snapshots only refresh on reported change.
        {
            let mut integrations = integrations_for_hook.borrow_mut();
            if integrations.reconcile()? {
                let network = integrations.network_snapshot();
                let media = integrations.media_snapshot();
                if host.system_mut().update_network(network) {
                    emit_system_signals(host, control_for_hook.as_ref())?;
                }
                if host.system_mut().update_media(media) {
                    emit_system_signals(host, control_for_hook.as_ref())?;
                }
            }
        }
        refresh_provider(
            &reactor_for_hook,
            &mut network_registration,
            &integrations_for_hook,
            &provider_error_for_hook,
        )?;
        refresh_provider(
            &reactor_for_hook,
            &mut mpris_registration,
            &integrations_for_hook,
            &provider_error_for_hook,
        )?;
        let now_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
        control_now_ms_for_hook.set(now_ms);
        flush_wm_changes(host, control_for_hook.as_ref(), changes)?;
        Ok(())
    });
    drop(pulse);
    integrations.borrow_mut().stop();
    result
}

fn emit_system_signals<E>(host: &PlatformHost<E>, control: &ControlServer) -> Result<(), AnyError>
where
    E: flamewm_api::ports::EnginePorts,
{
    let snapshot = host.system_snapshot();
    control.emit_signal(&ControlSignal::SystemSnapshotChanged {
        snapshot: snapshot.clone(),
    })?;
    control.emit_signal(&ControlSignal::SystemChanged {
        revision: snapshot.revision,
    })?;
    Ok(())
}

fn sync_snap_preview_opacity(snapshot: &SettingsSnapshot) {
    if let Some(SettingValue::Percent(opacity)) =
        snapshot.values.get("WindowSnapPreviewFillOpacity")
    {
        crate::snap_preview::set_live_opacity(*opacity);
    }
}

/// Drain one `WmChangeSet` once per reactor turn: refresh only required
/// snapshots, emit one snapshot signal per changed domain plus legacy
/// revision-only compat. MANAGE/MINIMIZE/RESTORE/CLOSE -> Windows+Panels,
/// WORKSPACE SWITCH -> Workspaces+Panels. `work_area`-only turns emit nothing.
fn flush_wm_changes<E>(
    host: &mut PlatformHost<E>,
    control: &ControlServer,
    changes: WmChangeSet,
) -> Result<(), AnyError>
where
    E: flamewm_api::ports::EnginePorts,
{
    for signal in collect_wm_signals(host, changes).map_err(any_from_flame)? {
        control.emit_signal(&signal).map_err(any_from_flame)?;
    }
    Ok(())
}

fn collect_wm_signals<E>(
    host: &mut PlatformHost<E>,
    changes: WmChangeSet,
) -> flamewm_api::FlameResult<Vec<ControlSignal>>
where
    E: flamewm_api::ports::EnginePorts,
{
    let mut signals = Vec::new();
    if changes.windows {
        let panels_before = host.panels_snapshot();
        if host.refresh_windows()? {
            let windows = host.engine().snapshot()?;
            let revision = windows
                .iter()
                .map(|window| window.state_generation)
                .max()
                .unwrap_or(0);
            signals.push(ControlSignal::WindowsSnapshotChanged { revision, windows });
            signals.push(ControlSignal::WindowsChanged { revision });
        }
        let panels = host.panels_snapshot();
        if panels != panels_before {
            signals.push(ControlSignal::PanelsSnapshotChanged { snapshot: panels });
        }
    }
    if changes.workspaces {
        let snapshot = host.workspace_snapshot()?;
        let revision = snapshot.revision;
        signals.push(ControlSignal::WorkspacesSnapshotChanged { snapshot });
        signals.push(ControlSignal::WorkspacesChanged { revision });
        signals.push(ControlSignal::PanelsSnapshotChanged {
            snapshot: host.panels_snapshot(),
        });
    }
    if changes.panels && !changes.windows && !changes.workspaces {
        signals.push(ControlSignal::PanelsSnapshotChanged {
            snapshot: host.panels_snapshot(),
        });
    }
    Ok(signals)
}

fn any_from_flame(error: flamewm_api::FlameError) -> AnyError {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::Other,
        error.to_string(),
    ))
}

struct ControlRegistration {
    fd: i32,
    events: FdEvents,
    token: RegistrationToken,
}

fn register_control(
    reactor: &Rc<RefCell<Reactor>>,
    control: &Rc<ControlServer>,
    host: &Rc<RefCell<Option<PlatformHost<X11Desktop>>>>,
    now_ms: &Rc<Cell<u64>>,
    control_error: &Rc<RefCell<Option<AnyError>>>,
) -> Result<ControlRegistration, AnyError> {
    let watch = control.watch();
    let control = Rc::clone(control);
    let host = Rc::clone(host);
    let now_ms = Rc::clone(now_ms);
    let control_error = Rc::clone(control_error);
    let token = reactor.borrow_mut().register_raw_fd_with_action(
        watch.fd,
        interest_for(watch.events),
        move |_, readiness| {
            if readiness.error || readiness.hangup {
                let mut control_error = control_error.borrow_mut();
                if control_error.is_none() {
                    *control_error = Some(Box::new(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "control D-Bus FD closed",
                    )));
                }
                return FdAction::Remove;
            }
            if !(readiness.readable || readiness.writable) {
                return FdAction::Continue;
            }
            let mut host = host.borrow_mut();
            let Some(host) = host.as_mut() else {
                return FdAction::Continue;
            };
            let panels_before = host.panels_snapshot();
            let result = control.on_ready(host, now_ms.get());
            if let Err(error) = result {
                let mut control_error = control_error.borrow_mut();
                if control_error.is_none() {
                    *control_error = Some(Box::new(error));
                }
                FdAction::Remove
            } else {
                sync_snap_preview_opacity(&host.settings_snapshot());
                let panels = host.panels_snapshot();
                if panels != panels_before {
                    if let Err(error) = control
                        .emit_signal(&ControlSignal::PanelsSnapshotChanged { snapshot: panels })
                    {
                        let mut control_error = control_error.borrow_mut();
                        if control_error.is_none() {
                            *control_error = Some(Box::new(error));
                        }
                        return FdAction::Remove;
                    }
                }
                FdAction::Continue
            }
        },
    )?;
    Ok(ControlRegistration {
        fd: watch.fd,
        events: watch.events,
        token,
    })
}

fn refresh_control(
    reactor: &Rc<RefCell<Reactor>>,
    registration: &mut ControlRegistration,
    control: &Rc<ControlServer>,
    host: &Rc<RefCell<Option<PlatformHost<X11Desktop>>>>,
    now_ms: &Rc<Cell<u64>>,
    control_error: &Rc<RefCell<Option<AnyError>>>,
) -> Result<(), AnyError> {
    let watch = control.watch();
    if watch.fd == registration.fd && watch.events == registration.events {
        return Ok(());
    }
    reactor.borrow_mut().remove(registration.token)?;
    *registration = register_control(reactor, control, host, now_ms, control_error)?;
    Ok(())
}

struct ProviderRegistration {
    source: LinuxIntegrationSource,
    fd: i32,
    events: FdEvents,
    token: RegistrationToken,
}

fn register_provider(
    reactor: &Rc<RefCell<Reactor>>,
    integrations: &Rc<RefCell<LinuxIntegrationRuntime>>,
    provider_error: &Rc<RefCell<Option<flamewm_api::FlameError>>>,
    source: LinuxIntegrationSource,
) -> Result<ProviderRegistration, AnyError> {
    let watch = integrations.borrow().watch(source);
    let integrations = Rc::clone(integrations);
    let provider_error = Rc::clone(provider_error);
    let token = reactor.borrow_mut().register_raw_fd_with_action(
        watch.fd,
        interest_for(watch.events),
        move |_, _| {
            if let Err(error) = integrations.borrow_mut().on_ready(source) {
                let mut provider_error = provider_error.borrow_mut();
                if provider_error.is_none() {
                    *provider_error = Some(error);
                }
            }
            FdAction::Continue
        },
    )?;
    Ok(ProviderRegistration {
        source,
        fd: watch.fd,
        events: watch.events,
        token,
    })
}

fn refresh_provider(
    reactor: &Rc<RefCell<Reactor>>,
    registration: &mut ProviderRegistration,
    integrations: &Rc<RefCell<LinuxIntegrationRuntime>>,
    provider_error: &Rc<RefCell<Option<flamewm_api::FlameError>>>,
) -> Result<(), AnyError> {
    let watch = integrations.borrow().watch(registration.source);
    if watch.fd == registration.fd && watch.events == registration.events {
        return Ok(());
    }
    reactor.borrow_mut().remove(registration.token)?;
    *registration = register_provider(reactor, integrations, provider_error, registration.source)?;
    Ok(())
}

fn interest_for(events: FdEvents) -> Interest {
    match (
        events.contains(FdEvents::READABLE),
        events.contains(FdEvents::WRITABLE),
    ) {
        (true, true) => Interest::BOTH,
        (false, true) => Interest::WRITE,
        _ => Interest::READ,
    }
}

/// Profiler report interval shared with `flamewm-profiler`; never hardcode
/// the delay here.
fn profile_report_interval() -> std::time::Duration {
    std::time::Duration::from_secs(flamewm_profiler::profile_interval_secs())
}

fn settings_path() -> PathBuf {
    env::var_os("FLAMEWM_SETTINGS_PATH")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("FLAMEWM_CONFIG_DIR").map(|path| PathBuf::from(path).join("settings.toml"))
        })
        .unwrap_or_else(|| PathBuf::from("settings.toml"))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use flamewm_api::applications::ApplicationLaunchOptions;
    use flamewm_api::display::{DisplaySnapshot, OutputSnapshot};
    use flamewm_api::input::PointerPosition;
    use flamewm_api::ports::{
        ApplicationPort, DisplayPort, InputPort, SessionPort, ShortcutPort, WindowPort,
        WorkspacePort,
    };
    use flamewm_api::session::{SessionAction, SessionCapabilities};
    use flamewm_api::shortcuts::KeyBinding;
    use flamewm_api::window::WindowSnapshot;
    use flamewm_api::workspace::{WorkspacePlan, WorkspaceSnapshot};
    use flamewm_api::{
        DesktopAppId, FlameResult, ModeId, OutputId, Rect, TransactionId, WindowRef,
    };

    use super::collect_wm_signals;
    use crate::wm::WmChangeSet;

    struct FakeEngine {
        windows: Vec<WindowSnapshot>,
        workspaces: WorkspaceSnapshot,
    }

    fn fake_window(id: u64, generation: u64) -> WindowSnapshot {
        WindowSnapshot {
            reference: WindowRef::new(id, 0),
            title: format!("w{id}"),
            app_id: DesktopAppId::new("org.example.Fake"),
            outer_geometry: Rect::new(0, 0, 100, 100),
            restore_geometry: Rect::new(0, 0, 100, 100),
            state: flamewm_api::window::WindowState::Normal,
            sticky: false,
            focused: false,
            workspace: flamewm_api::WorkspaceRef::new(0, 0),
            output: OutputId::new("eDP-1"),
            state_generation: generation,
        }
    }

    impl FakeEngine {
        fn new() -> Self {
            Self {
                windows: vec![fake_window(1, 1)],
                workspaces: WorkspaceSnapshot {
                    revision: 2,
                    count: 2,
                    active_index: 0,
                    last_index: None,
                    names: Vec::new(),
                },
            }
        }

        fn host(self) -> PlatformHost<Self> {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(1);
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("flamewm-j13-{}-{id}", std::process::id()));
            let mut host = PlatformHost::new(self, path);
            host.start().expect("host starts");
            host
        }
    }

    use flamewm_platform::host::PlatformHost;

    impl WindowPort for FakeEngine {
        fn get(&self, window: WindowRef) -> FlameResult<WindowSnapshot> {
            self.windows
                .iter()
                .find(|item| item.reference.id == window.id)
                .cloned()
                .ok_or_else(|| flamewm_api::FlameError::not_found("missing"))
        }
        fn snapshot(&self) -> FlameResult<Vec<WindowSnapshot>> {
            Ok(self.windows.clone())
        }
        fn activate(&mut self, _window: WindowRef) -> FlameResult<()> {
            Ok(())
        }
        fn minimize(&mut self, _window: WindowRef) -> FlameResult<()> {
            Ok(())
        }
        fn maximize(&mut self, _window: WindowRef) -> FlameResult<()> {
            Ok(())
        }
        fn restore(&mut self, _window: WindowRef) -> FlameResult<()> {
            Ok(())
        }
        fn close(&mut self, window: WindowRef) -> FlameResult<()> {
            self.windows.retain(|item| item.reference.id != window.id);
            Ok(())
        }
        fn set_outer_geometry(&mut self, _window: WindowRef, _geometry: Rect) -> FlameResult<()> {
            Ok(())
        }
        fn work_area(&self, _window: WindowRef) -> FlameResult<Rect> {
            Ok(Rect::new(0, 0, 1920, 1036))
        }
        fn output(&self, _window: WindowRef) -> FlameResult<OutputId> {
            Ok(OutputId::new("eDP-1"))
        }
    }

    impl WorkspacePort for FakeEngine {
        fn workspace_snapshot(&self) -> FlameResult<WorkspaceSnapshot> {
            Ok(self.workspaces.clone())
        }
        fn activate_workspace(&mut self, index: usize, _revision: u64) -> FlameResult<()> {
            self.workspaces.active_index = index;
            self.workspaces.revision += 1;
            Ok(())
        }
        fn move_window_to_workspace(
            &mut self,
            _window: WindowRef,
            _target: usize,
        ) -> FlameResult<()> {
            Ok(())
        }
        fn switch_workspace_with_window(
            &mut self,
            _window: WindowRef,
            target: usize,
            revision: u64,
        ) -> FlameResult<()> {
            self.activate_workspace(target, revision)
        }
        fn apply_workspace_plan(&mut self, _plan: &WorkspacePlan) -> FlameResult<()> {
            Ok(())
        }
    }

    impl DisplayPort for FakeEngine {
        fn display_snapshot(&self) -> FlameResult<DisplaySnapshot> {
            Ok(DisplaySnapshot {
                generation: 1,
                outputs: vec![OutputSnapshot {
                    id: OutputId::new("eDP-1"),
                    connector: "eDP-1".to_owned(),
                    edid_identity: "panel".to_owned(),
                    connected: true,
                    primary: true,
                    geometry: Rect::new(0, 0, 1920, 1080),
                    current_mode: ModeId(1),
                    modes: Vec::new(),
                    shell_scale_percent: 100,
                }],
                pending: None,
            })
        }
        fn apply_mode(&mut self, _output: &OutputId, _mode: ModeId) -> FlameResult<TransactionId> {
            Ok(TransactionId(1))
        }
        fn keep_mode(&mut self, _transaction: TransactionId) -> FlameResult<()> {
            Ok(())
        }
        fn revert_mode(&mut self, _transaction: TransactionId) -> FlameResult<()> {
            Ok(())
        }
    }

    impl ShortcutPort for FakeEngine {
        fn prepare_shortcuts(
            &mut self,
            _desired: &BTreeMap<String, KeyBinding>,
        ) -> FlameResult<()> {
            Ok(())
        }
        fn commit_shortcuts(&mut self) -> FlameResult<()> {
            Ok(())
        }
        fn rollback_shortcuts(&mut self) {}
    }

    impl InputPort for FakeEngine {
        fn root_pointer(&self) -> FlameResult<PointerPosition> {
            Ok(PointerPosition {
                root: flamewm_api::Point::new(0, 0),
                output: None,
            })
        }
    }

    impl ApplicationPort for FakeEngine {
        fn launch(
            &mut self,
            _app: &DesktopAppId,
            _options: &ApplicationLaunchOptions,
        ) -> FlameResult<()> {
            Ok(())
        }
        fn launch_uri(&mut self, _uri: &str) -> FlameResult<()> {
            Ok(())
        }
    }

    impl SessionPort for FakeEngine {
        fn session_capabilities(&self) -> SessionCapabilities {
            SessionCapabilities {
                lock: false,
                logout: false,
                suspend: false,
                reboot: false,
                shutdown: false,
            }
        }
        fn perform_session_action(&mut self, _action: SessionAction) -> FlameResult<()> {
            Ok(())
        }
    }

    fn change(windows: bool, workspaces: bool, panels: bool) -> WmChangeSet {
        WmChangeSet {
            windows,
            workspaces,
            panels,
            work_area: false,
        }
    }

    fn members(signals: &[flamewm_control_wire::ControlSignal]) -> Vec<&'static str> {
        signals
            .iter()
            .map(|signal| match signal {
                flamewm_control_wire::ControlSignal::WindowsSnapshotChanged { .. } => {
                    "WindowsSnapshotChanged"
                }
                flamewm_control_wire::ControlSignal::WindowsChanged { .. } => "WindowsChanged",
                flamewm_control_wire::ControlSignal::WorkspacesSnapshotChanged { .. } => {
                    "WorkspacesSnapshotChanged"
                }
                flamewm_control_wire::ControlSignal::WorkspacesChanged { .. } => {
                    "WorkspacesChanged"
                }
                flamewm_control_wire::ControlSignal::PanelsSnapshotChanged { .. } => {
                    "PanelsSnapshotChanged"
                }
                _ => "other",
            })
            .collect()
    }

    #[test]
    fn empty_changeset_emits_no_signals() {
        let mut host = FakeEngine::new().host();
        let signals = collect_wm_signals(&mut host, change(false, false, false)).expect("collect");
        assert!(signals.is_empty());
    }

    #[test]
    fn manage_window_emits_windows_plus_panels_once() {
        let mut host = FakeEngine::new().host();
        let _ = collect_wm_signals(&mut host, change(true, false, false)).expect("seed");
        host.engine_mut().windows.push(fake_window(2, 2));
        let signals = collect_wm_signals(&mut host, change(true, false, false)).expect("collect");
        assert_eq!(
            members(&signals),
            vec![
                "WindowsSnapshotChanged",
                "WindowsChanged",
                "PanelsSnapshotChanged"
            ]
        );
    }

    #[test]
    fn unchanged_windows_emit_no_window_signals() {
        let mut host = FakeEngine::new().host();
        let _ = collect_wm_signals(&mut host, change(true, false, false)).expect("seed");
        let signals = collect_wm_signals(&mut host, change(true, false, false)).expect("collect");
        assert!(signals.is_empty());
    }

    #[test]
    fn workspace_switch_emits_workspaces_plus_panels() {
        let mut host = FakeEngine::new().host();
        let signals = collect_wm_signals(&mut host, change(false, true, false)).expect("collect");
        assert_eq!(
            members(&signals),
            vec![
                "WorkspacesSnapshotChanged",
                "WorkspacesChanged",
                "PanelsSnapshotChanged"
            ]
        );
    }

    #[test]
    fn work_area_only_emits_nothing() {
        let mut host = FakeEngine::new().host();
        let only_area = WmChangeSet {
            windows: false,
            workspaces: false,
            panels: false,
            work_area: true,
        };
        let signals = collect_wm_signals(&mut host, only_area).expect("collect");
        assert!(signals.is_empty());
    }
}
