use super::*;

use std::cell::Cell;
use std::os::fd::AsRawFd;

pub(super) const SESSION: &str = "/org/freedesktop/login1/session/_32";
pub(super) const SEAT: &str = "/org/freedesktop/login1/seat/seat0";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Call {
    GetSession(String),
    GetSessionByPid(u32),
    Class,
    Active,
    TakeControl,
    ReleaseControl,
    Activate,
    SwitchTo(u32),
    PauseDeviceComplete(u32, u32),
    Subscribe,
}

/// Records every call and can inject the signals logind would send.
pub(super) struct FakeBus {
    pub(super) session: RefCell<Option<String>>,
    pub(super) by_pid: RefCell<Option<String>>,
    pub(super) class: RefCell<String>,
    pub(super) active: Cell<bool>,
    /// Whether `org.freedesktop.login1.Session.Activate` makes the fake report the session
    /// active. `false` models a session logind refuses to activate (the fail-closed case).
    pub(super) activates: Cell<bool>,
    /// Read counter at which `Active` starts reporting true, for the activation wait.
    pub(super) active_after: Cell<Option<usize>>,
    pub(super) active_reads: Cell<usize>,
    pub(super) take_device_paused: Cell<bool>,
    pub(super) seat: RefCell<String>,
    pub(super) calls: RefCell<Vec<Call>>,
    pub(super) queue: RefCell<Option<EventQueue>>,
}

impl FakeBus {
    pub(super) fn new() -> Self {
        Self {
            session: RefCell::new(Some(SESSION.to_owned())),
            by_pid: RefCell::new(Some(SESSION.to_owned())),
            class: RefCell::new("user".to_owned()),
            active: Cell::new(true),
            activates: Cell::new(true),
            active_after: Cell::new(None),
            active_reads: Cell::new(0),
            take_device_paused: Cell::new(false),
            seat: RefCell::new("seat0".to_owned()),
            calls: RefCell::new(Vec::new()),
            queue: RefCell::new(None),
        }
    }

    pub(super) fn record(&self, call: Call) {
        self.calls.borrow_mut().push(call);
    }

    pub(super) fn calls(&self) -> Vec<Call> {
        self.calls.borrow().clone()
    }

    pub(super) fn count(&self, wanted: &Call) -> usize {
        self.calls
            .borrow()
            .iter()
            .filter(|call| *call == wanted)
            .count()
    }

    pub(super) fn pause_acks(&self) -> Vec<(u32, u32)> {
        self.calls
            .borrow()
            .iter()
            .filter_map(|call| match call {
                Call::PauseDeviceComplete(major, minor) => Some((*major, *minor)),
                _ => None,
            })
            .collect()
    }

    pub(super) fn signal(&self, event: Event) {
        self.queue
            .borrow()
            .as_ref()
            .expect("session subscribed first")
            .push(event);
    }

    pub(super) fn signal_pause(&self, major: u32, minor: u32, kind: PauseKind) {
        self.signal(Event::Pause { major, minor, kind });
    }
}

impl Bus for FakeBus {
    fn get_session(&self, id: &str) -> Result<String, LogindError> {
        self.record(Call::GetSession(id.to_owned()));
        self.session
            .borrow()
            .clone()
            .ok_or_else(|| LogindError(format!("GetSession({id}) unknown")))
    }

    fn get_session_by_pid(&self, pid: u32) -> Result<String, LogindError> {
        self.record(Call::GetSessionByPid(pid));
        self.by_pid
            .borrow()
            .clone()
            .ok_or_else(|| LogindError(format!("GetSessionByPID({pid}) unknown")))
    }

    fn session_seat(&self, _session_path: &str) -> Result<(String, String), LogindError> {
        Ok((self.seat.borrow().clone(), SEAT.to_owned()))
    }

    fn session_class(&self, _session_path: &str) -> Result<String, LogindError> {
        self.record(Call::Class);
        Ok(self.class.borrow().clone())
    }

    fn session_active(&self, _session_path: &str) -> Result<bool, LogindError> {
        self.record(Call::Active);
        let reads = self.active_reads.get() + 1;
        self.active_reads.set(reads);
        Ok(match self.active_after.get() {
            Some(after) => self.active.get() || reads > after,
            None => self.active.get(),
        })
    }

    fn take_control(&self, _session_path: &str) -> Result<(), LogindError> {
        self.record(Call::TakeControl);
        Ok(())
    }

    fn release_control(&self, _session_path: &str) -> Result<(), LogindError> {
        self.record(Call::ReleaseControl);
        Ok(())
    }

    fn activate_session(&self, _session_path: &str) -> Result<(), LogindError> {
        self.record(Call::Activate);
        if self.activates.get() {
            self.active.set(true);
        }
        Ok(())
    }

    fn pause_device_complete(
        &self,
        _session_path: &str,
        major: u32,
        minor: u32,
    ) -> Result<(), LogindError> {
        self.record(Call::PauseDeviceComplete(major, minor));
        Ok(())
    }

    fn take_device(
        &self,
        _session_path: &str,
        _major: u32,
        _minor: u32,
    ) -> Result<(OwnedFd, bool), LogindError> {
        let fd = File::open("/dev/null").map_err(|error| LogindError(format!("{error}")))?;
        Ok((OwnedFd::from(fd), self.take_device_paused.get()))
    }

    fn release_device(
        &self,
        _session_path: &str,
        _major: u32,
        _minor: u32,
    ) -> Result<(), LogindError> {
        Ok(())
    }

    fn switch_to(&self, _seat_path: &str, vt: u32) -> Result<(), LogindError> {
        self.record(Call::SwitchTo(vt));
        Ok(())
    }

    fn subscribe(&self, _session_path: &str, queue: EventQueue) -> Result<(), LogindError> {
        self.record(Call::Subscribe);
        *self.queue.borrow_mut() = Some(queue);
        Ok(())
    }

    fn pump(&self) {}
}

pub(super) fn session() -> (LogindSession, Rc<FakeBus>) {
    let fake = Rc::new(FakeBus::new());
    let bus: Rc<dyn Bus> = fake.clone();
    let session =
        LogindSession::open_with_bus(bus, Some("5"), Some("seat0")).expect("user session");
    (session, fake)
}

#[test]
pub(super) fn active_user_session_takes_control_and_subscribes() {
    let (session, fake) = session();
    assert_eq!(session.path(), SESSION);
    assert_eq!(fake.count(&Call::TakeControl), 1);
    assert_eq!(fake.count(&Call::Subscribe), 1);
    assert!(session.is_active_flag());
}

#[test]
pub(super) fn session_lookup_falls_back_to_get_session_by_pid() {
    let fake = Rc::new(FakeBus::new());
    *fake.session.borrow_mut() = None; // XDG_SESSION_ID set but logind rejects it
    let bus: Rc<dyn Bus> = fake.clone();
    let session = LogindSession::open_with_bus(bus, Some("5"), None).expect("fallback session");
    assert_eq!(session.path(), SESSION);
    let calls = fake.calls();
    assert!(
        calls.contains(&Call::GetSession("5".to_owned())),
        "{calls:?}"
    );
    assert!(
        calls.contains(&Call::GetSessionByPid(std::process::id())),
        "{calls:?}"
    );
}

#[test]
pub(super) fn session_lookup_uses_xdg_session_id_when_it_resolves() {
    let fake = Rc::new(FakeBus::new());
    let bus: Rc<dyn Bus> = fake.clone();
    LogindSession::open_with_bus(bus, Some("5"), None).expect("session");
    let calls = fake.calls();
    assert!(
        calls.contains(&Call::GetSession("5".to_owned())),
        "{calls:?}"
    );
    assert!(
        !calls
            .iter()
            .any(|call| matches!(call, Call::GetSessionByPid(_))),
        "{calls:?}"
    );
}

#[test]
pub(super) fn greeter_session_is_rejected_before_take_control() {
    let fake = Rc::new(FakeBus::new());
    *fake.class.borrow_mut() = "greeter".to_owned();
    let bus: Rc<dyn Bus> = fake.clone();
    let error = LogindSession::open_with_bus(bus, Some("5"), None)
        .err()
        .expect("rejected");
    assert_eq!(error.reason(), Some("greeter"));
    assert_eq!(fake.count(&Call::TakeControl), 0);
    assert_eq!(fake.count(&Call::Subscribe), 0);
}

#[test]
pub(super) fn inactive_session_of_another_user_is_rejected() {
    let fake = Rc::new(FakeBus::new());
    fake.active.set(false);
    // GetSessionByPID resolves a different session, so the requested one is somebody else's.
    *fake.by_pid.borrow_mut() = Some("/org/freedesktop/login1/session/_99".to_owned());
    let bus: Rc<dyn Bus> = fake.clone();
    let error = LogindSession::open_with_bus(bus, Some("5"), None)
        .err()
        .expect("rejected");
    assert_eq!(error.reason(), Some("inactive"));
    assert_eq!(fake.count(&Call::TakeControl), 0);
    assert_eq!(fake.count(&Call::Subscribe), 0);
}

/// A display manager starts us before the VT is ours: the session has to be activated explicitly
/// (`Session.Activate`, after `TakeControl`) and it then opens once logind reports `Active`.
#[test]
pub(super) fn inactive_own_session_is_activated_then_taken_over() {
    let fake = Rc::new(FakeBus::new());
    fake.active.set(false);
    let bus: Rc<dyn Bus> = fake.clone();
    let session = LogindSession::open_with_bus_waiting(
        bus,
        Some("5"),
        Some("seat0"),
        ActiveWait::IMMEDIATE,
        ActiveWait::IMMEDIATE,
        None,
    )
    .expect("own inactive session is activated and taken over");
    assert_eq!(fake.count(&Call::Activate), 1, "{:?}", fake.calls());
    assert_eq!(fake.count(&Call::TakeControl), 1);
    assert_eq!(fake.count(&Call::Subscribe), 1);
    // No VT in the environment: no `Seat.SwitchTo`.
    assert!(
        !fake
            .calls()
            .iter()
            .any(|call| matches!(call, Call::SwitchTo(_))),
        "{:?}",
        fake.calls()
    );
    assert!(session.is_active_flag());
    // Activation is requested only once, and only after control was taken.
    let calls = fake.calls();
    let control = calls.iter().position(|c| *c == Call::TakeControl);
    let activate = calls.iter().position(|c| *c == Call::Activate);
    assert!(
        matches!((control, activate), (Some(control), Some(activate)) if control < activate),
        "{calls:?}"
    );
    // A later `Activate` signal still only confirms what the property already said.
    fake.signal(Event::ActivateSession);
    assert!(matches!(
        session.take_events().as_slice(),
        [Event::ActivateSession]
    ));
    assert!(session.is_active_flag());
    assert_eq!(fake.count(&Call::Activate), 1, "{calls:?}");
}

/// With a VT the activation also asks the seat to switch to it.
#[test]
pub(super) fn activation_switches_to_the_session_vt() {
    let fake = Rc::new(FakeBus::new());
    fake.active.set(false);
    let bus: Rc<dyn Bus> = fake.clone();
    let session = LogindSession::open_with_bus_waiting(
        bus,
        Some("5"),
        None,
        ActiveWait::IMMEDIATE,
        ActiveWait::IMMEDIATE,
        Some(2),
    )
    .expect("activated on VT 2");
    assert!(session.is_active_flag());
    let calls = fake.calls();
    assert_eq!(fake.count(&Call::Activate), 1, "{calls:?}");
    assert!(
        calls.contains(&Call::SwitchTo(2)),
        "expected Seat.SwitchTo(2): {calls:?}"
    );
}

/// Fail closed: a session logind will not activate must not be taken over, and control goes back.
#[test]
pub(super) fn still_inactive_after_activation_fails_closed() {
    let fake = Rc::new(FakeBus::new());
    fake.active.set(false);
    fake.activates.set(false); // logind refuses to activate this session
    let bus: Rc<dyn Bus> = fake.clone();
    let error = LogindSession::open_with_bus_waiting(
        bus,
        Some("5"),
        Some("seat0"),
        ActiveWait::IMMEDIATE,
        ActiveWait::IMMEDIATE,
        Some(3),
    )
    .err()
    .expect("refused to continue inactive");
    assert_eq!(error.reason(), Some("inactive_after_activate"));
    assert_eq!(fake.count(&Call::Activate), 1, "{:?}", fake.calls());
    assert_eq!(fake.count(&Call::SwitchTo(3)), 1, "{:?}", fake.calls());
    assert_eq!(fake.count(&Call::TakeControl), 1);
    // The guard released control instead of leaving logind holding a session nobody uses.
    assert_eq!(fake.count(&Call::ReleaseControl), 1, "{:?}", fake.calls());
}

/// An already active session is not activated again: no `Activate`, no `SwitchTo`.
#[test]
pub(super) fn active_session_is_not_activated() {
    let (session, fake) = session();
    assert!(session.is_active_flag());
    let calls = fake.calls();
    assert_eq!(fake.count(&Call::Activate), 0, "{calls:?}");
    assert!(
        !calls.iter().any(|call| matches!(call, Call::SwitchTo(_))),
        "{calls:?}"
    );
}

#[test]
pub(super) fn vt_number_parsing() {
    assert_eq!(parse_vt(Some("2")), Some(2));
    assert_eq!(parse_vt(Some(" 7 ")), Some(7));
    assert_eq!(parse_vt(Some("0")), None);
    assert_eq!(parse_vt(Some("-1")), None);
    assert_eq!(parse_vt(Some("tty1")), None);
    assert_eq!(parse_vt(Some("")), None);
    assert_eq!(parse_vt(None), None);
}

#[test]
pub(super) fn activation_during_the_wait_reports_waited_for_active() {
    let fake = Rc::new(FakeBus::new());
    fake.active.set(false);
    // Reads 1..=3 are inactive, the fourth is active.
    fake.active_after.set(Some(3));
    let validated = validate_session(fake.as_ref(), SESSION, Some("seat0"), ActiveWait::IMMEDIATE)
        .expect("validated");
    assert!(validated.active);
    assert_eq!(validated.note, Some("waited_for_active"));
}

/// Validation no longer has a "continue inactive" note: the open activates or fails.
#[test]
pub(super) fn activation_after_the_wait_leaves_the_session_inactive() {
    let fake = Rc::new(FakeBus::new());
    fake.active.set(false);
    let validated =
        validate_session(fake.as_ref(), SESSION, None, ActiveWait::IMMEDIATE).expect("validated");
    assert!(!validated.active);
    assert_eq!(validated.note, None);
}

#[test]
pub(super) fn session_on_another_seat_is_rejected() {
    let fake = Rc::new(FakeBus::new());
    *fake.seat.borrow_mut() = "seat7".to_owned();
    let bus: Rc<dyn Bus> = fake.clone();
    let error = LogindSession::open_with_bus(bus, Some("5"), Some("seat0"))
        .err()
        .expect("rejected");
    assert_eq!(error.reason(), Some("wrong-seat"));
    assert_eq!(fake.count(&Call::TakeControl), 0);
}

#[test]
pub(super) fn pause_is_not_acked_before_the_consumer_acknowledges() {
    let (session, fake) = session();
    fake.signal_pause(1, 2, PauseKind::Pause);
    session.pump();
    // Queued, not acked: the callback never talks to logind.
    assert!(fake.pause_acks().is_empty(), "{:?}", fake.calls());
    let events = session.take_events();
    assert!(
        matches!(
            events.as_slice(),
            [Event::Pause {
                major: 1,
                minor: 2,
                kind: PauseKind::Pause
            }]
        ),
        "{events:?}"
    );
    assert!(fake.pause_acks().is_empty(), "{:?}", fake.calls());
    assert_eq!(session.pending_devices(), vec![(1, 2)]);
}

#[test]
pub(super) fn ack_pause_sends_pause_device_complete_exactly_once() {
    let (session, fake) = session();
    fake.signal_pause(1, 2, PauseKind::Pause);
    assert_eq!(session.take_events().len(), 1);
    session.ack_pause(1, 2).expect("first ack");
    assert_eq!(fake.pause_acks(), vec![(1, 2)]);
    assert!(session.ack_pause(1, 2).is_err());
    assert_eq!(fake.pause_acks(), vec![(1, 2)]);
    assert!(session.pending_devices().is_empty());
}

#[test]
pub(super) fn force_and_gone_send_no_ack() {
    let (session, fake) = session();
    fake.signal_pause(1, 2, PauseKind::Force);
    fake.signal_pause(3, 4, PauseKind::Gone);
    assert_eq!(session.take_events().len(), 2);
    assert!(fake.pause_acks().is_empty(), "{:?}", fake.calls());
    assert!(session.pending_devices().is_empty());
    assert!(session.ack_pause(1, 2).is_err());
    assert!(fake.pause_acks().is_empty());
}

#[test]
pub(super) fn two_paused_devices_get_two_acks() {
    let (session, fake) = session();
    fake.signal_pause(1, 2, PauseKind::Pause);
    fake.signal_pause(3, 4, PauseKind::Pause);
    assert_eq!(session.take_events().len(), 2);
    assert_eq!(session.pending_devices(), vec![(1, 2), (3, 4)]);
    session.ack_pause(3, 4).expect("ack second device");
    session.ack_pause(1, 2).expect("ack first device");
    assert_eq!(fake.pause_acks(), vec![(3, 4), (1, 2)]);
    assert!(session.pending_devices().is_empty());
}

#[test]
pub(super) fn resume_carries_the_received_fd() {
    let (session, fake) = session();
    let file = File::open("/dev/null").expect("/dev/null");
    let raw = file.as_raw_fd();
    fake.signal(Event::Resume {
        major: 226,
        minor: 0,
        fd: OwnedFd::from(file),
    });
    let events = session.take_events();
    match events.as_slice() {
        [Event::Resume { major, minor, fd }] => {
            assert_eq!((*major, *minor), (226, 0));
            // The fd survived the hand-off: the session never closed it.
            assert_eq!(fd.as_raw_fd(), raw);
            assert!(
                File::from(fd.try_clone().expect("open fd"))
                    .metadata()
                    .is_ok()
            );
        }
        other => panic!("expected one Resume event, got {other:?}"),
    }
    // A resume never needs a pause reply.
    assert!(fake.pause_acks().is_empty());
}

/// A per-device pause (keyboard, touchpad) is per-device state: it must never clear session
/// activation, and the ack handshake must keep working exactly as before.
#[test]
pub(super) fn device_pause_does_not_clear_session_activation() {
    let (session, fake) = session();
    assert!(session.is_active_flag());
    fake.signal_pause(13, 5, PauseKind::Force); // keyboard
    fake.signal_pause(13, 6, PauseKind::Pause); // touchpad, logind waits for the reply
    assert_eq!(session.take_events().len(), 2);
    assert!(
        session.is_active_flag(),
        "a device pause must not deactivate the session"
    );
    assert_eq!(session.paused_devices(), vec![(13, 5), (13, 6)]);
    session.ack_pause(13, 6).expect("ack the paused touchpad");
    assert_eq!(fake.pause_acks(), vec![(13, 6)]);
    assert!(session.pending_devices().is_empty());
    assert_eq!(session.paused_devices(), vec![(13, 5), (13, 6)]);
}

#[test]
pub(super) fn resume_drops_paused_device_state_without_reactivating_a_session() {
    let (session, fake) = session();
    // Startup requires an active session now, so the inactive state a VT switch away produces is
    // simulated directly: what this test pins down is that a device resume never sets it.
    session.clear_active_flag();
    assert!(!session.is_active_flag());
    fake.signal_pause(13, 5, PauseKind::Force);
    session.take_events();
    assert_eq!(session.paused_devices(), vec![(13, 5)]);
    assert!(!session.is_active_flag());
    fake.signal(Event::Resume {
        major: 13,
        minor: 5,
        fd: OwnedFd::from(File::open("/dev/null").expect("/dev/null")),
    });
    session.take_events();
    assert!(session.paused_devices().is_empty());
    assert!(
        !session.is_active_flag(),
        "a device resume is not a session activation"
    );
    fake.signal(Event::ActivateSession);
    session.take_events();
    assert!(session.is_active_flag());
}

/// A `gone` device left the seat, so it never becomes paused bookkeeping and never needs an ack.
#[test]
pub(super) fn gone_devices_are_not_tracked_as_paused() {
    let (session, fake) = session();
    fake.signal_pause(226, 0, PauseKind::Gone);
    session.take_events();
    assert!(session.paused_devices().is_empty());
    assert!(session.is_active_flag());
    assert!(fake.pause_acks().is_empty(), "{:?}", fake.calls());
}

/// `TakeDevice` reporting `paused` is about that descriptor, not about the session.
#[cfg(feature = "drm")]
#[test]
pub(super) fn paused_take_device_does_not_clear_session_activation() {
    use smithay::backend::session::Session;
    use smithay::reexports::rustix::fs::OFlags;

    let (mut session, fake) = session();
    fake.take_device_paused.set(true);
    let fd = session
        .open(Path::new("/dev/null"), OFlags::RDWR)
        .expect("device opened");
    drop(fd);
    assert!(
        session.is_active(),
        "a paused device is not an inactive session"
    );
    // /dev/null is 1:3, so that is the key logind paused.
    assert_eq!(session.paused_devices(), vec![(1, 3)]);
}

#[test]
pub(super) fn release_control_is_idempotent() {
    let (session, fake) = session();
    session.release_control().expect("release");
    session.release_control().expect("release again");
    assert_eq!(fake.count(&Call::ReleaseControl), 1);
}

#[test]
pub(super) fn dropping_the_session_releases_control() {
    let fake = Rc::new(FakeBus::new());
    let bus: Rc<dyn Bus> = fake.clone();
    {
        let _session = LogindSession::open_with_bus(bus, None, None).expect("session");
        assert_eq!(fake.count(&Call::ReleaseControl), 0);
    }
    assert_eq!(fake.count(&Call::ReleaseControl), 1);
}

#[test]
pub(super) fn pause_kinds_carry_their_ack_requirement() {
    assert!(PauseKind::from_signal("whatever").is_none());
    assert!(PauseKind::from_signal("pause").expect("pause").needs_ack());
    assert!(!PauseKind::from_signal("force").expect("force").needs_ack());
    assert!(!PauseKind::from_signal("gone").expect("gone").needs_ack());
    assert_eq!(PauseKind::Gone.name(), "gone");
}
