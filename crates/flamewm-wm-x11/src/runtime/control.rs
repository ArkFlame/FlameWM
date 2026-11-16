use super::*;

/// Shared session-bus control link. `None` while the bus is unreachable or after it dropped.
pub(super) type ControlSlot = Rc<RefCell<Option<Rc<ControlServer>>>>;

pub(super) const BUS_RETRY_MIN: std::time::Duration = std::time::Duration::from_secs(1);
pub(super) const BUS_RETRY_MAX: std::time::Duration = std::time::Duration::from_secs(30);

/// Capped exponential backoff for reconnecting an optional session-bus link.
pub(super) struct BusRetry {
    pub(super) next_at: Instant,
    pub(super) delay: std::time::Duration,
}

impl BusRetry {
    pub(super) fn new() -> Self {
        Self {
            next_at: Instant::now(),
            delay: BUS_RETRY_MIN,
        }
    }

    pub(super) fn due(&self, now: Instant) -> bool {
        now >= self.next_at
    }

    pub(super) fn reset(&mut self, now: Instant) {
        self.next_at = now;
        self.delay = BUS_RETRY_MIN;
    }

    pub(super) fn failed(&mut self, now: Instant) {
        self.next_at = now + self.delay;
        self.delay = (self.delay * 2).min(BUS_RETRY_MAX);
    }
}

pub(super) struct ControlLink {
    pub(super) slot: ControlSlot,
    pub(super) registration: Option<ControlRegistration>,
    pub(super) retry: BusRetry,
}

pub(super) struct ControlRegistration {
    pub(super) fd: i32,
    pub(super) events: FdEvents,
    pub(super) token: RegistrationToken,
}

/// One connect attempt. A failure is logged and scheduled, never fatal.
pub(super) fn connect_control_session(retry: &mut BusRetry) -> Option<Rc<ControlServer>> {
    let now = Instant::now();
    match ControlServer::connect_session() {
        Ok(server) => {
            retry.reset(now);
            Some(Rc::new(server))
        }
        Err(error) => {
            retry.failed(now);
            eprintln!(
                "flamewm: control D-Bus unavailable, windows and keys stay active, retrying: {error}"
            );
            None
        }
    }
}

/// Emits one control signal when the bus is connected. A failed write drops the link so the
/// backoff reconnects it; it never stops the WM.
pub(super) fn emit_control(control: &ControlSlot, signal: &ControlSignal) {
    let server = control.borrow().clone();
    let Some(server) = server else {
        return;
    };
    if let Err(error) = server.emit_signal(signal) {
        eprintln!("flamewm: control D-Bus emit failed, reconnecting: {error}");
        *control.borrow_mut() = None;
    }
}

pub(super) fn register_control(
    reactor: &Rc<RefCell<Reactor>>,
    server: &Rc<ControlServer>,
    slot: &ControlSlot,
    host: &Rc<RefCell<Option<PlatformHost<X11Desktop>>>>,
    now_ms: &Rc<Cell<u64>>,
    control_error: &Rc<RefCell<Option<AnyError>>>,
    settings_observer: &SharedSettingsObserver,
) -> Result<ControlRegistration, AnyError> {
    let watch = server.watch();
    let server = Rc::clone(server);
    let slot = Rc::clone(slot);
    let host = Rc::clone(host);
    let now_ms = Rc::clone(now_ms);
    let control_error = Rc::clone(control_error);
    let settings_observer = Rc::clone(settings_observer);
    let token = reactor.borrow_mut().register_raw_fd_with_action(
        watch.fd,
        interest_for(watch.events),
        move |_, readiness| {
            if readiness.error || readiness.hangup {
                eprintln!("flamewm: control D-Bus connection closed, retrying");
                *slot.borrow_mut() = None;
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
            if let Err(error) = server.on_ready(host, now_ms.get()) {
                keep_first_error(&control_error, Box::new(error));
                FdAction::Remove
            } else {
                publish_settings_snapshot(&settings_observer, &host.settings_snapshot());
                // Displays first so the shell's display snapshot is current before panel geometry lands.
                let mut signals = Vec::new();
                let displays = host.cached_display_snapshot();
                if displays != displays_before
                    && let Some(snapshot) = displays
                {
                    signals.push(ControlSignal::DisplaysSnapshotChanged { snapshot });
                }
                let panels = host.panels_snapshot();
                if panels != panels_before {
                    signals.push(ControlSignal::PanelsSnapshotChanged { snapshot: panels });
                }
                for signal in &signals {
                    emit_control(&slot, signal);
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

/// Keeps the control link current each turn: drops a stale registration, reconnects a missing
/// link once its backoff is due, and re-registers when the bus fd changes.
pub(super) fn refresh_control(
    reactor: &Rc<RefCell<Reactor>>,
    link: &mut ControlLink,
    host: &Rc<RefCell<Option<PlatformHost<X11Desktop>>>>,
    now_ms: &Rc<Cell<u64>>,
    control_error: &Rc<RefCell<Option<AnyError>>>,
    settings_observer: &SharedSettingsObserver,
) -> Result<(), AnyError> {
    let mut server = link.slot.borrow().clone();
    if server.is_none() {
        if let Some(registration) = link.registration.take() {
            // Already removed by the reactor when the bus closed; a stale token is ignored.
            let _ = reactor.borrow_mut().remove(registration.token);
        }
        if !link.retry.due(Instant::now()) {
            return Ok(());
        }
        server = connect_control_session(&mut link.retry);
        if server.is_some() {
            eprintln!("flamewm: control D-Bus connected");
        }
        (*link.slot.borrow_mut()).clone_from(&server);
    }
    let Some(server) = server else {
        return Ok(());
    };
    let watch = server.watch();
    if let Some(registration) = link.registration.take() {
        if watch.fd == registration.fd && watch.events == registration.events {
            link.registration = Some(registration);
            return Ok(());
        }
        reactor.borrow_mut().remove(registration.token)?;
    }
    link.registration = Some(register_control(
        reactor,
        &server,
        &link.slot,
        host,
        now_ms,
        control_error,
        settings_observer,
    )?);
    Ok(())
}
