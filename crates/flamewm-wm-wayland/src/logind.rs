//! Seat access through systemd-logind on the system bus (feature `drm`). Replaces smithay's libseat
//! session so the binary links only libraries that stock desktop installs already ship. Lightdm
//! creates a logind session for the user, so `TakeControl` and `TakeDevice` work without seatd.
//!
//! Device fds come from `TakeDevice`; VT switches arrive as `PauseDevice` / `ResumeDevice` signals.
//! Signals are read by `pump`, which the tty backend calls from a short calloop timer.
//!
//! # Ownership of the pause handshake
//!
//! The D-Bus signal callbacks only enqueue. `PauseDeviceComplete` is sent exclusively by
//! [`LogindSession::ack_pause`], once per paused device, because a pause must be queued for the
//! compositor (which has to stop scanout first) before logind is told the device is free. Paused
//! devices live in a pending list, so an unacknowledged pause is never lost and an acknowledged one
//! is never sent twice. `force` and `gone` are already revoked by logind and need no reply.
//!
//! The event queue and the pause bookkeeping never touch D-Bus directly: they talk to the [`Bus`]
//! abstraction, so the whole handshake is exercised by tests with a recording fake bus and without
//! a seat. The real `dbus` implementation lives in the `system` submodule, which only exists in
//! `drm` builds.
//!
//! # Session activation
//!
//! A session manager (lightdm, a login shell, a `--backend=drm` exec) can start this process before
//! logind has made the session the active one on its seat: the VT is still owned by the greeter while
//! the session is being set up. An inactive session that is *our own* is therefore not a failure —
//! validation polls the `Active` property for a short bounded time (see [`ActiveWait`]) and then
//! proceeds anyway, leaving `is_active()` false until the `ActivateSession` event arrives. The tty
//! backend arms DRM on that event, exactly like any other logind client. Sessions that are not ours
//! (another user, another seat, a greeter) are still rejected while inactive: taking control of them
//! would steal somebody else's devices.
//!
//! `is_active()` is session activation and nothing else (`ActivateSession` sets it, validation seeds
//! it). A per-device pause — the touchpad or keyboard going away, which logind also does for reasons
//! that have nothing to do with a VT switch — is tracked separately in [`Inner::paused_devices`] and
//! never clears it. Stopping scanout on a pause is the tty backend's own decision, so a single device
//! pause can no longer strand the session in a permanently inactive state.

#![cfg_attr(not(feature = "drm"), allow(dead_code, unused_imports))]

use std::cell::RefCell;
use std::collections::{BTreeSet, VecDeque};
use std::error::Error as StdError;
use std::fmt;
use std::fs::File;
use std::os::fd::OwnedFd;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Why a device was paused.
///
/// `pause` completes the VT switch logind is waiting for, so it must be acknowledged. `force` is
/// sent when logind revokes the device itself and `gone` when the device left the seat; both are
/// already revoked and must not be acknowledged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseKind {
    Pause,
    Force,
    Gone,
}

impl PauseKind {
    /// Parses the third `PauseDevice` signal argument.
    fn from_signal(kind: &str) -> Option<Self> {
        match kind {
            "pause" => Some(Self::Pause),
            "force" => Some(Self::Force),
            "gone" => Some(Self::Gone),
            _ => None,
        }
    }

    /// Only `pause` needs `PauseDeviceComplete`.
    pub fn needs_ack(self) -> bool {
        matches!(self, Self::Pause)
    }

    #[cfg(test)]
    fn name(self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::Force => "force",
            Self::Gone => "gone",
        }
    }
}

/// One device or session event, in logind delivery order.
#[derive(Debug)]
pub enum Event {
    /// The device is going away. `kind` says whether logind waits for [`LogindSession::ack_pause`].
    Pause {
        major: u32,
        minor: u32,
        kind: PauseKind,
    },
    /// The device is usable again; `fd` is the descriptor logind handed back and belongs to the
    /// consumer from here on (it is never closed by this module).
    Resume { major: u32, minor: u32, fd: OwnedFd },
    /// The session became the active one on its seat.
    ActivateSession,
}

/// Signal sink shared between the bus callbacks and the [`LogindSession`]; only ever appended to or
/// drained, never borrowed across a bus call.
#[derive(Clone)]
pub(crate) struct EventQueue(Arc<Mutex<VecDeque<Event>>>);

impl EventQueue {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(VecDeque::new())))
    }

    fn push(&self, event: Event) {
        match self.0.lock() {
            Ok(mut queue) => queue.push_back(event),
            Err(_) => eprintln!("flamewm-wayland: logind event queue poisoned"),
        }
    }

    fn drain(&self) -> Vec<Event> {
        self.0
            .lock()
            .map(|mut queue| queue.drain(..).collect())
            .unwrap_or_default()
    }
}

/// Session failure with the logind message, so the tty backend can report why it could not start.
#[derive(Debug)]
pub struct LogindError(String);

impl LogindError {
    fn rejected(reason: &str, message: String) -> Self {
        Self(format!("reason={reason}: {message}"))
    }

    /// The validated rejection reason (`greeter`, `inactive`, `wrong-seat`, `no-session`) when this
    /// error came from session validation. The runtime path reports it through `Display`.
    #[cfg(test)]
    pub fn reason(&self) -> Option<&str> {
        let rest = self.0.strip_prefix("reason=")?;
        Some(rest.split(':').next().unwrap_or(rest).trim())
    }
}

impl fmt::Display for LogindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl StdError for LogindError {}

/// The D-Bus surface logind access needs. Implemented by the real system bus in `drm` builds and by
/// a recording fake in the tests.
pub(crate) trait Bus {
    fn get_session(&self, id: &str) -> Result<String, LogindError>;
    fn get_session_by_pid(&self, pid: u32) -> Result<String, LogindError>;
    /// `(seat name, seat object path)` of a session.
    fn session_seat(&self, session_path: &str) -> Result<(String, String), LogindError>;
    fn session_class(&self, session_path: &str) -> Result<String, LogindError>;
    fn session_active(&self, session_path: &str) -> Result<bool, LogindError>;
    fn take_control(&self, session_path: &str) -> Result<(), LogindError>;
    fn release_control(&self, session_path: &str) -> Result<(), LogindError>;
    fn pause_device_complete(
        &self,
        session_path: &str,
        major: u32,
        minor: u32,
    ) -> Result<(), LogindError>;
    fn take_device(
        &self,
        session_path: &str,
        major: u32,
        minor: u32,
    ) -> Result<(OwnedFd, bool), LogindError>;
    fn release_device(&self, session_path: &str, major: u32, minor: u32)
    -> Result<(), LogindError>;
    fn switch_to(&self, seat_path: &str, vt: u32) -> Result<(), LogindError>;
    /// Starts `PauseDevice`, `ResumeDevice` and `Activate` delivery into `queue`.
    fn subscribe(&self, session_path: &str, queue: EventQueue) -> Result<(), LogindError>;
    fn pump(&self);
}

struct Inner {
    bus: Rc<dyn Bus>,
    session_path: String,
    seat_name: String,
    seat_path: String,
    /// Session activation only: seeded by validation and set by the `ActivateSession` event. A
    /// per-device pause never writes it (see [`Inner::paused_devices`]).
    active: bool,
    controlled: bool,
    /// Devices paused with [`PauseKind::Pause`] that still need `PauseDeviceComplete`, in arrival
    /// order. Recorded when the pause is handed to the compositor, so an ack can never overtake the
    /// pause it completes.
    pending: Vec<(u32, u32)>,
    /// Devices logind has paused and not yet resumed, keyed by `(major, minor)`.
    ///
    /// Per-device state, deliberately separate from session activation: a paused keyboard or
    /// touchpad says nothing about whether the session is the active one on its seat. Read by the
    /// resume path (a resume for a device that was never paused is logged) and by the tests.
    #[cfg_attr(not(test), allow(dead_code))]
    paused_devices: BTreeSet<(u32, u32)>,
}

impl Inner {
    fn release(&mut self) {
        if !self.controlled {
            return;
        }
        self.controlled = false;
        // Best effort: this also runs from `Drop`, where a failure has nowhere useful to go.
        if let Err(error) = self.bus.release_control(&self.session_path) {
            eprintln!("flamewm-wayland: logind ReleaseControl failed: {error}");
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        // A failing `drm::init` drops the session before anything else ran; logind must not keep
        // control of a session this compositor no longer uses.
        self.release();
    }
}

/// Cloneable handle; every clone shares one bus connection and event queue.
#[derive(Clone)]
pub struct LogindSession {
    inner: Rc<RefCell<Inner>>,
    events: EventQueue,
}

fn device_numbers(path: &Path) -> Result<(u32, u32), LogindError> {
    let rdev = std::fs::metadata(path)
        .map_err(|e| LogindError(format!("stat {}: {e}", path.display())))?
        .rdev();
    Ok((major(rdev), minor(rdev)))
}

fn major(dev: u64) -> u32 {
    (((dev >> 8) & 0xfff) | ((dev >> 32) & !0xfff)) as u32
}

fn minor(dev: u64) -> u32 {
    ((dev & 0xff) | ((dev >> 12) & !0xff)) as u32
}

/// Resolves the logind session object path for this process.
///
/// `XDG_SESSION_ID` is only a hint: under login managers, wrappers and `su` it can be absent or
/// stale, so any failure of `GetSession` falls back to `GetSessionByPID` for our own pid.
fn resolve_session(bus: &dyn Bus, explicit: Option<&str>) -> Result<String, LogindError> {
    if let Some(id) = explicit.map(str::trim).filter(|id| !id.is_empty()) {
        match bus.get_session(id) {
            Ok(path) => return Ok(path),
            Err(error) => eprintln!(
                "flamewm-wayland: logind GetSession failed ({error}); falling back to GetSessionByPID"
            ),
        }
    }
    let pid = std::process::id();
    bus.get_session_by_pid(pid).map_err(|error| {
        LogindError::rejected(
            "no-session",
            format!("no logind session for pid {pid}: {error}"),
        )
    })
}

/// The properties a session must have before this compositor may take it over.
struct Validated {
    seat_name: String,
    seat_path: String,
    /// Session activation at the end of validation. False when the wait below expired; the session
    /// is then taken over anyway and activation is awaited as an event.
    active: bool,
    /// Short log token describing what the activation wait decided, for the startup log.
    note: Option<&'static str>,
}

/// Bounded wait for the `Active` property, in logind property reads.
///
/// libseat blocks until the seat is active, and a compositor is routinely started while the VT still
/// belongs to the greeter. Ten reads over three seconds cover a normal session setup; after that the
/// session is used anyway and activation arrives as the `ActivateSession` signal.
#[derive(Clone, Copy, Debug)]
struct ActiveWait {
    attempts: usize,
    interval: Duration,
}

impl ActiveWait {
    /// The production budget: 10 × 300 ms.
    const DEFAULT: Self = Self {
        attempts: 10,
        interval: Duration::from_millis(300),
    };

    /// The same number of reads without the pauses, so the tests exercise the polling loop itself.
    #[cfg(test)]
    const IMMEDIATE: Self = Self {
        attempts: 10,
        interval: Duration::ZERO,
    };
}

/// Polls the `Active` property up to `wait.attempts` times. A read that fails counts as "not active
/// yet": the caller's fallback (proceed and wait for `ActivateSession`) is the same either way.
///
/// Runs before the event loop takes over, so the short sleeps here never block a calloop callback.
fn wait_for_active(bus: &dyn Bus, session_path: &str, wait: ActiveWait) -> bool {
    for attempt in 0..wait.attempts {
        if !wait.interval.is_zero() && attempt > 0 {
            std::thread::sleep(wait.interval);
        }
        match bus.session_active(session_path) {
            Ok(true) => return true,
            Ok(false) => {}
            Err(error) => eprintln!("flamewm-wayland: logind Active read failed: {error}"),
        }
    }
    false
}

/// Whether `session_path` is this process's own session.
///
/// `GetSessionByPID` is how logind itself answers the question. A failed lookup counts as "ours",
/// because the session path then came from `XDG_SESSION_ID` and `TakeControl` still enforces
/// ownership on logind's side; refusing here would resurrect the very bug the wait exists to fix.
fn is_own_session(bus: &dyn Bus, session_path: &str) -> bool {
    match bus.get_session_by_pid(std::process::id()) {
        Ok(own) => own == session_path,
        Err(_) => true,
    }
}

/// Validates the resolved session.
///
/// Only a user session on the expected seat may be controlled: a greeter or another seat's session
/// belongs to somebody else, and taking control of it would steal their devices. A session of our
/// own that is not active yet is waited for (see [`ActiveWait`]) instead of rejected; any other
/// inactive session is still rejected.
fn validate_session(
    bus: &dyn Bus,
    session_path: &str,
    expected_seat: Option<&str>,
    wait: ActiveWait,
) -> Result<Validated, LogindError> {
    let class = bus.session_class(session_path)?;
    if class != "user" {
        return Err(LogindError::rejected(
            "greeter",
            format!("session {session_path} has class {class}, not a user session"),
        ));
    }
    // The seat is checked before the activation wait: a foreign seat must not cost three seconds.
    let (seat_name, seat_path) = bus.session_seat(session_path)?;
    if let Some(expected) = expected_seat.map(str::trim).filter(|seat| !seat.is_empty()) {
        if expected != seat_name {
            return Err(LogindError::rejected(
                "wrong-seat",
                format!("XDG_SEAT={expected} but session {session_path} is on seat {seat_name}"),
            ));
        }
    }
    let mut active = bus.session_active(session_path)?;
    let mut note = None;
    if !active {
        if !is_own_session(bus, session_path) {
            return Err(LogindError::rejected(
                "inactive",
                format!("session {session_path} is not the active session on its seat"),
            ));
        }
        if wait_for_active(bus, session_path, wait) {
            active = true;
            note = Some("waited_for_active");
        } else {
            // Deliberate: proceed and let `ActivateSession` activate the session.
            note = Some("inactive_continuing");
        }
    }
    Ok(Validated {
        seat_name,
        seat_path,
        active,
        note,
    })
}

impl LogindSession {
    /// Connects to the system bus, takes control of the caller's logind session and subscribes to
    /// the device pause, resume and activation signals.
    #[cfg(feature = "drm")]
    pub fn new() -> Result<Self, LogindError> {
        let bus: Rc<dyn Bus> = Rc::new(system::SystemBus::connect()?);
        let session_id = std::env::var("XDG_SESSION_ID").ok();
        let seat = std::env::var("XDG_SEAT").ok();
        Self::open_with_bus(bus, session_id.as_deref(), seat.as_deref())
    }

    /// Session discovery, validation, `TakeControl` and signal subscription over any [`Bus`].
    fn open_with_bus(
        bus: Rc<dyn Bus>,
        session_id: Option<&str>,
        expected_seat: Option<&str>,
    ) -> Result<Self, LogindError> {
        Self::open_with_bus_waiting(bus, session_id, expected_seat, ActiveWait::DEFAULT)
    }

    /// [`Self::open_with_bus`] with an explicit activation budget, so tests do not sleep.
    fn open_with_bus_waiting(
        bus: Rc<dyn Bus>,
        session_id: Option<&str>,
        expected_seat: Option<&str>,
        wait: ActiveWait,
    ) -> Result<Self, LogindError> {
        let session_path = resolve_session(bus.as_ref(), session_id)?;
        let validated = validate_session(bus.as_ref(), &session_path, expected_seat, wait)?;
        bus.take_control(&session_path).map_err(|error| {
            LogindError(format!(
                "logind TakeControl on {session_path} failed; run from the user's own graphical session: {error}"
            ))
        })?;
        let events = EventQueue::new();
        bus.subscribe(&session_path, events.clone())?;
        if let Some(note) = validated.note {
            eprintln!("flamewm-wayland: logind session {session_path} detail={note}");
        }
        // The seat can become active between the last poll and the subscription, and that one-shot
        // `Activate` signal would be lost. Queue the event the tty backend arms DRM on instead of
        // silently claiming an activation no event ever announced.
        if !validated.active && bus.session_active(&session_path).unwrap_or(false) {
            eprintln!("flamewm-wayland: logind session {session_path} detail=activate_queued");
            events.push(Event::ActivateSession);
        }
        Ok(Self {
            inner: Rc::new(RefCell::new(Inner {
                bus,
                session_path,
                seat_name: validated.seat_name,
                seat_path: validated.seat_path,
                active: validated.active,
                controlled: true,
                pending: Vec::new(),
                paused_devices: BTreeSet::new(),
            })),
            events,
        })
    }

    /// Dispatches pending bus messages without blocking. Callbacks only enqueue.
    pub fn pump(&self) {
        let bus = Rc::clone(&self.inner.borrow().bus);
        bus.pump();
    }

    /// Drains the pause, resume and activation events seen since the last call, in delivery order.
    ///
    /// A `pause` handed out here becomes pending: the compositor has seen the pause, so
    /// [`Self::ack_pause`] may now complete it. `force` and `gone` are not tracked because logind
    /// never waits for them.
    ///
    /// A device pause is per-device bookkeeping only. Session activation changes on
    /// `ActivateSession` alone, so a keyboard or touchpad pause can no longer mark the session
    /// inactive for the rest of its life.
    pub fn take_events(&self) -> Vec<Event> {
        let raw = self.events.drain();
        let mut delivered = Vec::with_capacity(raw.len());
        let mut inner = self.inner.borrow_mut();
        for event in raw {
            match &event {
                Event::Pause { major, minor, kind } => {
                    // `gone` means the device left the seat, so it is neither paused nor resumable.
                    if *kind != PauseKind::Gone {
                        inner.paused_devices.insert((*major, *minor));
                    }
                    if kind.needs_ack() {
                        inner.pending.push((*major, *minor));
                    }
                }
                Event::Resume { major, minor, .. } => {
                    if !inner.paused_devices.remove(&(*major, *minor)) {
                        eprintln!(
                            "flamewm-wayland: logind resume {major}:{minor} detail=resume_without_pause"
                        );
                    }
                }
                Event::ActivateSession => inner.active = true,
            }
            delivered.push(event);
        }
        delivered
    }

    /// Completes one paused device. This is the only place `PauseDeviceComplete` is sent.
    ///
    /// Each paused device is acknowledged exactly once: the device leaves the pending list before
    /// the call, so a repeated ack fails instead of repeating the reply. A call that fails is put
    /// back so the caller can retry.
    pub fn ack_pause(&self, major: u32, minor: u32) -> Result<(), LogindError> {
        let (bus, session_path) = {
            let mut inner = self.inner.borrow_mut();
            match inner
                .pending
                .iter()
                .position(|device| *device == (major, minor))
            {
                Some(index) => {
                    inner.pending.remove(index);
                }
                None => {
                    return Err(LogindError(format!(
                        "ack_pause({major}:{minor}): device is not pending a pause reply"
                    )));
                }
            }
            (Rc::clone(&inner.bus), inner.session_path.clone())
        };
        if let Err(error) = bus.pause_device_complete(&session_path, major, minor) {
            self.inner.borrow_mut().pending.push((major, minor));
            return Err(error);
        }
        Ok(())
    }

    /// Gives control of the session back to logind. Idempotent, so it is safe from both an explicit
    /// shutdown path and `Drop`.
    pub fn release_control(&self) -> Result<(), LogindError> {
        let (bus, session_path, controlled) = {
            let inner = self.inner.borrow();
            (
                Rc::clone(&inner.bus),
                inner.session_path.clone(),
                inner.controlled,
            )
        };
        if !controlled {
            return Ok(());
        }
        bus.release_control(&session_path)?;
        self.inner.borrow_mut().controlled = false;
        Ok(())
    }

    /// Devices that are waiting for [`Self::ack_pause`].
    #[cfg(test)]
    fn pending_devices(&self) -> Vec<(u32, u32)> {
        self.inner.borrow().pending.clone()
    }

    /// Devices logind paused and has not resumed yet. Independent of the activation flag.
    #[cfg(test)]
    fn paused_devices(&self) -> Vec<(u32, u32)> {
        self.inner.borrow().paused_devices.iter().copied().collect()
    }

    /// The session object path this handle controls.
    #[cfg(test)]
    fn path(&self) -> String {
        self.inner.borrow().session_path.clone()
    }

    /// Test-only view of the cached session-active flag (the `Session` trait's `is_active` only
    /// exists in `drm` builds).
    #[cfg(test)]
    fn is_active_flag(&self) -> bool {
        self.inner.borrow().active
    }
}

#[cfg(feature = "drm")]
impl smithay::backend::session::AsErrno for LogindError {
    fn as_errno(&self) -> Option<i32> {
        None
    }
}

#[cfg(feature = "drm")]
impl From<dbus::Error> for LogindError {
    fn from(error: dbus::Error) -> Self {
        Self(format!(
            "{}: {}",
            error.name().unwrap_or("dbus error"),
            error.message().unwrap_or("")
        ))
    }
}

#[cfg(feature = "drm")]
impl smithay::backend::session::Session for LogindSession {
    type Error = LogindError;

    fn open(
        &mut self,
        path: &Path,
        _flags: smithay::reexports::rustix::fs::OFlags,
    ) -> Result<OwnedFd, Self::Error> {
        let (major, minor) = device_numbers(path)?;
        let mut inner = self.inner.borrow_mut();
        let session_path = inner.session_path.clone();
        let (fd, paused) = inner.bus.take_device(&session_path, major, minor)?;
        // `paused` is about this device, not about the session: a device logind hands over paused
        // (the VT is being switched, the device was reclaimed) says nothing about whether the
        // session is the active one on its seat.
        if paused {
            inner.paused_devices.insert((major, minor));
        }
        Ok(fd)
    }

    fn close(&mut self, fd: OwnedFd) -> Result<(), Self::Error> {
        let meta = File::from(fd)
            .metadata()
            .map_err(|e| LogindError(format!("fstat device: {e}")))?;
        let key = (major(meta.rdev()), minor(meta.rdev()));
        let inner = self.inner.borrow();
        inner.bus.release_device(&inner.session_path, key.0, key.1)
    }

    fn change_vt(&mut self, vt: i32) -> Result<(), Self::Error> {
        let inner = self.inner.borrow();
        let vt = u32::try_from(vt).map_err(|_| LogindError(format!("invalid VT {vt}")))?;
        inner.bus.switch_to(&inner.seat_path, vt)
    }

    fn is_active(&self) -> bool {
        self.inner.borrow().active
    }

    fn seat(&self) -> String {
        self.inner.borrow().seat_name.clone()
    }
}

#[cfg(feature = "drm")]
mod system {
    use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};
    use std::time::Duration;

    use dbus::arg::OwnedFd as DbusFd;
    use dbus::blocking::Connection;
    use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
    use dbus::message::MatchRule;

    use super::{Bus, Event, EventQueue, LogindError, PauseKind};

    const LOGIN: &str = "org.freedesktop.login1";
    const MANAGER_PATH: &str = "/org/freedesktop/login1";
    const MANAGER_IF: &str = "org.freedesktop.login1.Manager";
    const SESSION_IF: &str = "org.freedesktop.login1.Session";
    const SEAT_IF: &str = "org.freedesktop.login1.Seat";
    const TIMEOUT: Duration = Duration::from_secs(5);

    /// The system-bus implementation of [`Bus`].
    pub(super) struct SystemBus {
        conn: Connection,
    }

    fn object_path(path: &str) -> Result<dbus::Path<'static>, LogindError> {
        dbus::Path::new(path)
            .map(|path| path.into_static())
            .map_err(|error| LogindError(format!("invalid object path {path}: {error}")))
    }

    impl SystemBus {
        pub(super) fn connect() -> Result<Self, LogindError> {
            Ok(Self {
                conn: Connection::new_system()?,
            })
        }

        fn session_property<R: for<'b> dbus::arg::Get<'b> + 'static>(
            &self,
            session_path: &str,
            property: &str,
        ) -> Result<R, LogindError> {
            let session = self
                .conn
                .with_proxy(LOGIN, object_path(session_path)?, TIMEOUT);
            Ok(session.get(SESSION_IF, property)?)
        }

        fn session_call<A: dbus::arg::AppendAll>(
            &self,
            session_path: &str,
            method: &str,
            args: A,
        ) -> Result<(), LogindError> {
            let session = self
                .conn
                .with_proxy(LOGIN, object_path(session_path)?, TIMEOUT);
            session.method_call::<(), _, _, _>(SESSION_IF, method, args)?;
            Ok(())
        }
    }

    impl Bus for SystemBus {
        fn get_session(&self, id: &str) -> Result<String, LogindError> {
            let manager = self.conn.with_proxy(LOGIN, MANAGER_PATH, TIMEOUT);
            let (path,): (dbus::Path<'static>,) =
                manager.method_call(MANAGER_IF, "GetSession", (id,))?;
            Ok(path.to_string())
        }

        fn get_session_by_pid(&self, pid: u32) -> Result<String, LogindError> {
            let manager = self.conn.with_proxy(LOGIN, MANAGER_PATH, TIMEOUT);
            let (path,): (dbus::Path<'static>,) =
                manager.method_call(MANAGER_IF, "GetSessionByPID", (pid,))?;
            Ok(path.to_string())
        }

        fn session_seat(&self, session_path: &str) -> Result<(String, String), LogindError> {
            let (name, path): (String, dbus::Path<'static>) =
                self.session_property(session_path, "Seat")?;
            Ok((name, path.to_string()))
        }

        fn session_class(&self, session_path: &str) -> Result<String, LogindError> {
            self.session_property(session_path, "Class")
        }

        fn session_active(&self, session_path: &str) -> Result<bool, LogindError> {
            self.session_property(session_path, "Active")
        }

        fn take_control(&self, session_path: &str) -> Result<(), LogindError> {
            self.session_call(session_path, "TakeControl", (false,))
        }

        fn release_control(&self, session_path: &str) -> Result<(), LogindError> {
            self.session_call(session_path, "ReleaseControl", ())
        }

        fn pause_device_complete(
            &self,
            session_path: &str,
            major: u32,
            minor: u32,
        ) -> Result<(), LogindError> {
            self.session_call(session_path, "PauseDeviceComplete", (major, minor))
        }

        fn take_device(
            &self,
            session_path: &str,
            major: u32,
            minor: u32,
        ) -> Result<(OwnedFd, bool), LogindError> {
            let session = self
                .conn
                .with_proxy(LOGIN, object_path(session_path)?, TIMEOUT);
            let (fd, paused): (DbusFd, bool) =
                session.method_call(SESSION_IF, "TakeDevice", (major, minor))?;
            // SAFETY: `into_raw_fd` gives up dbus's ownership of the descriptor, so the `OwnedFd`
            // below is its only owner.
            #[allow(unsafe_code)]
            let fd = unsafe { OwnedFd::from_raw_fd(fd.into_raw_fd()) };
            Ok((fd, paused))
        }

        fn release_device(
            &self,
            session_path: &str,
            major: u32,
            minor: u32,
        ) -> Result<(), LogindError> {
            self.session_call(session_path, "ReleaseDevice", (major, minor))
        }

        fn switch_to(&self, seat_path: &str, vt: u32) -> Result<(), LogindError> {
            let seat = self
                .conn
                .with_proxy(LOGIN, object_path(seat_path)?, TIMEOUT);
            seat.method_call::<(), _, _, _>(SEAT_IF, "SwitchTo", (vt,))?;
            Ok(())
        }

        fn subscribe(&self, session_path: &str, queue: EventQueue) -> Result<(), LogindError> {
            let path = object_path(session_path)?;

            // Signal callbacks only enqueue: the pause reply is the compositor's decision.
            let rule = MatchRule::new_signal(SESSION_IF, "PauseDevice").with_path(path.clone());
            let pause_queue = queue.clone();
            self.conn.add_match(
                rule,
                move |(major, minor, kind): (u32, u32, String), _, _| {
                    match PauseKind::from_signal(&kind) {
                        Some(kind) => pause_queue.push(Event::Pause { major, minor, kind }),
                        None => eprintln!("flamewm-wayland: unknown PauseDevice kind {kind}"),
                    }
                    true
                },
            )?;

            let rule = MatchRule::new_signal(SESSION_IF, "ResumeDevice").with_path(path.clone());
            let resume_queue = queue.clone();
            self.conn
                .add_match(rule, move |(major, minor, fd): (u32, u32, DbusFd), _, _| {
                    // SAFETY: dbus hands the descriptor over; from here the event owns it and the
                    // compositor closes it.
                    #[allow(unsafe_code)]
                    let fd = unsafe { OwnedFd::from_raw_fd(fd.into_raw_fd()) };
                    resume_queue.push(Event::Resume { major, minor, fd });
                    true
                })?;

            let rule = MatchRule::new_signal(SESSION_IF, "Activate").with_path(path);
            let activate_queue = queue;
            self.conn.add_match(rule, move |(), _, _| {
                activate_queue.push(Event::ActivateSession);
                true
            })?;
            Ok(())
        }

        fn pump(&self) {
            if let Err(error) = self.conn.process(Duration::ZERO) {
                eprintln!("flamewm-wayland: logind bus error: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::os::fd::AsRawFd;

    const SESSION: &str = "/org/freedesktop/login1/session/_32";
    const SEAT: &str = "/org/freedesktop/login1/seat/seat0";

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Call {
        GetSession(String),
        GetSessionByPid(u32),
        Class,
        Active,
        TakeControl,
        ReleaseControl,
        PauseDeviceComplete(u32, u32),
        Subscribe,
    }

    /// Records every call and can inject the signals logind would send.
    struct FakeBus {
        session: RefCell<Option<String>>,
        by_pid: RefCell<Option<String>>,
        class: RefCell<String>,
        active: Cell<bool>,
        /// Read counter at which `Active` starts reporting true, for the activation wait.
        active_after: Cell<Option<usize>>,
        active_reads: Cell<usize>,
        take_device_paused: Cell<bool>,
        seat: RefCell<String>,
        calls: RefCell<Vec<Call>>,
        queue: RefCell<Option<EventQueue>>,
    }

    impl FakeBus {
        fn new() -> Self {
            Self {
                session: RefCell::new(Some(SESSION.to_owned())),
                by_pid: RefCell::new(Some(SESSION.to_owned())),
                class: RefCell::new("user".to_owned()),
                active: Cell::new(true),
                active_after: Cell::new(None),
                active_reads: Cell::new(0),
                take_device_paused: Cell::new(false),
                seat: RefCell::new("seat0".to_owned()),
                calls: RefCell::new(Vec::new()),
                queue: RefCell::new(None),
            }
        }

        fn record(&self, call: Call) {
            self.calls.borrow_mut().push(call);
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.borrow().clone()
        }

        fn count(&self, wanted: &Call) -> usize {
            self.calls
                .borrow()
                .iter()
                .filter(|call| *call == wanted)
                .count()
        }

        fn pause_acks(&self) -> Vec<(u32, u32)> {
            self.calls
                .borrow()
                .iter()
                .filter_map(|call| match call {
                    Call::PauseDeviceComplete(major, minor) => Some((*major, *minor)),
                    _ => None,
                })
                .collect()
        }

        fn signal(&self, event: Event) {
            self.queue
                .borrow()
                .as_ref()
                .expect("session subscribed first")
                .push(event);
        }

        fn signal_pause(&self, major: u32, minor: u32, kind: PauseKind) {
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

        fn switch_to(&self, _seat_path: &str, _vt: u32) -> Result<(), LogindError> {
            Ok(())
        }

        fn subscribe(&self, _session_path: &str, queue: EventQueue) -> Result<(), LogindError> {
            self.record(Call::Subscribe);
            *self.queue.borrow_mut() = Some(queue);
            Ok(())
        }

        fn pump(&self) {}
    }

    fn session() -> (LogindSession, Rc<FakeBus>) {
        let fake = Rc::new(FakeBus::new());
        let bus: Rc<dyn Bus> = fake.clone();
        let session =
            LogindSession::open_with_bus(bus, Some("5"), Some("seat0")).expect("user session");
        (session, fake)
    }

    #[test]
    fn active_user_session_takes_control_and_subscribes() {
        let (session, fake) = session();
        assert_eq!(session.path(), SESSION);
        assert_eq!(fake.count(&Call::TakeControl), 1);
        assert_eq!(fake.count(&Call::Subscribe), 1);
        assert!(session.is_active_flag());
    }

    #[test]
    fn session_lookup_falls_back_to_get_session_by_pid() {
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
    fn session_lookup_uses_xdg_session_id_when_it_resolves() {
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
    fn greeter_session_is_rejected_before_take_control() {
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
    fn inactive_session_of_another_user_is_rejected() {
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

    /// A display manager starts us before the VT is ours: waiting for `Active` and then proceeding is
    /// the fix for "cursor, then back to the greeter", and activation arrives as an event.
    #[test]
    fn inactive_own_session_is_taken_over_and_waits_for_activation() {
        let fake = Rc::new(FakeBus::new());
        fake.active.set(false);
        let bus: Rc<dyn Bus> = fake.clone();
        let session = LogindSession::open_with_bus_waiting(
            bus,
            Some("5"),
            Some("seat0"),
            ActiveWait::IMMEDIATE,
        )
        .expect("own inactive session is taken over");
        // One validating read, the ten polls and one read after subscribing.
        assert!(
            fake.count(&Call::Active) > ActiveWait::IMMEDIATE.attempts,
            "{:?}",
            fake.calls()
        );
        assert_eq!(fake.count(&Call::TakeControl), 1);
        assert_eq!(fake.count(&Call::Subscribe), 1);
        assert!(
            !session.is_active_flag(),
            "inactive until an Activate event"
        );
        fake.signal(Event::ActivateSession);
        assert!(matches!(
            session.take_events().as_slice(),
            [Event::ActivateSession]
        ));
        assert!(session.is_active_flag());
    }

    #[test]
    fn activation_during_the_wait_reports_waited_for_active() {
        let fake = Rc::new(FakeBus::new());
        fake.active.set(false);
        // Reads 1..=3 are inactive, the fourth is active.
        fake.active_after.set(Some(3));
        let validated =
            validate_session(fake.as_ref(), SESSION, Some("seat0"), ActiveWait::IMMEDIATE)
                .expect("validated");
        assert!(validated.active);
        assert_eq!(validated.note, Some("waited_for_active"));
    }

    #[test]
    fn activation_after_the_wait_reports_inactive_continuing() {
        let fake = Rc::new(FakeBus::new());
        fake.active.set(false);
        let validated = validate_session(fake.as_ref(), SESSION, None, ActiveWait::IMMEDIATE)
            .expect("validated");
        assert!(!validated.active);
        assert_eq!(validated.note, Some("inactive_continuing"));
    }

    /// The one-shot `Activate` can fire between the last poll and `subscribe`; the event is queued
    /// so the tty backend still arms DRM exactly once.
    #[test]
    fn activation_between_the_wait_and_the_subscription_is_queued() {
        let fake = Rc::new(FakeBus::new());
        fake.active.set(false);
        // 11 reads are the validating read plus the polls; the read after subscribing is the 12th.
        fake.active_after.set(Some(11));
        let bus: Rc<dyn Bus> = fake.clone();
        let session =
            LogindSession::open_with_bus_waiting(bus, Some("5"), None, ActiveWait::IMMEDIATE)
                .expect("session");
        assert!(!session.is_active_flag());
        assert!(matches!(
            session.take_events().as_slice(),
            [Event::ActivateSession]
        ));
        assert!(session.is_active_flag());
    }

    #[test]
    fn session_on_another_seat_is_rejected() {
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
    fn pause_is_not_acked_before_the_consumer_acknowledges() {
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
    fn ack_pause_sends_pause_device_complete_exactly_once() {
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
    fn force_and_gone_send_no_ack() {
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
    fn two_paused_devices_get_two_acks() {
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
    fn resume_carries_the_received_fd() {
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
    fn device_pause_does_not_clear_session_activation() {
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
    fn resume_drops_paused_device_state_without_reactivating_a_session() {
        let fake = Rc::new(FakeBus::new());
        fake.active.set(false);
        let bus: Rc<dyn Bus> = fake.clone();
        let session = LogindSession::open_with_bus_waiting(bus, None, None, ActiveWait::IMMEDIATE)
            .expect("session");
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
    fn gone_devices_are_not_tracked_as_paused() {
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
    fn paused_take_device_does_not_clear_session_activation() {
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
        // /dev/null is 1:5, so that is the key logind paused.
        assert_eq!(session.paused_devices(), vec![(1, 5)]);
    }

    #[test]
    fn release_control_is_idempotent() {
        let (session, fake) = session();
        session.release_control().expect("release");
        session.release_control().expect("release again");
        assert_eq!(fake.count(&Call::ReleaseControl), 1);
    }

    #[test]
    fn dropping_the_session_releases_control() {
        let fake = Rc::new(FakeBus::new());
        let bus: Rc<dyn Bus> = fake.clone();
        {
            let _session = LogindSession::open_with_bus(bus, None, None).expect("session");
            assert_eq!(fake.count(&Call::ReleaseControl), 0);
        }
        assert_eq!(fake.count(&Call::ReleaseControl), 1);
    }

    #[test]
    fn pause_kinds_carry_their_ack_requirement() {
        assert!(PauseKind::from_signal("whatever").is_none());
        assert!(PauseKind::from_signal("pause").expect("pause").needs_ack());
        assert!(!PauseKind::from_signal("force").expect("force").needs_ack());
        assert!(!PauseKind::from_signal("gone").expect("gone").needs_ack());
        assert_eq!(PauseKind::Gone.name(), "gone");
    }
}
