use super::*;

/// Tick: drain queued parent commands; OPEN closes the current helper
/// popup then projects/fits/shows/grabs; CLOSE closes; SHUTDOWN stops.
pub(crate) fn tick_host(
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
                guard.active_sequence = None;
            }
            Command::CloseSession(sequence) => {
                let mut guard = state.borrow_mut();
                if close_matches_active(guard.active_sequence, sequence) {
                    guard.surfaces.close(runtime);
                    guard.active_sequence = None;
                }
            }
            Command::Weather(feed) => {
                let mut guard = state.borrow_mut();
                guard.snapshot.apply_weather_feed(feed);
                if guard.surfaces.open == Some(QuickControlKind::Weather) {
                    guard.system_reprojection_pending = true;
                }
            }
            Command::Open(request) => {
                {
                    let mut guard = state.borrow_mut();
                    if !accept_scoped_open(&mut guard.latest_sequence, request.scope.as_ref()) {
                        send_open_result(scoped_result(request.scope.as_ref(), false, false));
                        continue;
                    }
                }
                // Live menu settings: re-read the current settings on every open (no timers).
                refresh_settings_for_open(state);
                let mut guard = state.borrow_mut();
                let guard = &mut *guard;
                // One helper popup at a time: close the current one first,
                // timing the real close turn for the slow-open line.
                let close_start = std::time::Instant::now();
                guard.surfaces.close(runtime);
                let close_elapsed = close_start.elapsed();
                if let Err(error) = open_request(
                    runtime,
                    &mut guard.surfaces,
                    &guard.snapshot,
                    &request,
                    close_elapsed,
                ) {
                    eprintln!("flamewm-quick-control: open refused: {error}");
                    send_open_result(scoped_result(
                        request.scope.as_ref(),
                        false,
                        error.rolled_back,
                    ));
                } else {
                    guard.active_sequence = request.scope.as_ref().map(|scope| scope.sequence);
                    send_open_result(scoped_result(request.scope.as_ref(), true, false));
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
        let changed = guard.snapshot.weather_search.poll();
        if changed && guard.surfaces.open == Some(QuickControlKind::Weather) {
            guard.system_reprojection_pending = true;
        }
    }
    let pending = {
        let mut guard = state.borrow_mut();
        if guard.system_reprojection_pending {
            guard.system_reprojection_pending = false;
            let open = guard.surfaces.open;
            open.and_then(|kind| {
                guard
                    .surfaces
                    .handle(kind)
                    .map(|surface| (surface, guard.snapshot.clone(), kind))
            })
        } else {
            None
        }
    };
    if let Some((surface, snapshot, kind)) = pending {
        let probe_start = std::time::Instant::now();
        project_snapshot(runtime, surface, &snapshot, kind).map_err(UiBackendError::Document)?;
        let projected = probe_start.elapsed();
        runtime.redraw(surface)?;
        if host_debug_enabled() {
            let ingress = R2_INGRESS.with(std::cell::Cell::take);
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
pub(crate) fn battery_menu_edge(
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

pub(crate) fn apply_system_snapshot(
    snapshot: &mut QuickControlSnapshot,
    system: flamewm_api::system::SystemSnapshot,
) -> bool {
    if snapshot.system == system {
        return false;
    }
    snapshot.system = system;
    true
}
