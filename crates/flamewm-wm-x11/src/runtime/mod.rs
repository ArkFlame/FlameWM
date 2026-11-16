use std::cell::{Cell, RefCell};
use std::env;
use std::io::{self, Read, Write};
use std::os::fd::RawFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
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
    LinuxIntegrationRuntime, LinuxIntegrationSource, audio_helper::AudioHelper,
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

/// Wake source for a settings owner (for example the `XSettings` manager). `fd`
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
            settings_wake_step(readiness, &mut on_readable)
        })
        .map_err(Into::into)
}

fn run_session(
    config: WmConfig,
    settings_observer: Option<SettingsObserver>,
    settings_wake: Option<SettingsWake>,
) -> Result<(), AnyError> {
    // One malloc arena and the mmap threshold, pinned before the WM starts its workers.
    flamewm_render_x11::return_large_frees_to_system();
    flamewm_profiler::init_process("flamewm-wm");
    flamewm_debug::init_process("wm");
    let (settings_path, settings_source) = settings_path();
    let config = config.with_persisted_workspaces(&settings_path);
    eprintln!(
        "flamewm-wm: settings path={} source={settings_source}",
        settings_path.display()
    );
    // The session bus is optional at start: windows and keys do not depend on it. A missing bus
    // is retried lazily with capped backoff from the hook (see `refresh_control`).
    let mut control_retry = BusRetry::new();
    let control: ControlSlot = Rc::new(RefCell::new(connect_control_session(&mut control_retry)));
    let reactor = Rc::new(RefCell::new(Reactor::new()?));
    let integrations = Rc::new(RefCell::new(LinuxIntegrationRuntime::connect()?));
    if let Err(error) = integrations.borrow_mut().start() {
        integrations.borrow_mut().stop();
        return Err(error.into());
    }
    let provider_error = Rc::new(RefCell::new(None));
    let register = |source| {
        register_provider(&reactor, &integrations, &provider_error, source)
            .inspect_err(|_| integrations.borrow_mut().stop())
    };
    let mut network_registration = register(LinuxIntegrationSource::NetworkManager)?;
    let mut mpris_registration = register(LinuxIntegrationSource::Mpris)?;
    let mut power_registration = register(LinuxIntegrationSource::Power)?;
    let mut power_profiles_registration = register(LinuxIntegrationSource::PowerProfiles)?;
    let _bluetooth_registration = register(LinuxIntegrationSource::Bluetooth)?;
    let (audio_sender, audio_receiver) = mpsc::sync_channel::<AudioSnapshot>(32);
    let (audio_reader, mut audio_writer) = UnixStream::pair()
        .and_then(|(reader, writer)| {
            reader.set_nonblocking(true)?;
            writer.set_nonblocking(true)?;
            Ok((reader, writer))
        })
        .inspect_err(|_| integrations.borrow_mut().stop())?;
    let pulse = Rc::new(
        AudioHelper::connect(move |snapshot| {
            if audio_sender.try_send(snapshot).is_ok() {
                let _ = audio_writer.write(&[1]);
            }
        })
        .inspect_err(|_| integrations.borrow_mut().stop())?,
    );
    // Best effort: the one eager spawn. It starts the helper so the first volume key steps from a
    // real snapshot. The helper exits after its idle period; the last snapshot stays until next use.
    let _ = pulse.request_snapshot();
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
            while let Ok(snapshot) = audio_receiver.try_recv() {
                updates.push(snapshot);
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
    let initial_control = control.borrow().clone();
    let control_registration = match initial_control {
        Some(server) => match register_control(
            &reactor,
            &server,
            &control,
            &host,
            &control_now_ms,
            &control_error,
            &settings_observer,
        ) {
            Ok(registration) => Some(registration),
            Err(error) => {
                drop(pulse);
                integrations.borrow_mut().stop();
                return Err(error);
            }
        },
        None => None,
    };
    let mut control_link = ControlLink {
        slot: Rc::clone(&control),
        registration: control_registration,
        retry: control_retry,
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
                    integrations.borrow_mut().stop();
                    AnyError::from(io::Error::other(error.to_string()))
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
    let Some(icon_fd) = icon_service.borrow().wake_fd() else {
        return Err(Box::new(io::Error::other(
            "IconService wake FD unavailable",
        )));
    };
    let icon_service_for_wake = Rc::clone(&icon_service);
    let _icon_registration = reactor.borrow_mut().register_raw_fd_with_action(
        icon_fd,
        Interest::READ,
        move |_, _| {
            icon_service_for_wake.borrow_mut().wake_drain();
            FdAction::Continue
        },
    )?;
    // Titlebar backdrop canvas readiness: a worker writes one byte per stored canvas, and the
    // reactor turn ends so the WM repaints the chrome that painted flat while it built.
    let (backdrop_reader, backdrop_writer) = UnixStream::pair()?;
    backdrop_reader.set_nonblocking(true)?;
    backdrop_writer.set_nonblocking(true)?;
    crate::frame::titlebar_backdrop::set_canvas_wake(backdrop_writer);
    let _backdrop_wake_registration = reactor.borrow_mut().register_fd_with_source_action(
        backdrop_reader,
        Interest::READ,
        move |_, source, _| {
            let mut wake_buffer = [0_u8; 64];
            loop {
                match source.read(&mut wake_buffer) {
                    Ok(0) => return FdAction::Remove,
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(_) => return FdAction::Remove,
                }
            }
            FdAction::Continue
        },
    )?;
    let _heap_trim_registration = reactor
        .borrow_mut()
        .register_timer(
            STARTUP_HEAP_TRIM_DELAY,
            false,
            flamewm_render_x11::release_free_heap,
        )
        .map_err(|error| AnyError::from(io::Error::other(error.to_string())))?;
    // Settings owner wake: SelectionClear on the manager connection is observed
    // without waiting for the next publish. A failed registration leaves the WM
    // running without live XSettings wake, like a refused claim.
    let _settings_wake_registration = settings_wake.and_then(|wake| {
        register_settings_wake(&reactor, wake)
            .inspect_err(|error| eprintln!("flamewm: XSettings wake not registered: {error}"))
            .ok()
    });
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

    let result = run_with_hook(
        config,
        &catalog,
        &reactor,
        Rc::clone(&icon_service),
        move |wm, conn, screen, changes| {
            if let Some(error) = control_error_for_hook.borrow_mut().take() {
                return Err(error);
            }
            wm.apply_decoration_tokens(crate::chrome::live_chrome_tokens())?;
            wm.apply_transition_settings(crate::animation::live_settings())?;
            let root = conn.setup().roots[screen].root;
            idle_lock_for_hook
                .borrow_mut()
                .turn(&reactor_for_hook, conn.as_ref(), root);
            // Blue light filter: a no-op unless the committed intensity differs from the last
            // upload, so this costs one thread-local read per turn.
            if let Err(error) =
                crate::blue_light::apply(conn.as_ref(), root, crate::blue_light::config())
            {
                eprintln!("flamewm: blue light filter update failed: {error}");
            }
            // Magnifier output: a no-op unless the runtime level differs from the last send.
            if let Err(error) =
                crate::zoom_transform::apply(conn.as_ref(), root, crate::zoom::level())
            {
                eprintln!("flamewm: zoom output update failed: {error}");
            }
            refresh_control(
                &reactor_for_hook,
                &mut control_link,
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
                        wm.unresponsive_clients(),
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
                        | SystemAction::SetBrightness(_)
                        | SystemAction::SetPowerProfile(_)
                        | SystemAction::BluetoothDevice(_)
                        | SystemAction::VpnConnection(_)
                        | SystemAction::BatteryMenuOpened
                        | SystemAction::BatteryMenuClosed => integrations
                            .borrow_mut()
                            .perform_action(action, &snapshot.network, &snapshot.media),
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
                        emit_system_signals(host, &control_for_hook);
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
                    emit_system_signals(host, &control_for_hook);
                }
                let power = integrations.power_snapshot();
                if host.system_mut().update_power(power) {
                    emit_system_signals(host, &control_for_hook);
                }
                let power_profiles = integrations.power_profiles_snapshot();
                if host.system_mut().update_power_profiles(power_profiles) {
                    emit_system_signals(host, &control_for_hook);
                }
                let bluetooth = integrations.bluetooth_snapshot();
                if host.system_mut().update_bluetooth(bluetooth) {
                    emit_system_signals(host, &control_for_hook);
                }
                let brightness = integrations.brightness_snapshot();
                if host.system_mut().update_brightness(brightness) {
                    emit_system_signals(host, &control_for_hook);
                }
                if reconciled {
                    let media = integrations.media_snapshot();
                    if host.system_mut().update_media(media) {
                        emit_system_signals(host, &control_for_hook);
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
            refresh_provider(
                &reactor_for_hook,
                &mut power_profiles_registration,
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
            if changed
                && host
                    .system_mut()
                    .update_power(integrations_for_hook.borrow().power_snapshot())
            {
                emit_system_signals(host, &control_for_hook);
            }
            let now_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
            control_now_ms_for_hook.set(now_ms);
            // Capture the old state BEFORE processing RandR events. Previously the
            // "before" snapshot was taken after refresh, so monitor rearrangements
            // compared equal and emitted neither the display nor panel update.
            let display_before = host.cached_display_snapshot();
            let panels_before = host.panels_snapshot();
            // RandR hotplug/mode notifications mark the topology dirty.
            // While a fullscreen client holds a temporary mode, the desktop stays at its recorded
            // geometry: skip the snapshot refresh but still consume the topology flag.
            let desktop_held = wm.observe_desktop_mode();
            if wm.take_display_topology_dirty() && !desktop_held {
                // A new crtc set must receive the filter again: the cached crtc ids are gone.
                crate::blue_light::invalidate();
                crate::zoom_transform::invalidate();
                match host.display_snapshot() {
                    Ok(displays) => {
                        if let Err(error) = host.resync_panels_for_displays(&displays) {
                            eprintln!("flamewm: panel resync after display change failed: {error}");
                        }
                    }
                    Err(error) => eprintln!("flamewm: display topology refresh failed: {error}"),
                }
            }
            // The transaction is retired before any native call, so a failed or disconnected-output
            // revert is reported once here and never retried; it must not abort the WM loop.
            if let Err(error) = host.tick_display_transactions(now_ms) {
                eprintln!("flamewm: display mode transaction: {error}");
            }
            let display_after = host.cached_display_snapshot();
            if display_after != display_before
                && let Some(snapshot) = display_after
            {
                // A timeout-driven mode revert can change geometry without an
                // already-consumed RandR dirty event. Reconcile panels before
                // publishing either snapshot and before recomputing WM struts.
                if let Err(error) = host.resync_panels_for_displays(&snapshot) {
                    eprintln!("flamewm: panel resync after mode transaction failed: {error}");
                }
                emit_control(
                    &control_for_hook,
                    &ControlSignal::DisplaysSnapshotChanged { snapshot },
                );
            }
            // The panel service is changed by resync_panels_for_displays(), not
            // by WmChangeSet. Emit its authoritative snapshot explicitly, including
            // when the only change was an XRandR output-coordinate swap.
            let panels_after = host.panels_snapshot();
            if panels_after != panels_before {
                emit_control(
                    &control_for_hook,
                    &ControlSignal::PanelsSnapshotChanged {
                        snapshot: panels_after,
                    },
                );
            }
            // Alt+Tab previews: at most one capture per turn, and only while the switcher is open.
            wm.pump_switcher_thumbs();
            // Alt+Tab overlay snapshot: queued by the switcher on each change, emitted once here.
            if let Some(signal) = wm.take_switcher_signal() {
                emit_control(&control_for_hook, &signal);
            }
            flush_wm_changes(host, &control_for_hook, &integrations_for_hook, changes)?;
            let areas = host.output_work_areas();
            let mut wait =
                crate::wm::next_wait_timeout(now_ms, host.display_transaction_deadline_ms());
            // A scheduled blue-light filter flips on the clock, not on an event: arm the boundary as
            // a wake and keep whichever deadline is nearer.
            if let Some(delay) = crate::blue_light::next_refresh_delay(
                crate::blue_light::config(),
                crate::blue_light::now_minutes(),
            ) {
                wait = Some(wait.map_or(delay, |current| current.min(delay)));
            }
            // An offline MPRIS bus is retried on backoff; wake at that deadline with no polling.
            if let Some(delay) = integrations_for_hook.borrow().next_retry_in(Instant::now()) {
                wait = Some(wait.map_or(delay, |current| current.min(delay)));
            }
            // Armed only by a close request: the one ping deadline, never a periodic timer.
            if let Some(delay) = wm.ping_wait_timeout(std::time::Instant::now()) {
                wait = Some(wait.map_or(delay, |current| current.min(delay)));
            }
            // Queued switcher captures run on the next turn, not on the next input event.
            if wm.switcher_thumbs_pending() {
                let delay = std::time::Duration::from_millis(1);
                wait = Some(wait.map_or(delay, |current| current.min(delay)));
            }
            drop(host_guard);
            wm.set_output_work_areas(areas)?;
            // Wake the reactor at the pending display revert deadline; None when nothing is pending.
            Ok(wait)
        },
    );
    drop(icon_service);
    drop(pulse);
    integrations.borrow_mut().stop();
    result
}

fn emit_system_signals<E>(host: &PlatformHost<E>, control: &ControlSlot)
where
    E: flamewm_api::ports::EnginePorts,
{
    let snapshot = host.system_snapshot();
    emit_control(
        control,
        &ControlSignal::SystemSnapshotChanged {
            snapshot: snapshot.clone(),
        },
    );
    emit_control(
        control,
        &ControlSignal::SystemChanged {
            revision: snapshot.revision,
        },
    );
}

/// Persisted `cursorTheme` from the settings file at startup, before the session
/// environment is applied. `None` when the file is missing or unparsable.
#[must_use]
fn flush_wm_changes<E>(
    host: &mut PlatformHost<E>,
    control: &ControlSlot,
    integrations: &RefCell<LinuxIntegrationRuntime>,
    mut changes: WmChangeSet,
) -> Result<(), AnyError>
where
    E: flamewm_api::ports::EnginePorts,
{
    // Take the one-shot start-menu request out of the drained set so it is emitted
    // exactly once for this turn and never carried into a later flush.
    let start_menu_toggle = std::mem::take(&mut changes.start_menu_toggle);
    let launcher_toggle = std::mem::take(&mut changes.launcher_toggle);
    let dock_auto_hide_toggle = std::mem::take(&mut changes.dock_auto_hide_toggle);
    let media_audio_key = changes.media_audio_key.take();
    let media_brightness_key = changes.media_brightness_key.take();
    for signal in collect_wm_signals(host, changes).map_err(any_from_flame)? {
        emit_control(control, &signal);
    }
    if start_menu_toggle {
        emit_control(control, &ControlSignal::StartMenuToggleRequested);
    }
    if launcher_toggle {
        emit_control(control, &ControlSignal::LauncherToggleRequested);
    }
    if dock_auto_hide_toggle && let Err(error) = host.toggle_dock_auto_hide() {
        eprintln!("flamewm: dock auto-hide toggle was not applied: {error:?}");
    }
    if let Some(key) = media_audio_key {
        // A rejected write (stale generation, no sink, Pulse down) must not stop the WM.
        if let Err(error) = host.media_audio_key(key, crate::live_settings::volume_cap_percent()) {
            eprintln!("flamewm: media key {key:?} was not applied: {error:?}");
        }
    }
    if let Some(key) = media_brightness_key {
        match host.media_brightness_key(key) {
            // Publish the observed backlight in the same turn so the brightness OSD shows now.
            Ok(()) => {
                let brightness = integrations.borrow_mut().brightness_snapshot();
                if host.system_mut().update_brightness(brightness) {
                    emit_system_signals(host, control);
                }
            }
            Err(error) => eprintln!("flamewm: brightness key {key:?} was not applied: {error:?}"),
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
        let occupancy_before = host.workspace_snapshot()?.occupancy;
        if host.refresh_windows()? {
            let windows = host.engine().snapshot()?;
            let revision = windows
                .iter()
                .map(|window| window.state_generation)
                .max()
                .unwrap_or(0);
            signals.push(ControlSignal::WindowsSnapshotChanged { revision, windows });
            signals.push(ControlSignal::WindowsChanged { revision });
            // Occupancy is window state, so the pager only hears about it when a count moves.
            let workspaces = host.workspace_snapshot()?;
            if workspaces.occupancy != occupancy_before {
                signals.push(ControlSignal::WorkspacesSnapshotChanged {
                    snapshot: workspaces,
                });
            }
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

/// Records `error` in `slot` unless an earlier error in this turn is already kept.
fn keep_first_error<T>(slot: &RefCell<Option<T>>, error: T) {
    let mut slot = slot.borrow_mut();
    if slot.is_none() {
        *slot = Some(error);
    }
}

fn any_from_flame(error: flamewm_api::FlameError) -> AnyError {
    Box::new(std::io::Error::other(error.to_string()))
}
mod control;
mod providers;
mod settings;

use control::*;
use providers::*;
pub(crate) use settings::settings_path;
use settings::*;
pub use settings::{persisted_appearance, persisted_cursor_theme};

#[cfg(test)]
mod bus_retry_tests;
#[cfg(test)]
mod power_cpu_sampling_tests;
#[cfg(test)]
mod settings_observer_tests;
#[cfg(test)]
mod tests;
