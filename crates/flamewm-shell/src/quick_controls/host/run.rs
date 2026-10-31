use super::*;

/// Run the quick-control host loop.
///
/// Connects its own control client, creates the three helper-owned popup
/// surfaces, then enters one reactor: X11 surface FD + nonblocking parent
/// command FD + timers/wake -> one event loop. Reuses flamewm-reactor.
/// The tick drains commands: OPEN closes the current helper popup,
/// projects, fits, shows/grabs; CLOSE closes; SHUTDOWN stops cleanly.
/// The surface event callback calls `route_helper_event`.
pub fn run_quick_control_host(packaged_root: std::path::PathBuf) -> Result<(), HostError> {
    let _ = PACKAGED_ROOT.set(packaged_root);
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
    let surfaces = HelperSurfaces::new();
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
        cursor_theme: crate::cursor_theme::CursorThemeSync::default(),
        latest_sequence: 0,
        active_sequence: None,
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
                for outcome in crate::control_actions::take_weather_pick_outcomes() {
                    finish_weather_pick(&mut outcome_state.borrow_mut(), outcome);
                }
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
                        #[allow(clippy::single_match_else)]
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
    let _stdin_file = stdin_holder;

    let event_state = std::rc::Rc::clone(&state);
    let tick_state = std::rc::Rc::clone(&state);
    let tick_input = Arc::clone(&input);
    let search_wake = reactor.wake_deadline();
    // Surfaces are created lazily on OPEN, so zero surfaces is the normal idle
    // state: keep running until shutdown was requested and surfaces are torn down.
    let keep_state = std::rc::Rc::clone(&state);
    let run_result = flamewm_ui_x11::run_surface_runtime_until(
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
            // The geocoding worker has no fd: keep the loop polling until its result lands.
            let searching = tick_state.borrow().snapshot.weather_search.in_flight();
            search_wake.set(
                searching.then(|| std::time::Instant::now() + crate::weather_host::WEATHER_POLL_INTERVAL),
            );
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
        move |runtime| !keep_state.borrow().shutdown || runtime.has_surfaces(),
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
