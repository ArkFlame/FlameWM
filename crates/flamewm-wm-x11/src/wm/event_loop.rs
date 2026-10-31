//! Wm event pump drain, wait timeout, and the run loop with hook.
use super::*;

/// Wait timeout for the reactor: `None` (block until an event) when no display
/// transaction is pending, otherwise the time left until its revert deadline.
pub(crate) fn next_wait_timeout(
    now_ms: u64,
    deadline_ms: Option<u64>,
) -> Option<std::time::Duration> {
    deadline_ms.map(|deadline| std::time::Duration::from_millis(deadline.saturating_sub(now_ms)))
}

/// Drains buffered X events through the coalescing pump, dispatching each one,
/// then records the batch size. Callers own the commit boundary after it.
pub(super) fn drain_pumped_events(
    wm: &mut Wm<'_, RustConnection>,
    pump: &mut WmEventPump,
    conn: &RustConnection,
) -> Result<(), AnyError> {
    let mut batch: usize = 0;
    while let Some(pumped) = pump.next(conn).map_err(any_conn_err)? {
        resize_counters().record_received(pumped.received as u64);
        resize_counters().record_coalesced(pumped.coalesced as u64);
        batch += 1;
        {
            let _guard = flamewm_profiler::start("wm.resize.motion.drain");
            wm.probe_event_dispatch(&pumped.event, pumped.received);
            wm.handle_event(pumped.event)?;
        }
    }
    resize_counters().record_committed(batch as u64);
    if batch > resize_counters().max_batch() {
        resize_counters().set_max_batch(batch);
    }
    Ok(())
}

pub(crate) fn run_with_hook<F>(
    config: WmConfig,
    catalog: &std::sync::Arc<flamewm_applications::ApplicationCatalog>,
    reactor: &Rc<RefCell<flamewm_reactor::Reactor>>,
    icon_service: Rc<RefCell<flamewm_integrations_linux::icon_service::IconService>>,
    mut hook: F,
) -> Result<(), AnyError>
where
    F: FnMut(
        &mut Wm<'_, RustConnection>,
        &Rc<RustConnection>,
        usize,
        WmChangeSet,
    ) -> Result<Option<std::time::Duration>, AnyError>,
{
    use std::os::fd::AsRawFd;
    let (conn, screen_num) = x11rb::connect(None)?;
    let conn = Rc::new(conn);
    let screen = &conn.setup().roots[screen_num];
    become_wm(conn.as_ref(), screen)?;

    let mut wm = Wm::new(
        conn.as_ref(),
        screen_num,
        config,
        std::sync::Arc::clone(catalog),
        icon_service,
    )?;
    wm.publish_desktop_state()?;
    wm.scan_existing()?;
    if let Err(error) = wm.select_raw_keys() {
        eprintln!("flamewm: raw escape-key feed unavailable: {error}");
    }
    conn.flush()?;

    // X fd source: borrowed fd wakes dispatch; buffered drain happens below.
    // The connection outlives the registration; calloop never owns the fd.
    let x_fd = conn.stream().as_raw_fd();
    let _x_token = reactor.borrow_mut().register_raw_fd_with_action(
        x_fd,
        calloop::Interest::READ,
        |_, _| flamewm_reactor::FdAction::Continue,
    )?;
    loop {
        // Buffered drain before block: one pump collapses contiguous
        // same event+child MotionNotify; order vs Button/Key/Configure/
        // Property/Enter/Leave/ClientMessage/Expose is preserved.
        let mut pump = WmEventPump::new();
        drain_pumped_events(&mut wm, &mut pump, conn.as_ref())?;
        // Batch boundary: a bare-Super tap completes only after its autorepeat
        // partner (same timestamp) had the chance to cancel it.
        wm.commit_super_tap();
        wm.tick_edge_drag()?;
        // Drain once per reactor turn: semantic owners marked facets above;
        // the hook flushes them to snapshots/signals exactly once.
        let changes = wm.take_changes();
        wm.pump_transitions(std::time::Instant::now())?;
        wm.flush_deferred_chrome(std::time::Instant::now())?;
        let wait = hook(&mut wm, &conn, screen_num, changes)?;
        drain_pumped_events(&mut wm, &mut pump, conn.as_ref())?;
        wm.commit_super_tap();
        wm.tick_edge_drag()?;
        // After the drain: an effect begun by a drained event (e.g. try_start_open) must cap the wait.
        let wait = wm.transition_wait(wait);
        let wait = wm.deferred_chrome_wait(wait);
        let wait = drag_edge_wait(
            wm.edge_drag,
            wm.mouse_workspace_cooldowns,
            std::time::Instant::now(),
            wait,
        );
        emit_resize_summary();
        emit_move_summary();
        // Flush once per reactor turn.
        conn.flush()?;
        // Blocking wait: woken by X, provider, audio, or profiler sources.
        reactor.borrow_mut().dispatch(wait)?;
        // The control reply is sent by the reactor callback before it returns.
        // Exit successfully on the following turn so the session supervisor
        // can stop and reap its managed dependents.
        if flamewm_x11_desktop::take_session_logout_request() {
            return Ok(());
        }
        wm.drain_catalog_icon_results();
    }
}
