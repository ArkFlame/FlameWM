use super::*;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};

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

pub(super) fn check_stop(stop_requested: &AtomicBool) -> Result<(), String> {
    if stop_requested.load(Ordering::Relaxed) {
        Err(SHUTDOWN_SIGNAL.to_owned())
    } else {
        Ok(())
    }
}

pub(super) fn check_parent(parent: rustix::process::Pid, stop_requested: &AtomicBool) -> Result<(), String> {
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
    while Instant::now() < deadline {
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
        if Instant::now() >= deadline {
            break;
        }
        let ewmh = ewmh_ready();
        if Instant::now() >= deadline {
            break;
        }
        match ewmh {
            Ok(true) => {
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
                if Instant::now() >= deadline {
                    break;
                }
            }
            Ok(false) => "EWMH supporting-WM check is not valid".clone_into(&mut last_error),
            Err(error) => last_error = format!("EWMH readiness: {error}"),
        }
        thread::sleep(TICK.min(deadline.saturating_duration_since(Instant::now())));
    }
    Err(format!(
        "WindowManager readiness timed out after {} seconds ({}): {last_error}",
        READY_TIMEOUT.as_secs(),
        wm.plan.program.display()
    ))
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

pub(super) fn supporting_wm_check_is_valid(root_support: Option<u32>, self_support: Option<u32>) -> bool {
    matches!(root_support, Some(support) if support != 0 && self_support == Some(support))
}
