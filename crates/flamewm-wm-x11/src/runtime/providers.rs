use super::*;

pub(super) struct ProviderRegistration {
    source: LinuxIntegrationSource,
    fd: i32,
    events: FdEvents,
    token: RegistrationToken,
}

/// Once startup (catalog, icon index, first frames) has settled, its freed heap is handed back.
pub(super) const STARTUP_HEAP_TRIM_DELAY: std::time::Duration = std::time::Duration::from_secs(10);

/// CPU sample cadence while the battery menu is open.
pub(super) const POWER_CPU_SAMPLE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// Returns `(sample_now, next_due)`. The deadline is `None` unless the battery menu is open, and
/// the first open samples immediately so the CPU baseline starts without waiting a full interval.
/// Runs one battery-menu CPU tick: the sampler is called only when the gate says a sample is due.
/// Returns whether the published CPU sample changed, plus the next deadline to arm.
pub(super) fn tick_power_cpu(
    menu_open: bool,
    next_due: Option<Instant>,
    now: Instant,
    sample: impl FnOnce() -> bool,
) -> (bool, Option<Instant>) {
    let (sample_due, next) = advance_power_cpu_sampling(menu_open, next_due, now);
    (sample_due && sample(), next)
}

pub(super) fn advance_power_cpu_sampling(
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

/// Whether the armed CPU-sample deadline moved. The hook writes the reactor wake slot only when
/// this is true, so a turn that leaves the menu closed (or between two samples) does not dirty the
/// reactor's earliest-wake cache.
#[must_use]
pub(super) fn cpu_deadline_moved(previous: Option<Instant>, next: Option<Instant>) -> bool {
    previous != next
}

pub(super) fn register_provider(
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
                keep_first_error(&provider_error, error);
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

pub(super) fn refresh_provider(
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

pub(super) fn interest_for(events: FdEvents) -> Interest {
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
pub(super) fn profile_report_interval() -> std::time::Duration {
    std::time::Duration::from_secs(flamewm_profiler::profile_interval_secs())
}

#[cfg(test)]
mod deadline_tests {
    use super::*;

    /// A closed menu and an interval between two samples leave the deadline untouched, so the
    /// reactor slot is not rewritten on those turns.
    #[test]
    fn the_reactor_deadline_is_only_rewritten_when_it_moves() {
        let t0 = Instant::now();
        let (sample, closed) = advance_power_cpu_sampling(false, None, t0);
        assert!(!sample);
        assert!(!cpu_deadline_moved(None, closed));

        let (sample, open) = advance_power_cpu_sampling(true, None, t0);
        assert!(sample);
        assert!(cpu_deadline_moved(None, open));

        let half = t0 + POWER_CPU_SAMPLE_INTERVAL / 2;
        let (sample, unchanged) = advance_power_cpu_sampling(true, open, half);
        assert!(!sample);
        assert!(!cpu_deadline_moved(open, unchanged));

        let (sample, ticked) =
            advance_power_cpu_sampling(true, open, t0 + POWER_CPU_SAMPLE_INTERVAL);
        assert!(sample);
        assert!(cpu_deadline_moved(open, ticked));
    }
}
