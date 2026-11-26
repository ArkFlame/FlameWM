use super::*;
use std::path::Path;
// `rustix/event` arrives through the workspace's feature unification (flamewm-integrations-linux
// enables it); polling the X11 socket and the wakeup pipe is what replaces the former sleep-poll.
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::io::Errno;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ChangeWindowAttributesAux, ConnectionExt, EventMask};
use x11rb::rust_connection::RustConnection;

/// Shortest readiness wait slice: a zero remainder still blocks once in `poll` before re-checking.
const MINIMUM_WAIT: Duration = Duration::from_millis(1);

/// Read end of the self-pipe written by the signal handlers for SIGCHLD and stop signals.
pub(super) static WAKEUP: OnceLock<UnixStream> = OnceLock::new();

pub(super) fn register_wakeups() -> Result<(), String> {
    let (reader, writer) =
        UnixStream::pair().map_err(|error| format!("create supervisor wakeup pipe: {error}"))?;
    for signal in [SIGCHLD, SIGTERM, SIGINT, SIGHUP] {
        let fd = writer
            .try_clone()
            .map_err(|error| format!("duplicate supervisor wakeup pipe: {error}"))?;
        pipe::register(signal, fd)
            .map_err(|error| format!("register supervisor wakeup for signal {signal}: {error}"))?;
    }
    WAKEUP
        .set(reader)
        .map_err(|_| "supervisor wakeup pipe registered twice".to_owned())
}

/// Blocks until a child exits, a stop signal arrives, or `timeout` elapses (`None` waits indefinitely).
pub(super) fn wait_for_wakeup(timeout: Option<Duration>) -> Result<(), String> {
    let reader = WAKEUP
        .get()
        .ok_or_else(|| "supervisor wakeup pipe is not registered".to_owned())?;
    reader
        .set_read_timeout(timeout.map(|duration| duration.max(Duration::from_millis(1))))
        .map_err(|error| format!("configure supervisor wakeup: {error}"))?;
    let mut buffer = [0u8; 64];
    match (&*reader).read(&mut buffer) {
        Ok(0) => Err("supervisor wakeup pipe closed".to_owned()),
        Ok(_) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(format!("wait for supervisor wakeup: {error}")),
    }
}

/// Event source for window-manager readiness: one persistent X11 connection whose root window is
/// selected for property changes, so the socket becomes readable exactly when
/// `_NET_SUPPORTING_WM_CHECK` can change. Replaces the former 100 ms sleep-poll of that property.
pub(super) struct ReadinessWatch {
    connection: RustConnection,
    screen_num: usize,
}

impl ReadinessWatch {
    fn open() -> Option<Self> {
        let (connection, screen_num) = x11rb::connect(None).ok()?;
        let root = connection.setup().roots.get(screen_num)?.root;
        connection
            .change_window_attributes(
                root,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            )
            .ok()?;
        connection.flush().ok()?;
        Some(Self {
            connection,
            screen_num,
        })
    }

    fn connection(&self) -> &RustConnection {
        &self.connection
    }

    fn ewmh_ready(&self) -> Result<bool, String> {
        ewmh_ready_on(&self.connection, self.screen_num)
    }

    /// Drops queued property events: the loop re-reads the property itself after every wakeup.
    fn drain_events(&self) {
        while matches!(self.connection.poll_for_event(), Ok(Some(_))) {}
    }
}

/// Block length for one readiness wait: bounded by the remaining readiness budget and by the
/// supervisor's missed-wakeup bound, so a dropped signal-hook byte costs one cheap re-check.
pub(super) fn readiness_wait_timeout(remaining: Duration) -> Duration {
    remaining.clamp(MINIMUM_WAIT, SUPERVISOR_SAFETY_TIMEOUT)
}

/// Blocks until readiness can change (root-window property event), a child exits or a stop signal
/// arrives, or `timeout` elapses. Returns `false` when the X11 watch socket is gone and must reopen.
pub(super) fn wait_for_ready_event(
    watch: Option<&ReadinessWatch>,
    timeout: Duration,
) -> Result<bool, String> {
    let reader = WAKEUP
        .get()
        .ok_or_else(|| "supervisor wakeup pipe is not registered".to_owned())?;
    if let Some(watch) = watch {
        watch.drain_events();
    }
    let mut fds = Vec::with_capacity(2);
    if let Some(watch) = watch {
        fds.push(PollFd::new(watch.connection().stream(), PollFlags::IN));
    }
    fds.push(PollFd::new(reader, PollFlags::IN));
    let timeout = Timespec {
        tv_sec: timeout.as_secs() as _,
        tv_nsec: timeout.subsec_nanos().into(),
    };
    loop {
        match poll(&mut fds, Some(&timeout)) {
            Ok(_) => break,
            Err(Errno::INTR) => {}
            Err(error) => return Err(format!("wait for readiness event: {error}")),
        }
    }
    let wakeup = fds.last().is_some_and(|fd| {
        fd.revents()
            .intersects(PollFlags::IN | PollFlags::HUP | PollFlags::ERR)
    });
    let x11_gone = watch.is_some()
        && fds.first().is_some_and(|fd| {
            fd.revents()
                .intersects(PollFlags::HUP | PollFlags::ERR | PollFlags::NVAL)
        });
    if wakeup {
        // Drain the signal bytes so the next poll blocks again; a closed pipe stays an error.
        wait_for_wakeup(Some(MINIMUM_WAIT))?;
    }
    Ok(!x11_gone)
}

pub(super) fn check_stop(stop_requested: &AtomicBool) -> Result<(), String> {
    if stop_requested.load(Ordering::Relaxed) {
        Err(SHUTDOWN_SIGNAL.to_owned())
    } else {
        Ok(())
    }
}

pub(super) fn check_parent(
    parent: rustix::process::Pid,
    stop_requested: &AtomicBool,
) -> Result<(), String> {
    check_stop(stop_requested)?;
    if getppid() == Some(parent) {
        Ok(())
    } else {
        Err("session parent exited or is unavailable; stopping session children".to_owned())
    }
}

pub(super) fn startup_grace(
    child: &mut Running,
    parent: rustix::process::Pid,
    stop_requested: &AtomicBool,
) -> Result<(), String> {
    let until = Instant::now() + STARTUP_GRACE;
    while Instant::now() < until {
        check_parent(parent, stop_requested)?;
        if let Some(status) = child
            .child
            .try_wait()
            .map_err(|error| child_error(child, "check startup", error))?
        {
            return Err(format!(
                "{:?} at {} exited during startup: {status}",
                child.plan.role,
                child.plan.program.display()
            ));
        }
        wait_for_wakeup(Some(until.saturating_duration_since(Instant::now())))?;
    }
    Ok(())
}

pub(super) fn wait_ready(
    wm: &mut Running,
    parent: rustix::process::Pid,
    stop_requested: &AtomicBool,
) -> Result<(), String> {
    let deadline = Instant::now() + READY_TIMEOUT;
    let mut last_error = String::from("readiness not yet available");
    let mut ewmh_seen = false;
    let mut watch = ReadinessWatch::open();
    loop {
        if Instant::now() >= deadline {
            break;
        }
        check_parent(parent, stop_requested)?;
        if let Some(status) = wm
            .child
            .try_wait()
            .map_err(|error| child_error(wm, "check WM readiness", error))?
        {
            return Err(format!(
                "WindowManager at {} exited before readiness: {status}",
                wm.plan.program.display()
            ));
        }
        if watch.is_none() {
            watch = ReadinessWatch::open();
        }
        let ewmh = match watch.as_ref() {
            Some(watch) => watch.ewmh_ready(),
            None => ewmh_ready(),
        };
        if ewmh.is_err() {
            // A broken watch connection is reopened on the next pass instead of spinning on it.
            watch = None;
        }
        match ewmh {
            Ok(true) => {
                ewmh_seen = true;
                if session_bus_absent() {
                    session_diagnostics::log(
                        "window manager",
                        "no session D-Bus address reachable; continuing without session control",
                    );
                    return Ok(());
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match control_ready(remaining) {
                    Ok(true) if Instant::now() < deadline => return Ok(()),
                    Ok(true) => break,
                    Ok(false) => {
                        "session control returned a non-bootstrap response"
                            .clone_into(&mut last_error);
                    }
                    Err(error) => last_error = format!("session control bootstrap: {error}"),
                }
            }
            Ok(false) => "EWMH supporting-WM check is not valid".clone_into(&mut last_error),
            Err(error) => last_error = format!("EWMH readiness: {error}"),
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        if !wait_for_ready_event(
            watch.as_ref(),
            if ewmh_seen {
                // The control socket has no event source: keep the short retry once the WM is up.
                readiness_wait_timeout(remaining).min(Duration::from_millis(100))
            } else {
                readiness_wait_timeout(remaining)
            },
        )? {
            watch = None;
        }
    }
    readiness_timeout(
        ewmh_seen,
        format!(
            "WindowManager readiness timed out after {} seconds ({}): {last_error}",
            READY_TIMEOUT.as_secs(),
            wm.plan.program.display()
        ),
    )
}

/// Session control is optional: once EWMH reports the WM ready, a missing or silent control bus
/// degrades the session (logged) instead of aborting it before the desktop and shell start.
pub(super) fn readiness_timeout(ewmh_ready: bool, message: String) -> Result<(), String> {
    if ewmh_ready {
        session_diagnostics::log(
            "window manager",
            &format!("{message}; continuing without session control"),
        );
        Ok(())
    } else {
        Err(message)
    }
}

/// No-bus fast path: true when no session bus can be reached at all, so waiting for the
/// control bootstrap would only burn `READY_TIMEOUT`. Checked from the environment, not by connecting.
pub(super) fn session_bus_absent() -> bool {
    let address = std::env::var("DBUS_SESSION_BUS_ADDRESS")
        .ok()
        .filter(|value| !value.is_empty());
    let uid = rustix::process::getuid().as_raw();
    session_bus_absent_for(
        address.as_deref(),
        Path::new(&format!("/run/user/{uid}/bus")),
        Path::exists,
    )
}

pub(super) fn session_bus_absent_for(
    address: Option<&str>,
    default_socket: &Path,
    exists: impl Fn(&Path) -> bool,
) -> bool {
    match address {
        None => !exists(default_socket),
        Some(address) => !address
            .split(';')
            .any(|entry| match entry.strip_prefix("unix:") {
                // A unix:path= entry is reachable only if its socket file exists; abstract sockets
                // and other transports cannot be checked cheaply, so they count as possibly reachable.
                Some(params) => params
                    .split(',')
                    .find_map(|pair| pair.strip_prefix("path="))
                    .is_none_or(|path| exists(Path::new(path))),
                None => true,
            }),
    }
}

pub(super) fn control_ready(timeout: Duration) -> Result<bool, String> {
    if timeout.is_zero() {
        return Ok(false);
    }
    let client = ControlClient::connect(BusKind::Session).map_err(|error| error.to_string())?;
    match client
        .call_with_timeout(&ControlRequest::GetShellBootstrap, timeout)
        .map_err(|error| error.message)?
    {
        ControlResponse::ShellBootstrap(_) => Ok(true),
        _ => Ok(false),
    }
}

pub(super) fn ewmh_ready() -> Result<bool, String> {
    let (connection, screen_num) = x11rb::connect(None).map_err(|error| error.to_string())?;
    ewmh_ready_on(&connection, screen_num)
}

fn ewmh_ready_on(connection: &RustConnection, screen_num: usize) -> Result<bool, String> {
    let root = connection
        .setup()
        .roots
        .get(screen_num)
        .ok_or_else(|| "X11 screen missing".to_owned())?
        .root;
    let atom = |name: &[u8]| -> Result<u32, String> {
        connection
            .intern_atom(false, name)
            .map_err(|error| error.to_string())?
            .reply()
            .map(|reply| reply.atom)
            .map_err(|error| error.to_string())
    };
    let property = atom(b"_NET_SUPPORTING_WM_CHECK")?;
    let first = connection
        .get_property(false, root, property, AtomEnum::WINDOW, 0, 1)
        .map_err(|error| error.to_string())?
        .reply()
        .map_err(|error| error.to_string())?;
    let root_support = first.value32().and_then(|mut values| values.next());
    let Some(support) = root_support else {
        return Ok(false);
    };
    let second = connection
        .get_property(false, support, property, AtomEnum::WINDOW, 0, 1)
        .map_err(|error| error.to_string())?
        .reply()
        .map_err(|error| error.to_string())?;
    Ok(supporting_wm_check_is_valid(
        root_support,
        second.value32().and_then(|mut values| values.next()),
    ))
}

pub(super) fn supporting_wm_check_is_valid(
    root_support: Option<u32>,
    self_support: Option<u32>,
) -> bool {
    matches!(root_support, Some(support) if support != 0 && self_support == Some(support))
}

#[cfg(test)]
mod tests {
    use super::{readiness_timeout, readiness_wait_timeout};
    use std::path::Path;
    use std::time::Duration;

    #[test]
    fn readiness_wait_is_deadline_and_fallback_bounded() {
        assert_eq!(
            readiness_wait_timeout(Duration::from_secs(30)),
            Duration::from_secs(1)
        );
        assert_eq!(
            readiness_wait_timeout(Duration::from_millis(250)),
            Duration::from_millis(250)
        );
        assert_eq!(
            readiness_wait_timeout(Duration::ZERO),
            Duration::from_millis(1)
        );
    }

    #[test]
    fn readiness_timeout_is_fatal_only_before_ewmh_is_ready() {
        assert!(readiness_timeout(false, "timed out".to_owned()).is_err());
        assert!(readiness_timeout(true, "control bus unavailable".to_owned()).is_ok());
    }

    #[test]
    fn session_bus_absent_when_no_address_and_no_default_socket() {
        let missing = Path::new("/run/user/1000/bus");
        assert!(super::session_bus_absent_for(None, missing, |_| false));
        assert!(!super::session_bus_absent_for(None, missing, |_| true));
    }

    #[test]
    fn session_bus_present_when_address_socket_exists_or_is_not_checkable() {
        let missing = Path::new("/run/user/1000/bus");
        assert!(!super::session_bus_absent_for(
            Some("unix:path=/tmp/bus"),
            missing,
            |path| path == Path::new("/tmp/bus")
        ));
        assert!(super::session_bus_absent_for(
            Some("unix:path=/tmp/gone"),
            missing,
            |_| false
        ));
        assert!(!super::session_bus_absent_for(
            Some("unix:abstract=/tmp/x"),
            missing,
            |_| false
        ));
    }
}
