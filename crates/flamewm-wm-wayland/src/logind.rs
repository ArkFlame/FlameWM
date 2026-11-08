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
//! the session is being set up. `TakeControl` does not change that — it only hands us the devices —
//! so this module activates the session itself, the way libseat (cosmic-comp) and wlroots' logind
//! backend do: validation polls `Active` for a short bounded time (see [`ActiveWait`]), then
//! `org.freedesktop.login1.Session.Activate()` is called on our own session path, plus
//! `org.freedesktop.login1.Seat.SwitchTo(XDG_VTNR)` when the session has a VT, and `Active` is
//! polled again. The whole wait is bounded at ~5 s. Skipping the activation is what leaves a
//! compositor holding DRM master on a VT that still belongs to the greeter: the CRTC is taken and
//! blanked, nothing is ever presented and our input/VT handling is never live.
//!
//! A session that is still not active after that makes opening the session FAIL
//! (`reason=inactive_after_activate`) instead of continuing: the tty backend returns before taking
//! DRM master, so the user gets a greeter-visible error instead of a dead black VT. Sessions that
//! are not ours (another user, another seat, a greeter) are rejected while inactive even earlier:
//! taking control of them would steal somebody else's devices.
//!
//! `is_active()` is session activation and nothing else (`ActivateSession` sets it, validation seeds
//! it). A per-device pause — the touchpad or keyboard going away, which logind also does for reasons
//! that have nothing to do with a VT switch — is tracked separately in [`Inner::paused_devices`] and
//! never clears it. Stopping scanout on a pause is the tty backend's own decision, so a single device
//! pause can no longer strand the session in a permanently inactive state.

#![cfg_attr(not(feature = "drm"), allow(unused_imports))]

use std::cell::RefCell;
use std::collections::{BTreeSet, VecDeque};
use std::error::Error as StdError;
use std::fmt;
use std::fs::File;
use std::os::fd::{OwnedFd, RawFd};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(feature = "drm")]
mod system;
#[cfg(test)]
mod tests;

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
    /// `org.freedesktop.login1.Session.Activate()`: makes this session the active one on its seat.
    fn activate_session(&self, session_path: &str) -> Result<(), LogindError>;
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
    /// Readable whenever bus data waits; `None` when the bus has no descriptor (tests).
    fn watch_fd(&self) -> Option<RawFd> {
        None
    }
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
    /// is then activated explicitly (see [`activate`]) or the open fails.
    active: bool,
    /// Short log token describing what the activation wait decided, for the startup log.
    note: Option<&'static str>,
}

/// Bounded wait for the `Active` property, in logind property reads.
///
/// libseat blocks until the seat is active, and a compositor is routinely started while the VT still
/// belongs to the greeter. Ten reads over three seconds cover a normal session setup; after that the
/// session is activated explicitly and one more short wait decides whether to proceed or fail.
#[derive(Clone, Copy, Debug)]
struct ActiveWait {
    attempts: usize,
    interval: Duration,
}

impl ActiveWait {
    /// The validation budget: 10 × 300 ms.
    const DEFAULT: Self = Self {
        attempts: 10,
        interval: Duration::from_millis(300),
    };

    /// The budget after `Session.Activate` + `Seat.SwitchTo`: 20 × 100 ms, so validation and
    /// activation together stay bounded at ~5 s.
    const AFTER_ACTIVATE: Self = Self {
        attempts: 20,
        interval: Duration::from_millis(100),
    };

    /// The same number of reads without the pauses, so the tests exercise the polling loop itself.
    #[cfg(test)]
    const IMMEDIATE: Self = Self {
        attempts: 20,
        interval: Duration::ZERO,
    };
}

/// Polls the `Active` property up to `wait.attempts` times. A read that fails counts as "not active
/// yet": the caller decides whether that is a failure (see [`activate`]).
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

/// The VT this session owns, from `XDG_VTNR`, when it is set and parses.
///
/// A number below 1 is not a VT and is ignored rather than handed to `Seat.SwitchTo`.
fn vt_number() -> Option<u32> {
    parse_vt(std::env::var("XDG_VTNR").ok().as_deref())
}

/// [`vt_number`] without the environment, so the parsing is testable.
fn parse_vt(value: Option<&str>) -> Option<u32> {
    value
        .map(str::trim)
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|vt| *vt > 0)
}

/// Asks logind to make this session the active one on its seat, then waits for `Active`.
///
/// `TakeControl` does not activate the session. libseat (cosmic-comp) and wlroots' logind backend
/// call `Session.Activate` — and `Seat.SwitchTo` when the session has a VT — as part of opening it.
/// A compositor that skips this takes DRM master on a VT that still belongs to the greeter: the CRTC
/// is taken and blanked, no frame is ever presented and input/VT handling is not live.
///
/// Returns whether the session reported `Active` within `wait`. Both calls are best effort at this
/// level (a failure is logged); the caller fails the open when the answer is still `false`.
fn activate(
    bus: &dyn Bus,
    session_path: &str,
    seat_path: &str,
    vt: Option<u32>,
    wait: ActiveWait,
) -> bool {
    match bus.activate_session(session_path) {
        Ok(()) => {
            eprintln!("flamewm-wayland: logind session {session_path} detail=activate_requested");
        }
        Err(error) => eprintln!("flamewm-wayland: logind Session.Activate failed: {error}"),
    }
    if let Some(vt) = vt {
        match bus.switch_to(seat_path, vt) {
            Ok(()) => eprintln!(
                "flamewm-wayland: logind session {session_path} detail=switched_to_vt{vt}"
            ),
            Err(error) => {
                eprintln!("flamewm-wayland: logind Seat.SwitchTo({vt}) failed: {error}");
            }
        }
    }
    if wait_for_active(bus, session_path, wait) {
        eprintln!("flamewm-wayland: logind session {session_path} detail=activated");
        true
    } else {
        eprintln!("flamewm-wayland: logind session {session_path} detail=still_inactive");
        false
    }
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
    if let Some(expected) = expected_seat.map(str::trim).filter(|seat| !seat.is_empty())
        && expected != seat_name
    {
        return Err(LogindError::rejected(
            "wrong-seat",
            format!("XDG_SEAT={expected} but session {session_path} is on seat {seat_name}"),
        ));
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
        }
        // Still inactive: the caller activates the session explicitly and fails the open when that
        // does not help (see [`activate`]). There is no "continue inactive" path.
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

    /// Session discovery, validation, `TakeControl`, activation and signal subscription over any
    /// [`Bus`].
    fn open_with_bus(
        bus: Rc<dyn Bus>,
        session_id: Option<&str>,
        expected_seat: Option<&str>,
    ) -> Result<Self, LogindError> {
        Self::open_with_bus_waiting(
            bus,
            session_id,
            expected_seat,
            ActiveWait::DEFAULT,
            ActiveWait::AFTER_ACTIVATE,
            vt_number(),
        )
    }

    /// [`Self::open_with_bus`] with an explicit activation budget and VT, so tests do not sleep and
    /// do not depend on the ambient environment.
    fn open_with_bus_waiting(
        bus: Rc<dyn Bus>,
        session_id: Option<&str>,
        expected_seat: Option<&str>,
        wait: ActiveWait,
        after_activate: ActiveWait,
        vt: Option<u32>,
    ) -> Result<Self, LogindError> {
        let session_path = resolve_session(bus.as_ref(), session_id)?;
        let validated = validate_session(bus.as_ref(), &session_path, expected_seat, wait)?;
        bus.take_control(&session_path).map_err(|error| {
            LogindError(format!(
                "logind TakeControl on {session_path} failed; run from the user's own graphical session: {error}"
            ))
        })?;
        // Control is ours from here on, so every failure below has to give it back: the guard owns
        // the release (`Inner::drop`), exactly like a running session does.
        let inner = Rc::new(RefCell::new(Inner {
            bus: Rc::clone(&bus),
            session_path: session_path.clone(),
            seat_name: validated.seat_name.clone(),
            seat_path: validated.seat_path.clone(),
            active: validated.active,
            controlled: true,
            pending: Vec::new(),
            paused_devices: BTreeSet::new(),
        }));
        let events = EventQueue::new();
        // Subscribed before the activation below, so an `Activate` signal that arrives while we wait
        // is queued and still flips `is_active()` later.
        bus.subscribe(&session_path, events.clone())?;
        if let Some(note) = validated.note {
            eprintln!("flamewm-wayland: logind session {session_path} detail={note}");
        }
        // `TakeControl` only hands over the devices: the session has to be activated explicitly,
        // like libseat and wlroots' logind backend do. Fail closed when it still will not activate:
        // continuing would take DRM master on a VT that belongs to the greeter (black screen, no
        // frames, no VT switches) instead of leaving a diagnosable failure behind.
        if !validated.active {
            if !activate(
                bus.as_ref(),
                &session_path,
                &validated.seat_path,
                vt,
                after_activate,
            ) {
                return Err(LogindError::rejected(
                    "inactive_after_activate",
                    format!(
                        "session {session_path} is still not active after Session.Activate and Seat.SwitchTo"
                    ),
                ));
            }
            inner.borrow_mut().active = true;
        }
        Ok(Self { inner, events })
    }

    /// Dispatches pending bus messages without blocking. Callbacks only enqueue.
    pub fn pump(&self) {
        let bus = Rc::clone(&self.inner.borrow().bus);
        bus.pump();
    }

    /// The bus descriptor, so the caller can sleep until logind has something to say.
    pub fn bus_fd(&self) -> Option<RawFd> {
        self.inner.borrow().bus.watch_fd()
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

    /// Test-only: pretend logind took the session away without pausing a device, so the resume path
    /// can be checked against an inactive session without a seat.
    #[cfg(test)]
    fn clear_active_flag(&self) {
        self.inner.borrow_mut().active = false;
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
