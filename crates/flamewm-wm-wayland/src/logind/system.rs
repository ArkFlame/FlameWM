use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};
use std::time::Duration;

use dbus::arg::OwnedFd as DbusFd;
use dbus::blocking::Connection;
use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
use dbus::message::MatchRule;

use super::{Bus, Event, EventQueue, LogindError, PauseKind};

pub(super) const LOGIN: &str = "org.freedesktop.login1";
pub(super) const MANAGER_PATH: &str = "/org/freedesktop/login1";
pub(super) const MANAGER_IF: &str = "org.freedesktop.login1.Manager";
pub(super) const SESSION_IF: &str = "org.freedesktop.login1.Session";
pub(super) const SEAT_IF: &str = "org.freedesktop.login1.Seat";
pub(super) const TIMEOUT: Duration = Duration::from_secs(5);

/// The system-bus implementation of [`Bus`].
pub(super) struct SystemBus {
    pub(super) conn: Connection,
}

pub(super) fn object_path(path: &str) -> Result<dbus::Path<'static>, LogindError> {
    dbus::Path::new(path)
        .map(dbus::Path::into_static)
        .map_err(|error| LogindError(format!("invalid object path {path}: {error}")))
}

impl SystemBus {
    pub(super) fn connect() -> Result<Self, LogindError> {
        Ok(Self {
            conn: Connection::new_system()?,
        })
    }

    pub(super) fn session_property<R: for<'b> dbus::arg::Get<'b> + 'static>(
        &self,
        session_path: &str,
        property: &str,
    ) -> Result<R, LogindError> {
        let session = self
            .conn
            .with_proxy(LOGIN, object_path(session_path)?, TIMEOUT);
        Ok(session.get(SESSION_IF, property)?)
    }

    pub(super) fn session_call<A: dbus::arg::AppendAll>(
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

    /// `org.freedesktop.login1.Session.Activate()`, no arguments: makes this session the active
    /// one on its seat, which is what `TakeControl` deliberately does not do.
    fn activate_session(&self, session_path: &str) -> Result<(), LogindError> {
        self.session_call(session_path, "Activate", ())
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
        let rule = MatchRule::new_signal(SESSION_IF, "PauseDevice")
            .with_sender(LOGIN)
            .with_path(path.clone());
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

        let rule = MatchRule::new_signal(SESSION_IF, "ResumeDevice")
            .with_sender(LOGIN)
            .with_path(path.clone());
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

        let rule = MatchRule::new_signal(SESSION_IF, "Activate")
            .with_sender(LOGIN)
            .with_path(path);
        let activate_queue = queue;
        self.conn.add_match(rule, move |(), _, _| {
            activate_queue.push(Event::ActivateSession);
            true
        })?;
        Ok(())
    }

    fn pump(&self) {
        // Drain everything libdbus already buffered: the fd only reports data the kernel holds.
        loop {
            match self.conn.process(Duration::ZERO) {
                Ok(true) => {}
                Ok(false) => break,
                Err(error) => {
                    eprintln!("flamewm-wayland: logind bus error: {error}");
                    break;
                }
            }
        }
    }

    fn watch_fd(&self) -> Option<std::os::fd::RawFd> {
        Some(self.conn.channel().watch().fd)
    }
}
