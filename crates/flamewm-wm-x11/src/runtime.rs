use std::cell::{Cell, RefCell};
use std::env;
use std::io::{self, Read, Write};
use std::os::fd::RawFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{self, TryRecvError};
use std::time::Instant;

use calloop::{Interest, RegistrationToken};
use x11rb::connection::Connection as _;

use crate::{X11Desktop, log_committed_startup_shortcuts};
use flamewm_api::ports::FdEvents;
use flamewm_api::settings::{AppearanceMode, SettingValue, SettingsSnapshot};
use flamewm_api::system::{AudioSnapshot, SystemAction};
use flamewm_applications::ApplicationCatalog;
use flamewm_control_dbus::ControlServer;
use flamewm_control_wire::ControlSignal;
use flamewm_integrations_linux::{
    LinuxIntegrationRuntime, LinuxIntegrationSource, pulse::PulseAdapter,
};
use flamewm_platform::host::PlatformHost;
use flamewm_reactor::{FdAction, FdReadiness, Reactor};

use crate::atoms::AnyError;
use crate::idle;
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

/// Callback invoked with the current settings snapshot at each settings sync
/// point (after host start and after every control-channel dispatch). Runs on
/// the WM thread; it must not block.
pub type SettingsObserver = Box<dyn FnMut(&SettingsSnapshot)>;

/// Shared, optional observer handle threaded through the reactor callbacks.
type SharedSettingsObserver = Rc<RefCell<Option<SettingsObserver>>>;

/// Wake source for a settings owner (for example the XSettings manager). `fd`
/// is watched for read readiness on the reactor; `on_readable` drains the
/// owner's pending events and must not publish. An `Err` or error/hangup
/// readiness removes the registration, so a broken descriptor cannot spin the
/// level-triggered loop.
pub struct SettingsWake {
    pub fd: RawFd,
    pub on_readable: Box<dyn FnMut() -> Result<(), String>>,
}

/// Run the window manager with no settings observer.
pub fn run(config: WmConfig) -> Result<(), AnyError> {
    run_session(config, None, None)
}

/// Run the window manager, notifying `observer` at every settings sync point.
/// When `wake` is given, its descriptor is registered with the reactor so the
/// settings owner is drained on readiness without any timer or poll.
/// Behaviour is otherwise identical to [`run`].
pub fn run_with_settings_observer(
    config: WmConfig,
    observer: impl FnMut(&SettingsSnapshot) + 'static,
    wake: Option<SettingsWake>,
) -> Result<(), AnyError> {
    run_session(config, Some(Box::new(observer)), wake)
}

/// Decide the reactor action for one settings-owner wake. Error or hangup
/// readiness removes the source without draining. A failed drain removes it
/// too. Only a clean drain keeps it registered.
fn settings_wake_step(
    readiness: FdReadiness,
    drain: impl FnOnce() -> Result<(), String>,
) -> FdAction {
    if readiness.error || readiness.hangup {
        eprintln!("flamewm: XSettings manager connection closed; no longer watching it");
        return FdAction::Remove;
    }
    match drain() {
        Ok(()) => FdAction::Continue,
        Err(error) => {
            eprintln!("flamewm: XSettings manager wake failed; no longer watching it: {error}");
            FdAction::Remove
        }
    }
}

fn register_settings_wake(
    reactor: &Rc<RefCell<Reactor>>,
    wake: SettingsWake,
) -> Result<RegistrationToken, AnyError> {
    let SettingsWake {
        fd,
        mut on_readable,
    } = wake;
    reactor
        .borrow_mut()
        .register_raw_fd_with_action(fd, Interest::READ, move |_, readiness| {
            settings_wake_step(readiness, || on_readable())
        })
        .map_err(Into::into)
}

fn run_session(
    config: WmConfig,
    settings_observer: Option<SettingsObserver>,
    settings_wake: Option<SettingsWake>,
) -> Result<(), AnyError> {
    flamewm_profiler::init_process("flamewm-wm");
    flamewm_debug::init_process("wm");
    let (settings_path, settings_source) = settings_path();
    let config = config.with_persisted_workspaces(&settings_path);
    eprintln!(
        "flamewm-wm: settings path={} source={settings_source}",
        settings_path.display()
    );
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
    let mut power_registration = match register_provider(
        &reactor,
        &integrations,
        &provider_error,
        LinuxIntegrationSource::Power,
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
    let settings_observer: SharedSettingsObserver = Rc::new(RefCell::new(settings_observer));
    let control_now_ms = Rc::new(Cell::new(0_u64));
    let control_error = Rc::new(RefCell::new(None::<AnyError>));
    let mut control_registration = match register_control(
        &reactor,
        &control,
        &host,
        &control_now_ms,
        &control_error,
        &settings_observer,
    ) {
            Ok(registration) => registration,
            Err(error) => {
                drop(pulse);
                integrations.borrow_mut().stop();
                return Err(error);
            }
        };
    // Profiler report timer on the reactor; interval from
    // `FLAMEWM_PROFILE_INTERVAL` (floored at 10s, default 60s), never
    // hardcoded here. No timer thread. Not registered when profiling is off.
    let _profiler_registration = if flamewm_profiler::enabled() {
        Some(
            reactor
                .borrow_mut()
                .register_timer(profile_report_interval(), true, || {
                    let report = flamewm_profiler::report_window();
                    if !report.is_empty() {
                        eprintln!("{report}");
                    }
                })
                .map_err(|error| {
                    drop(pulse.clone());
                    integrations.borrow_mut().stop();
                    AnyError::from(io::Error::new(io::ErrorKind::Other, error.to_string()))
                })?,
        )
    } else {
        None
    };
    let _startup_guard = flamewm_profiler::start("wm.startup.catalog");
    let catalog = discover_catalog_once()?;
    drop(_startup_guard);
    // One worker is sufficient for this single WM; the shared index is built
    // before the event loop and all cold reads/decodes remain off-loop.
    let icon_base = flamewm_integrations_linux::IconResolver::from_environment(
        crate::wm::flamewm_workspace_root(),
        0,
    );
    let icon_index = std::sync::Arc::clone(icon_base.shared_index());
    let icon_base = std::sync::Arc::new(icon_base);
    let icon_service = Rc::new(RefCell::new(
        flamewm_integrations_linux::icon_service::IconService::with_index(
            1,
            128,
            move || icon_base.fork_worker(),
            icon_index,
        ),
    ));
    let icon_fd = icon_service.borrow().wake_fd();
    let _icon_registration = if let Some(icon_fd) = icon_fd {
        let icon_service_for_wake = Rc::clone(&icon_service);
        Some(reactor.borrow_mut().register_raw_fd_with_action(
            icon_fd,
            Interest::READ,
            move |_, _| {
                icon_service_for_wake.borrow_mut().wake_drain();
                FdAction::Continue
            },
        )?)
    } else {
        return Err(Box::new(io::Error::new(io::ErrorKind::Other, "IconService wake FD unavailable")));
    };
    // Settings owner wake: SelectionClear on the manager connection is observed
    // without waiting for the next publish. A failed registration leaves the WM
    // running without live XSettings wake, like a refused claim.
    let _settings_wake_registration = match settings_wake {
        Some(wake) => match register_settings_wake(&reactor, wake) {
            Ok(token) => Some(token),
            Err(error) => {
                eprintln!("flamewm: XSettings wake not registered: {error}");
                None
            }
        },
        None => None,
    };
    let catalog_for_hook = std::sync::Arc::clone(&catalog);
    let started = Instant::now();
    let mut host_started = false;
    let integrations_for_hook = Rc::clone(&integrations);
    let provider_error_for_hook = Rc::clone(&provider_error);
    let pulse_for_hook = Rc::clone(&pulse);
    let audio_updates_for_hook = Rc::clone(&audio_updates);
    let reactor_for_hook = Rc::clone(&reactor);
    let battery_menu_open_for_hook = Rc::new(Cell::new(false));
    let battery_menu_open_for_clone = Rc::clone(&battery_menu_open_for_hook);
    let power_cpu_due_for_hook = Rc::new(Cell::new(None::<Instant>));
    let host_for_hook = Rc::clone(&host);
    let control_for_hook = Rc::clone(&control);
    let control_now_ms_for_hook = Rc::clone(&control_now_ms);
    let control_error_for_hook = Rc::clone(&control_error);
    let settings_observer_for_hook = Rc::clone(&settings_observer);
    let idle_lock_for_hook = Rc::new(RefCell::new(idle::IdleLock::new()));

    let result = run_with_hook(config, &catalog, &reactor, Rc::clone(&icon_service), move |wm, conn, screen, changes| {
        if let Some(error) = control_error_for_hook.borrow_mut().take() {
            return Err(error);
        }
        wm.apply_decoration_tokens(crate::chrome::live_chrome_tokens())?;
        wm.apply_transition_settings(crate::animation::live_settings())?;
        let root = conn.setup().roots[screen].root;
        idle_lock_for_hook
            .borrow_mut()
            .turn(&reactor_for_hook, conn.as_ref(), root);
        refresh_control(
            &reactor_for_hook,
            &mut control_registration,
            &control_for_hook,
            &host_for_hook,
            &control_now_ms_for_hook,
            &control_error_for_hook,
            &settings_observer_for_hook,
        )?;
        let mut host_guard = host_for_hook.borrow_mut();
        // Scoped per-turn span: the whole-process `wm.loop` guard above has
        // been removed so loop-turn CPU is attributed per turn, not once
        // across the full process lifetime.
        let _turn_guard = flamewm_profiler::start("wm.loop");
        if host_guard.is_none() {
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
            let battery_open = Rc::clone(&battery_menu_open_for_clone);
            let pulse = Rc::clone(&pulse_for_hook);
            new_host.set_system_action_handler(Box::new(move |action, snapshot| {
                match action {
                    SystemAction::BatteryMenuOpened => battery_open.set(true),
                    SystemAction::BatteryMenuClosed => battery_open.set(false),
                    _ => {}
                }
                match action {
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
                | SystemAction::Previous { .. }
                | SystemAction::BatteryMenuOpened
                | SystemAction::BatteryMenuClosed => integrations.borrow_mut().perform_action(
                    action,
                    &snapshot.network,
                    &snapshot.media,
                ),
            }
        }));
            *host_guard = Some(new_host);
        }
        let host = host_guard.as_mut().expect("host initialized");
        if !host_started {
            let _host_guard = flamewm_profiler::start("wm.startup.host");
            host.start()?;
            match host.commit_startup_shortcuts() {
                Ok(()) => {
                    let snapshot = host.shortcut_snapshot();
                    log_committed_startup_shortcuts(host.engine(), &snapshot.bindings);
                }
                Err(error) => {
                    eprintln!("flamewm: startup shortcuts were not grabbed: {error}");
                }
            }
            publish_settings_snapshot(&settings_observer_for_hook, &host.settings_snapshot());
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
            let reconciled = integrations.reconcile()?;
            // Network replies (access points, profiles) land after reconcile
            // queues its requests, so the network snapshot is compared on every
            // turn; `update_network` dedupes, so unchanged state emits nothing.
            let network = integrations.network_snapshot();
            if host.system_mut().update_network(network) {
                emit_system_signals(host, control_for_hook.as_ref())?;
            }
            let power = integrations.power_snapshot();
            if host.system_mut().update_power(power) {
                emit_system_signals(host, control_for_hook.as_ref())?;
            }
            if reconciled {
                let media = integrations.media_snapshot();
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
        refresh_provider(
            &reactor_for_hook,
            &mut power_registration,
            &integrations_for_hook,
            &provider_error_for_hook,
        )?;
        // Battery-menu CPU sampling: deadline armed only while the menu is open (zero idle wakeups).
        let (changed, next_cpu_due) = tick_power_cpu(
            battery_menu_open_for_hook.get(),
            power_cpu_due_for_hook.get(),
            Instant::now(),
            || integrations_for_hook.borrow_mut().sample_power_cpu(),
        );
        power_cpu_due_for_hook.set(next_cpu_due);
        reactor_for_hook.borrow().wake_deadline().set(next_cpu_due);
        if changed && host.system_mut().update_power(integrations_for_hook.borrow().power_snapshot()) {
            emit_system_signals(host, control_for_hook.as_ref())?;
        }
        let now_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
        control_now_ms_for_hook.set(now_ms);
        // Deadline check only: reverts an unconfirmed display mode once its deadline passes.
        // RandR hotplug/mode notifications mark the topology dirty; refresh it before comparing.
        if wm.take_display_topology_dirty() {
            match host.display_snapshot() {
                Ok(displays) => {
                    if let Err(error) = host.resync_panels_for_displays(&displays) {
                        eprintln!("flamewm: panel resync after display change failed: {error}");
                    }
                }
                Err(error) => eprintln!("flamewm: display topology refresh failed: {error}"),
            }
        }
        let display_before = host.cached_display_snapshot();
        // The transaction is retired before any native call, so a failed or disconnected-output
        // revert is reported once here and never retried; it must not abort the WM loop.
        if let Err(error) = host.tick_display_transactions(now_ms) {
            eprintln!("flamewm: display mode transaction: {error}");
        }
        let display_after = host.cached_display_snapshot();
        if display_after != display_before {
            if let Some(snapshot) = display_after {
                control_for_hook.emit_signal(&ControlSignal::DisplaysSnapshotChanged { snapshot })?;
            }
        }
        flush_wm_changes(host, control_for_hook.as_ref(), changes)?;
        let areas = host.output_work_areas();
        let wait = crate::wm::next_wait_timeout(now_ms, host.display_transaction_deadline_ms());
        drop(host);
        drop(host_guard);
        wm.set_output_work_areas(areas)?;
        // Wake the reactor at the pending display revert deadline; None when nothing is pending.
        Ok(wait)
    });
    drop(icon_service);
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

/// Persisted `cursorTheme` from the settings file at startup, before the session
/// environment is applied. `None` when the file is missing or unparsable.
pub fn persisted_cursor_theme() -> Option<String> {
    let (path, _) = settings_path();
    let text = std::fs::read_to_string(path).ok()?;
    let settings = flamewm_platform::settings::ConfigStore::default()
        .parse_snapshot(&text)
        .ok()?;
    Some(settings.cursor_theme)
}

/// Persisted `appearance` from the settings file at startup. `None` when the
/// file is missing or unparsable.
pub fn persisted_appearance() -> Option<AppearanceMode> {
    let (path, _) = settings_path();
    let text = std::fs::read_to_string(path).ok()?;
    let settings = flamewm_platform::settings::ConfigStore::default()
        .parse_snapshot(&text)
        .ok()?;
    Some(settings.appearance)
}

/// Single settings sync point: applies the WM-owned snapshot effects, then
/// notifies the optional observer with the same snapshot.
fn publish_settings_snapshot(observer: &SharedSettingsObserver, snapshot: &SettingsSnapshot) {
    sync_snap_preview_opacity(snapshot);
    crate::live_settings::sync(snapshot);
    sync_window_decoration(snapshot);
    sync_idle_lock_timeout(snapshot);
    if let Some(observer) = observer.borrow_mut().as_mut() {
        observer(snapshot);
    }
}

fn sync_window_decoration(snapshot: &SettingsSnapshot) {
    crate::chrome::set_live_chrome_tokens(crate::chrome::chrome_tokens_from_settings(snapshot));
    crate::animation::set_live_settings(crate::animation::settings_from_snapshot(snapshot));
}

fn sync_idle_lock_timeout(snapshot: &SettingsSnapshot) {
    // Missing or out-of-range values fall back to manual lock only (0).
    let minutes = match snapshot.values.get("lockTimeoutMinutes") {
        Some(SettingValue::Integer(minutes)) => u16::try_from(*minutes).unwrap_or(0),
        _ => 0,
    };
    idle::set_live_lock_timeout(minutes);
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
    mut changes: WmChangeSet,
) -> Result<(), AnyError>
where
    E: flamewm_api::ports::EnginePorts,
{
    // Take the one-shot start-menu request out of the drained set so it is emitted
    // exactly once for this turn and never carried into a later flush.
    let start_menu_toggle = std::mem::take(&mut changes.start_menu_toggle);
    let media_audio_key = changes.media_audio_key.take();
    for signal in collect_wm_signals(host, changes).map_err(any_from_flame)? {
        control.emit_signal(&signal).map_err(any_from_flame)?;
    }
    if start_menu_toggle {
        control
            .emit_signal(&ControlSignal::StartMenuToggleRequested)
            .map_err(any_from_flame)?;
    }
    if let Some(key) = media_audio_key {
        // A rejected write (stale generation, no sink, Pulse down) must not stop the WM.
        if let Err(error) = host.media_audio_key(key) {
            eprintln!("flamewm: media key {key:?} was not applied: {error:?}");
        }
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
    settings_observer: &SharedSettingsObserver,
) -> Result<ControlRegistration, AnyError> {
    let watch = control.watch();
    let control = Rc::clone(control);
    let host = Rc::clone(host);
    let now_ms = Rc::clone(now_ms);
    let control_error = Rc::clone(control_error);
    let settings_observer = Rc::clone(settings_observer);
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
            let displays_before = host.cached_display_snapshot();
            let result = control.on_ready(host, now_ms.get());
            if let Err(error) = result {
                let mut control_error = control_error.borrow_mut();
                if control_error.is_none() {
                    *control_error = Some(Box::new(error));
                }
                FdAction::Remove
            } else {
                publish_settings_snapshot(&settings_observer, &host.settings_snapshot());
                // Displays first so the shell's display snapshot is current before panel geometry lands.
                let mut signals = Vec::new();
                let displays = host.cached_display_snapshot();
                if displays != displays_before {
                    if let Some(snapshot) = displays {
                        signals.push(ControlSignal::DisplaysSnapshotChanged { snapshot });
                    }
                }
                let panels = host.panels_snapshot();
                if panels != panels_before {
                    signals.push(ControlSignal::PanelsSnapshotChanged { snapshot: panels });
                }
                for signal in &signals {
                    if let Err(error) = control.emit_signal(signal) {
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
    settings_observer: &SharedSettingsObserver,
) -> Result<(), AnyError> {
    let watch = control.watch();
    if watch.fd == registration.fd && watch.events == registration.events {
        return Ok(());
    }
    reactor.borrow_mut().remove(registration.token)?;
    *registration = register_control(
        reactor,
        control,
        host,
        now_ms,
        control_error,
        settings_observer,
    )?;
    Ok(())
}

struct ProviderRegistration {
    source: LinuxIntegrationSource,
    fd: i32,
    events: FdEvents,
    token: RegistrationToken,
}

/// CPU sample cadence while the battery menu is open.
const POWER_CPU_SAMPLE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// Returns `(sample_now, next_due)`. The deadline is `None` unless the battery menu is open, and
/// the first open samples immediately so the CPU baseline starts without waiting a full interval.
/// Runs one battery-menu CPU tick: the sampler is called only when the gate says a sample is due.
/// Returns whether the published CPU sample changed, plus the next deadline to arm.
fn tick_power_cpu(
    menu_open: bool,
    next_due: Option<Instant>,
    now: Instant,
    sample: impl FnOnce() -> bool,
) -> (bool, Option<Instant>) {
    let (sample_due, next) = advance_power_cpu_sampling(menu_open, next_due, now);
    (sample_due && sample(), next)
}

fn advance_power_cpu_sampling(
    menu_open: bool,
    next_due: Option<Instant>,
    now: Instant,
) -> (bool, Option<Instant>) {
    if !menu_open {
        return (false, None);
    }
    match next_due {
        Some(at) if now < at => (false, Some(at)),
        _ => (true, Some(now + POWER_CPU_SAMPLE_INTERVAL)),
    }
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

/// Resolves the product settings file. Precedence: `FLAMEWM_SETTINGS_PATH`,
/// then `FLAMEWM_CONFIG_DIR/settings.toml`, then the XDG default
/// (`XDG_CONFIG_HOME` or `$HOME/.config`, `/flamewm/settings.toml`). The
/// last fallback never depends on the current working directory.
fn settings_path() -> (PathBuf, &'static str) {
    if let Some(path) = env::var_os("FLAMEWM_SETTINGS_PATH").filter(|value| !value.is_empty()) {
        return (PathBuf::from(path), "FLAMEWM_SETTINGS_PATH");
    }
    if let Some(dir) = env::var_os("FLAMEWM_CONFIG_DIR").filter(|value| !value.is_empty()) {
        return (PathBuf::from(dir).join("settings.toml"), "FLAMEWM_CONFIG_DIR");
    }
    let config_home = env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("/root/.config"));
    (config_home.join("flamewm").join("settings.toml"), "xdg-default")
}

#[cfg(test)]
mod settings_observer_tests {
    use super::*;

    #[test]
    fn publish_settings_snapshot_notifies_observer_with_same_snapshot() {
        let seen: Rc<RefCell<Vec<SettingsSnapshot>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&seen);
        let boxed: SettingsObserver = Box::new(move |snapshot: &SettingsSnapshot| {
            sink.borrow_mut().push(snapshot.clone());
        });
        let observer: SharedSettingsObserver = Rc::new(RefCell::new(Some(boxed)));
        let mut snapshot = SettingsSnapshot::new(7);
        snapshot
            .values
            .insert("appearance".to_owned(), SettingValue::Text("light".to_owned()));
        publish_settings_snapshot(&observer, &snapshot);
        assert_eq!(seen.borrow().as_slice(), std::slice::from_ref(&snapshot));
    }

    #[test]
    fn publish_settings_snapshot_without_observer_is_a_noop() {
        let observer: SharedSettingsObserver = Rc::new(RefCell::new(None));
        publish_settings_snapshot(&observer, &SettingsSnapshot::new(1));
    }

    fn readable() -> FdReadiness {
        FdReadiness {
            readable: true,
            ..FdReadiness::default()
        }
    }

    #[test]
    fn settings_wake_clean_drain_keeps_registration() {
        let mut drained = 0;
        let action = settings_wake_step(readable(), || {
            drained += 1;
            Ok(())
        });
        assert_eq!(action, FdAction::Continue);
        assert_eq!(drained, 1);
    }

    #[test]
    fn settings_wake_drain_error_removes_registration() {
        let action = settings_wake_step(readable(), || Err("drain failed".to_owned()));
        assert_eq!(action, FdAction::Remove);
    }

    #[test]
    fn settings_wake_hangup_or_error_readiness_removes_without_draining() {
        for readiness in [
            FdReadiness {
                error: true,
                ..FdReadiness::default()
            },
            FdReadiness {
                hangup: true,
                ..FdReadiness::default()
            },
        ] {
            let mut drained = false;
            let action = settings_wake_step(readiness, || {
                drained = true;
                Ok(())
            });
            assert_eq!(action, FdAction::Remove);
            assert!(!drained);
        }
    }
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

        /// Simulates a desktop change that bypasses activate_workspace, as a
        /// root-property change observed by the WM would. Bumps the revision.
        fn external_desktop_change(&mut self, index: usize) {
            self.workspaces.active_index = index;
            self.workspaces.revision += 1;
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
        fn activate_workspace(&mut self, index: usize, revision: u64) -> FlameResult<()> {
            if revision != self.workspaces.revision {
                return Err(flamewm_api::FlameError::stale("workspace revision changed"));
            }
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
            start_menu_toggle: false,
            media_audio_key: None,
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
    fn workspace_activation_rejects_revision_stale_after_external_desktop_change() {
        let mut engine = FakeEngine::new();
        let revision = engine.workspace_snapshot().expect("snapshot").revision;
        engine.external_desktop_change(1);
        let after = engine.workspace_snapshot().expect("snapshot");
        assert_ne!(after.revision, revision);
        let err = engine
            .activate_workspace(0, revision)
            .expect_err("stale revision must be rejected");
        assert_eq!(err.code, flamewm_api::ErrorCode::StaleRevision);
    }

    #[test]
    fn work_area_only_emits_nothing() {
        let mut host = FakeEngine::new().host();
        let only_area = WmChangeSet {
            windows: false,
            workspaces: false,
            panels: false,
            work_area: true,
            start_menu_toggle: false,
            media_audio_key: None,
        };
        let signals = collect_wm_signals(&mut host, only_area).expect("collect");
        assert!(signals.is_empty());
    }
}

#[cfg(test)]
mod power_cpu_sampling_tests {
    use super::*;

    #[test]
    fn closed_menu_never_arms_even_with_a_stale_deadline() {
        let t0 = Instant::now();
        let stale = Some(t0 + POWER_CPU_SAMPLE_INTERVAL);
        assert_eq!(advance_power_cpu_sampling(false, stale, t0), (false, None));
    }

    #[test]
    fn due_sample_while_open_calls_sampler_and_rearms() {
        let t0 = Instant::now();
        let mut calls = 0;
        let out = tick_power_cpu(true, Some(t0), t0, || {
            calls += 1;
            true
        });
        assert_eq!(calls, 1);
        assert_eq!(out, (true, Some(t0 + POWER_CPU_SAMPLE_INTERVAL)));
    }

    #[test]
    fn closed_menu_never_calls_sampler_and_never_arms() {
        let t0 = Instant::now();
        let mut calls = 0;
        let out = tick_power_cpu(false, Some(t0), t0, || {
            calls += 1;
            true
        });
        assert_eq!(calls, 0);
        assert_eq!(out, (false, None));
    }

    #[test]
    fn not_yet_due_open_menu_does_not_sample() {
        let t0 = Instant::now();
        let due = t0 + POWER_CPU_SAMPLE_INTERVAL;
        let out = tick_power_cpu(true, Some(due), t0, || panic!("sampled before due"));
        assert_eq!(out, (false, Some(due)));
    }

    #[test]
    fn first_open_samples_now_and_arms_one_second() {
        let t0 = Instant::now();
        assert_eq!(
            advance_power_cpu_sampling(true, None, t0),
            (true, Some(t0 + POWER_CPU_SAMPLE_INTERVAL))
        );
    }

    #[test]
    fn open_menu_waits_until_due_then_rearms() {
        let t0 = Instant::now();
        let due = t0 + POWER_CPU_SAMPLE_INTERVAL;
        let half = t0 + POWER_CPU_SAMPLE_INTERVAL / 2;
        assert_eq!(advance_power_cpu_sampling(true, Some(due), half), (false, Some(due)));
        assert_eq!(
            advance_power_cpu_sampling(true, Some(due), due),
            (true, Some(due + POWER_CPU_SAMPLE_INTERVAL))
        );
    }
}
