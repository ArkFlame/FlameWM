use std::collections::BTreeMap;
use std::env;
use std::io::{ErrorKind, Read};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus};
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt;
use std::sync::{Arc, OnceLock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use flamewm_control_core::{ControlRequest, ControlResponse};
use flamewm_control_dbus::ControlClient;
use flamewm_dbus_reactor::BusKind;
use flamewm_session_core::{
    ChildExit, ProcessPlan, ProcessSupervisorPolicy, SessionRole, SupervisorDecision,
    build_launch_plan, default_paths_from_home,
};
use rustix::process::{Pid, Signal, getppid, kill_process, set_parent_process_death_signal};
use signal_hook::consts::signal::{SIGCHLD, SIGHUP, SIGINT, SIGTERM};
use signal_hook::flag;
use signal_hook::low_level::pipe;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};

const READY_TIMEOUT: Duration = Duration::from_secs(30);
const TICK: Duration = Duration::from_millis(100);
const STARTUP_GRACE: Duration = Duration::from_millis(500);

struct Running {
    plan: ProcessPlan,
    child: Child,
    supervisor: ProcessSupervisorPolicy,
}

fn main() {
    if let Err(error) = execute() {
        eprintln!("flamewm-session: {error}");
        std::process::exit(1);
    }
}

fn execute() -> Result<(), String> {
    let stop_requested = Arc::new(AtomicBool::new(false));
    for signal in [SIGTERM, SIGINT, SIGHUP] {
        flag::register(signal, Arc::clone(&stop_requested))
            .map_err(|error| format!("register session shutdown signal {signal}: {error}"))?;
    }
    register_wakeups()?;
    let parent = getppid().ok_or_else(|| "session has no parent process".to_owned())?;
    set_parent_process_death_signal(Some(Signal::CHILD))
        .map_err(|error| format!("configure parent-death notification: {error}"))?;
    if getppid() != Some(parent) {
        return Err("session parent exited during supervisor setup".to_owned());
    }

    let executable = env::current_exe().map_err(|error| format!("locate session executable: {error}"))?;
    let bindir = executable.parent().ok_or_else(|| "session executable has no parent directory".to_owned())?.to_path_buf();
    let home = env::var_os("HOME").map_or_else(|| PathBuf::from("/root"), PathBuf::from);
    let mut paths = default_paths_from_home(&bindir, home);
    if let Some(config) = env::var_os("XDG_CONFIG_HOME") {
        paths.config_home = PathBuf::from(config);
    }
    let existing = env::vars().collect::<BTreeMap<_, _>>();
    let args = env::args().skip(1).collect::<Vec<_>>();
    let plan = build_launch_plan(&paths, &args, &existing);
    if plan.processes.len() != 3
        || plan.processes[0].role != SessionRole::WindowManager
        || plan.processes[1].role != SessionRole::Desktop
        || plan.processes[2].role != SessionRole::Shell
    {
        return Err("session launch plan does not contain WM, Desktop, Shell in required order".to_owned());
    }
    let role_plans = plan.processes;
    let wm = role_plans[0].clone();
    let desktop = role_plans[1].clone();
    let shell = role_plans[2].clone();
    let mut running: Vec<Running> = Vec::new();

    let result = (|| {
        check_stop(&stop_requested)?;
        running.push(spawn(wm)?);
        wait_ready(&mut running[0], parent, &stop_requested)?;
        check_stop(&stop_requested)?;
        running.push(spawn(desktop)?);
        startup_grace(&mut running[1], parent, &stop_requested)?;
        check_stop(&stop_requested)?;
        running.push(spawn(shell)?);
        startup_grace(&mut running[2], parent, &stop_requested)?;
        supervise(&mut running, parent, &role_plans, &stop_requested)
    })();
    let cleanup = stop_all(&mut running);
    match (result, cleanup) {
        (Err(error), Ok(())) if error == "session shutdown signal received" => Ok(()),
        (Err(error), Err(cleanup)) => Err(format!("{error}; cleanup failed: {cleanup}")),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(cleanup)) => Err(format!("cleanup failed: {cleanup}")),
        (Ok(()), Ok(())) => Ok(()),
    }
}

const ALLOCATOR_TUNING: [(&str, &str); 3] = [
    ("MALLOC_ARENA_MAX", "2"),
    ("MALLOC_TRIM_THRESHOLD_", "131072"),
    ("MALLOC_MMAP_THRESHOLD_", "131072"),
];

/// Allocator-tuning pairs whose key is not already set; a user-provided value always wins.
fn allocator_tuning_defaults(is_set: impl Fn(&str) -> bool) -> Vec<(&'static str, &'static str)> {
    ALLOCATOR_TUNING.into_iter().filter(|(key, _)| !is_set(key)).collect()
}

fn spawn(plan: ProcessPlan) -> Result<Running, String> {
    let mut command = Command::new(&plan.program);
    command.args(&plan.arguments).envs(&plan.environment);
    for (key, value) in allocator_tuning_defaults(|key| env::var_os(key).is_some() || plan.environment.contains_key(key)) {
        command.env(key, value);
    }
    let child = command.spawn().map_err(|error| format!("spawn {:?} at {}: {error}", plan.role, plan.program.display()))?;
    Ok(Running { plan, child, supervisor: ProcessSupervisorPolicy::default() })
}

/// Read end of the self-pipe written by the signal handlers for SIGCHLD and stop signals.
static WAKEUP: OnceLock<UnixStream> = OnceLock::new();

fn register_wakeups() -> Result<(), String> {
    let (reader, writer) = UnixStream::pair().map_err(|error| format!("create supervisor wakeup pipe: {error}"))?;
    for signal in [SIGCHLD, SIGTERM, SIGINT, SIGHUP] {
        let fd = writer.try_clone().map_err(|error| format!("duplicate supervisor wakeup pipe: {error}"))?;
        pipe::register(signal, fd).map_err(|error| format!("register supervisor wakeup for signal {signal}: {error}"))?;
    }
    WAKEUP.set(reader).map_err(|_| "supervisor wakeup pipe registered twice".to_owned())
}

/// Blocks until a child exits, a stop signal arrives, or `timeout` elapses (`None` waits indefinitely).
fn wait_for_wakeup(timeout: Option<Duration>) -> Result<(), String> {
    let reader = WAKEUP.get().ok_or_else(|| "supervisor wakeup pipe is not registered".to_owned())?;
    reader
        .set_read_timeout(timeout.map(|duration| duration.max(Duration::from_millis(1))))
        .map_err(|error| format!("configure supervisor wakeup: {error}"))?;
    let mut buffer = [0u8; 64];
    match (&*reader).read(&mut buffer) {
        Ok(0) => Err("supervisor wakeup pipe closed".to_owned()),
        Ok(_) => Ok(()),
        Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => Ok(()),
        Err(error) => Err(format!("wait for supervisor wakeup: {error}")),
    }
}

fn check_stop(stop_requested: &AtomicBool) -> Result<(), String> {
    if stop_requested.load(Ordering::Relaxed) { Err("session shutdown signal received".to_owned()) } else { Ok(()) }
}

fn check_parent(parent: rustix::process::Pid, stop_requested: &AtomicBool) -> Result<(), String> {
    check_stop(stop_requested)?;
    if getppid() != Some(parent) { Err("session parent exited or is unavailable; stopping session children".to_owned()) } else { Ok(()) }
}

fn startup_grace(child: &mut Running, parent: rustix::process::Pid, stop_requested: &AtomicBool) -> Result<(), String> {
    let until = Instant::now() + STARTUP_GRACE;
    while Instant::now() < until {
        check_parent(parent, stop_requested)?;
        if let Some(status) = child.child.try_wait().map_err(|error| child_error(child, "check startup", error))? {
            return Err(format!("{:?} at {} exited during startup: {status}", child.plan.role, child.plan.program.display()));
        }
        wait_for_wakeup(Some(until.saturating_duration_since(Instant::now())))?;
    }
    Ok(())
}

fn wait_ready(wm: &mut Running, parent: rustix::process::Pid, stop_requested: &AtomicBool) -> Result<(), String> {
    let deadline = Instant::now() + READY_TIMEOUT;
    let mut last_error = String::from("readiness not yet available");
    while Instant::now() < deadline {
        if Instant::now() >= deadline { break; }
        check_parent(parent, stop_requested)?;
        if let Some(status) = wm.child.try_wait().map_err(|error| child_error(wm, "check WM readiness", error))? {
            return Err(format!("WindowManager at {} exited before readiness: {status}", wm.plan.program.display()));
        }
        if Instant::now() >= deadline { break; }
        let ewmh = ewmh_ready();
        if Instant::now() >= deadline { break; }
        match ewmh {
            Ok(true) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() { break; }
                match control_ready(remaining) {
                Ok(true) if Instant::now() < deadline => return Ok(()),
                Ok(true) => break,
                Ok(false) => last_error = "session control returned a non-bootstrap response".to_owned(),
                Err(error) => last_error = format!("session control bootstrap: {error}"),
                }
                if Instant::now() >= deadline { break; }
            }
            Ok(false) => last_error = "EWMH supporting-WM check is not valid".to_owned(),
            Err(error) => last_error = format!("EWMH readiness: {error}"),
        }
        thread::sleep(TICK.min(deadline.saturating_duration_since(Instant::now())));
    }
    Err(format!("WindowManager readiness timed out after {} seconds ({}): {last_error}", READY_TIMEOUT.as_secs(), wm.plan.program.display()))
}

fn control_ready(timeout: Duration) -> Result<bool, String> {
    if timeout.is_zero() { return Ok(false); }
    let client = ControlClient::connect(BusKind::Session).map_err(|error| error.to_string())?;
    match client.call_with_timeout(&ControlRequest::GetShellBootstrap, timeout).map_err(|error| error.message)? {
        ControlResponse::ShellBootstrap(_) => Ok(true),
        _ => Ok(false),
    }
}

fn ewmh_ready() -> Result<bool, String> {
    let (connection, screen_num) = x11rb::connect(None).map_err(|error| error.to_string())?;
    let root = connection.setup().roots.get(screen_num).ok_or_else(|| "X11 screen missing".to_owned())?.root;
    let atom = |name: &[u8]| -> Result<u32, String> {
        connection.intern_atom(false, name).map_err(|error| error.to_string())?.reply().map(|reply| reply.atom).map_err(|error| error.to_string())
    };
    let property = atom(b"_NET_SUPPORTING_WM_CHECK")?;
    let first = connection.get_property(false, root, property, AtomEnum::WINDOW, 0, 1)
        .map_err(|error| error.to_string())?.reply().map_err(|error| error.to_string())?;
    let root_support = first.value32().and_then(|mut values| values.next());
    let Some(support) = root_support else { return Ok(false); };
    let second = connection.get_property(false, support, property, AtomEnum::WINDOW, 0, 1)
        .map_err(|error| error.to_string())?.reply().map_err(|error| error.to_string())?;
    Ok(supporting_wm_check_is_valid(root_support, second.value32().and_then(|mut values| values.next())))
}

fn supporting_wm_check_is_valid(root_support: Option<u32>, self_support: Option<u32>) -> bool {
    matches!(root_support, Some(support) if support != 0 && self_support == Some(support))
}

#[cfg(test)]
mod tests {
    use super::{allocator_tuning_defaults, supporting_wm_check_is_valid};

    #[test]
    fn allocator_tuning_fills_only_unset_keys() {
        let all = allocator_tuning_defaults(|_| false);
        assert_eq!(
            all,
            vec![
                ("MALLOC_ARENA_MAX", "2"),
                ("MALLOC_TRIM_THRESHOLD_", "131072"),
                ("MALLOC_MMAP_THRESHOLD_", "131072"),
            ]
        );
        let user_arena = allocator_tuning_defaults(|key| key == "MALLOC_ARENA_MAX");
        assert_eq!(user_arena.len(), 2);
        assert!(user_arena.iter().all(|(key, _)| *key != "MALLOC_ARENA_MAX"));
        assert!(allocator_tuning_defaults(|_| true).is_empty());
    }

    #[test]
    fn supporting_wm_check_requires_matching_nonzero_self_reference() {
        assert!(supporting_wm_check_is_valid(Some(42), Some(42)));
        assert!(!supporting_wm_check_is_valid(None, Some(42)));
        assert!(!supporting_wm_check_is_valid(Some(42), None));
        assert!(!supporting_wm_check_is_valid(Some(42), Some(43)));
        assert!(!supporting_wm_check_is_valid(Some(0), Some(0)));
    }
}

fn supervise(children: &mut Vec<Running>, parent: rustix::process::Pid, role_plans: &[ProcessPlan], stop_requested: &AtomicBool) -> Result<(), String> {
        loop {
        check_parent(parent, stop_requested)?;
        for index in (0..children.len()).rev() {
            let Some(status) = children[index].child.try_wait().map_err(|error| child_error(&children[index], "supervise", error))? else { continue; };
            if index == 0 && status.success() {
                children[0].supervisor.request_stop();
                return Ok(());
            }
            let exit = status_to_exit(status);
            match children[index].supervisor.child_exited(exit) {
                SupervisorDecision::RestartAfter(delay) => {
                    if index == 0 {
                        stop_dependents(children)?;
                    } else if index == 1 && children.len() > 2 {
                        stop_role(&mut children[2])?;
                        children.truncate(2);
                    }
                    wait_backoff(delay, parent, stop_requested)?;
                    let plan = role_plans[index].clone();
                    let supervisor = children[index].supervisor.clone();
                    children[index] = spawn(plan)?;
                    children[index].supervisor = supervisor;
                    if index == 0 {
                        wait_ready(&mut children[0], parent, stop_requested)?;
                        // Dependents reuse the original launch plans, strictly after WM readiness.
                        for plan in role_plans.iter().skip(1) {
                            children.push(spawn(plan.clone())?);
                            let child = children.last_mut().ok_or_else(|| {
                                "supervisor invariant violated: spawned dependent is missing after WM restart".to_owned()
                            })?;
                            startup_grace(child, parent, stop_requested)?;
                        }
                    } else {
                        startup_grace(&mut children[index], parent, stop_requested)?;
                        if index == 1 && children.len() == 2 {
                            children.push(spawn(role_plans[2].clone())?);
                            let child = children.last_mut().ok_or_else(|| {
                                "supervisor invariant violated: spawned shell is missing after desktop restart".to_owned()
                            })?;
                            startup_grace(child, parent, stop_requested)?;
                        }
                    }
                }
                SupervisorDecision::GiveUp => return Err(format!("{:?} at {} exhausted its restart budget", children[index].plan.role, children[index].plan.program.display())),
                SupervisorDecision::StopCleanly => return Ok(()),
                SupervisorDecision::SpawnNow => {
                    return Err(format!(
                        "supervisor invariant violated: {:?} at {} requested immediate spawn after exit",
                        children[index].plan.role,
                        children[index].plan.program.display()
                    ));
                }
            }
            break;
        }
            wait_for_wakeup(None)?;
    }
}

fn wait_backoff(delay_ms: u64, parent: rustix::process::Pid, stop_requested: &AtomicBool) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_millis(delay_ms);
    while Instant::now() < deadline {
        check_parent(parent, stop_requested)?;
        wait_for_wakeup(Some(deadline.saturating_duration_since(Instant::now())))?;
    }
    check_parent(parent, stop_requested)
}

fn stop_dependents(children: &mut Vec<Running>) -> Result<(), String> {
    let mut failures = Vec::new();
    while children.len() > 1 { if let Some(mut child) = children.pop() { if let Err(error) = stop_role(&mut child) { failures.push(error); } } }
    if failures.is_empty() { Ok(()) } else { Err(failures.join("; ")) }
}

fn stop_all(children: &mut Vec<Running>) -> Result<(), String> {
    let mut failures = Vec::new();
    while let Some(mut child) = children.pop() {
        child.supervisor.request_stop();
        if let Err(error) = stop_role(&mut child) { failures.push(error); }
    }
    if failures.is_empty() { Ok(()) } else { Err(failures.join("; ")) }
}

fn stop_role(child: &mut Running) -> Result<(), String> {
    child.supervisor.request_stop();
    let mut failures = Vec::new();
    if let Some(pid) = Pid::from_raw(child.child.id() as i32) {
        if let Err(error) = kill_process(pid, Signal::TERM) {
            if error != rustix::io::Errno::SRCH { failures.push(format!("terminate {:?}: {error}", child.plan.role)); }
        }
    }
    let deadline = Instant::now() + Duration::from_millis(child.supervisor.graceful_stop_ms());
    let mut exited = false;
    loop {
        match child.child.try_wait() {
            Ok(Some(_)) => {
                exited = true;
                break;
            }
            Ok(None) if Instant::now() < deadline => {
                if let Err(error) = wait_for_wakeup(Some(deadline.saturating_duration_since(Instant::now()))) {
                    failures.push(error);
                    break;
                }
            }
            Ok(None) => break,
            Err(error) => {
                failures.push(child_error(child, "check during graceful stop", error));
                break;
            }
        }
    }
    if !exited {
        if let Err(error) = child.child.kill() {
            if error.kind() != std::io::ErrorKind::NotFound {
                failures.push(format!("force-kill {:?}: {error}", child.plan.role));
            }
        }
    }
    match child.child.wait() {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => failures.push(format!("reap {:?}: {error}", child.plan.role)),
    }
    if failures.is_empty() { Ok(()) } else { Err(failures.join("; ")) }
}

fn status_to_exit(status: ExitStatus) -> ChildExit {
    if status.success() {
        ChildExit::Success
    } else if let Some(code) = status.code() {
        ChildExit::ExitCode(code)
    } else if let Some(signal) = status.signal() {
        ChildExit::Signal(signal)
    } else {
        ChildExit::ExitCode(-1)
    }
}

fn child_error(child: &Running, action: &str, error: std::io::Error) -> String {
    format!("{action} {:?} at {}: {error}", child.plan.role, child.plan.program.display())
}
