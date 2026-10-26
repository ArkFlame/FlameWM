#[rustfmt::skip]
#[path = "../session_app_compat.rs"]
mod session_app_compat;

use std::collections::BTreeMap;
use std::env;
use std::io::{ErrorKind, Read};
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
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
/// Upper bound on the supervisor's idle block. SIGCHLD normally wakes it; this bounds a missed
/// wakeup (signal-hook drops a wakeup byte if the pipe is full) to one cheap 1 s re-scan.
/// SIG_IGN note: signal-hook installs its own SIGCHLD handler, so children never inherit SIG_IGN
/// (exec resets caught signals to SIG_DFL); auto-reaping cannot hide exits from try_wait here.
const SUPERVISOR_SAFETY_TIMEOUT: Duration = Duration::from_secs(1);

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

    let executable =
        env::current_exe().map_err(|error| format!("locate session executable: {error}"))?;
    let bindir = executable
        .parent()
        .ok_or_else(|| "session executable has no parent directory".to_owned())?
        .to_path_buf();
    let home = env::var_os("HOME").map_or_else(|| PathBuf::from("/root"), PathBuf::from);
    let mut paths = default_paths_from_home(&bindir, home.clone());
    if let Some(config) = env::var_os("XDG_CONFIG_HOME") {
        paths.config_home = PathBuf::from(config);
    }
    let existing = env::vars().collect::<BTreeMap<_, _>>();
    let args = env::args().skip(1).collect::<Vec<_>>();
    let mut plan = build_launch_plan(&paths, &args, &existing);
    // Generate only FlameWM-owned XDG overlays; never mutate existing Brave
    // profiles, wallet contents, Plasma configuration, or desktop identities.
    if let Err(error) = session_app_compat::configure(&home, &mut plan.processes) {
        eprintln!("flamewm-session: KDE app compatibility degraded: {error}");
    }
    if plan.processes.len() != 3
        || plan.processes[0].role != SessionRole::WindowManager
        || plan.processes[1].role != SessionRole::Desktop
        || plan.processes[2].role != SessionRole::Shell
    {
        return Err(
            "session launch plan does not contain WM, Desktop, Shell in required order".to_owned(),
        );
    }
    // D-Bus/systemd user activation runs outside the process supervisor.
    // Without updating its environment, a portal or KWallet service activated
    // after login can inherit stale Plasma credentials, DISPLAY and desktop id.
    // This never starts an additional session bus or modifies saved wallets.
    sync_activation_environment(&plan.processes[0].environment);
    session_app_compat::finalize_pam_kwallet_login(&plan.processes[0].environment);
    restore_saved_display_order();
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
    ALLOCATOR_TUNING
        .into_iter()
        .filter(|(key, _)| !is_set(key))
        .collect()
}

/// A two-connector preference belongs to FlameWM, not Plasma/KScreen.
/// Restore only when BOTH connector names are currently active. A stale
/// hotplug preference does not cause a mode-set or block login.
fn restore_saved_display_order() {
    let Some((left, right)) = flamewm_session_core::display_order::load_order() else {
        return;
    };
    let Ok(snapshot) = Command::new("xrandr").arg("--query").output() else {
        return;
    };
    if !snapshot.status.success() {
        return;
    }
    let output = String::from_utf8_lossy(&snapshot.stdout);
    let connected = |connector: &str| {
        output.lines().any(|line| {
            line.strip_prefix(connector).is_some_and(|rest| {
                rest.starts_with(" connected ")
                    && rest
                        .split_whitespace()
                        .any(|token| token.contains('x') && token.contains('+'))
            })
        })
    };
    if !connected(&left) || !connected(&right) {
        return;
    }
    match Command::new("xrandr")
        .args(["--output", &right, "--right-of", &left])
        .status()
    {
        Ok(status) if status.success() => (),
        Ok(status) => eprintln!("flamewm-session: monitor order restore rejected: {status}"),
        Err(error) => eprintln!("flamewm-session: monitor order restore unavailable: {error}"),
    }
}

/// Publish the SAME session identity and display coordinates to services
/// activated later by Dolphin, Brave, xdg-desktop-portal or KWallet.
/// No credential access and no global user-config mutation.
fn activation_assignments(
    environment: &BTreeMap<String, String>,
    inherited: &BTreeMap<String, String>,
) -> Vec<String> {
    const KEYS: &[&str] = &[
        "DISPLAY",
        "XAUTHORITY",
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_DESKTOP",
        "XDG_SESSION_TYPE",
        "XDG_DATA_DIRS",
        "XDG_CONFIG_DIRS",
        "XDG_MENU_PREFIX",
        "QT_QPA_PLATFORMTHEME",
        "KDE_SESSION_VERSION",
    ];
    KEYS.iter()
        .filter_map(|key| {
            // Unlike other keys, a missing KDE version must CLEAR stale activation
            // state from a prior Plasma login, not resurrect an inherited value.
            if *key == "KDE_SESSION_VERSION" {
                return Some(format!(
                    "KDE_SESSION_VERSION={}",
                    environment.get(*key).map(String::as_str).unwrap_or("")
                ));
            }
            environment
                .get(*key)
                .or_else(|| inherited.get(*key))
                .filter(|value| !value.is_empty())
                .map(|value| format!("{key}={value}"))
        })
        .collect()
}

fn sync_activation_environment(environment: &BTreeMap<String, String>) {
    let mut command = Command::new("dbus-update-activation-environment");
    command.arg("--systemd").args(activation_assignments(
        environment,
        &env::vars().collect::<BTreeMap<_, _>>(),
    ));
    match command.status() {
        Ok(status) if status.success() => (),
        Ok(status) => {
            eprintln!("flamewm-session: D-Bus activation environment not synchronized: {status}")
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            eprintln!(
                "flamewm-session: dbus-update-activation-environment unavailable; portal session integration degraded"
            );
        }
        Err(error) => eprintln!("flamewm-session: activation environment: {error}"),
    }
}

fn spawn(plan: ProcessPlan) -> Result<Running, String> {
    let mut command = Command::new(&plan.program);
    command
        .args(&plan.arguments)
        .envs(&plan.environment)
        .env(
            "FLAMEWM_SESSION_SUPERVISOR_PID",
            std::process::id().to_string(),
        )
        .env_remove("FLAMEWM_SESSION_LOGOUT_OWNER");
    if plan.role == SessionRole::WindowManager {
        command.env("FLAMEWM_SESSION_LOGOUT_OWNER", "1");
    }
    for (key, value) in allocator_tuning_defaults(|key| {
        env::var_os(key).is_some() || plan.environment.contains_key(key)
    }) {
        command.env(key, value);
    }
    let child = command.spawn().map_err(|error| {
        format!(
            "spawn {:?} at {}: {error}",
            plan.role,
            plan.program.display()
        )
    })?;
    Ok(Running {
        plan,
        child,
        supervisor: ProcessSupervisorPolicy::default(),
    })
}

/// Read end of the self-pipe written by the signal handlers for SIGCHLD and stop signals.
static WAKEUP: OnceLock<UnixStream> = OnceLock::new();

fn register_wakeups() -> Result<(), String> {
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
fn wait_for_wakeup(timeout: Option<Duration>) -> Result<(), String> {
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

fn check_stop(stop_requested: &AtomicBool) -> Result<(), String> {
    if stop_requested.load(Ordering::Relaxed) {
        Err("session shutdown signal received".to_owned())
    } else {
        Ok(())
    }
}

fn check_parent(parent: rustix::process::Pid, stop_requested: &AtomicBool) -> Result<(), String> {
    check_stop(stop_requested)?;
    if getppid() != Some(parent) {
        Err("session parent exited or is unavailable; stopping session children".to_owned())
    } else {
        Ok(())
    }
}

fn startup_grace(
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

fn wait_ready(
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
                        last_error = "session control returned a non-bootstrap response".to_owned()
                    }
                    Err(error) => last_error = format!("session control bootstrap: {error}"),
                }
                if Instant::now() >= deadline {
                    break;
                }
            }
            Ok(false) => last_error = "EWMH supporting-WM check is not valid".to_owned(),
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

fn control_ready(timeout: Duration) -> Result<bool, String> {
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

fn ewmh_ready() -> Result<bool, String> {
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

fn supporting_wm_check_is_valid(root_support: Option<u32>, self_support: Option<u32>) -> bool {
    matches!(root_support, Some(support) if support != 0 && self_support == Some(support))
}

fn clean_wm_exit(index: usize, status: ExitStatus) -> bool {
    index == 0 && status.success()
}

#[cfg(test)]
mod tests {
    use super::{
        ProcessPlan, Running, WAKEUP, activation_assignments, allocator_tuning_defaults,
        clean_wm_exit, scan_exits, stop_all, supporting_wm_check_is_valid,
    };
    use flamewm_session_core::{ProcessSupervisorPolicy, SessionRole, SupervisorPolicy};
    use std::collections::BTreeMap;
    use std::os::unix::net::UnixStream;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command, ExitStatus};

    #[test]
    fn successful_wm_exit_stops_session_without_restarting_it() {
        assert!(clean_wm_exit(0, ExitStatus::from_raw(0)));
        assert!(!clean_wm_exit(0, ExitStatus::from_raw(1 << 8)));
        assert!(!clean_wm_exit(1, ExitStatus::from_raw(0)));
    }

    #[test]
    fn logout_cleanup_reaps_only_managed_children_without_restart() {
        let (reader, _writer) = UnixStream::pair().unwrap();
        let _ = WAKEUP.set(reader);
        let make_plan = |role| ProcessPlan {
            role,
            program: if role == SessionRole::WindowManager {
                "/bin/true"
            } else {
                "/bin/sleep"
            }
            .into(),
            arguments: if role == SessionRole::WindowManager {
                Vec::new()
            } else {
                vec!["30".into()]
            },
            environment: BTreeMap::new(),
            required: true,
        };
        let wm_plan = make_plan(SessionRole::WindowManager);
        let mut wm_child = Command::new(&wm_plan.program).spawn().unwrap();
        let wm_pid = wm_child.id();
        let wm_status = wm_child.wait().unwrap();
        assert!(clean_wm_exit(0, wm_status));
        let wm = Running {
            plan: wm_plan,
            child: wm_child,
            supervisor: ProcessSupervisorPolicy::new(SupervisorPolicy {
                graceful_stop_ms: 5,
                ..SupervisorPolicy::default()
            }),
        };
        let desktop_plan = make_plan(SessionRole::Desktop);
        let desktop_child = Command::new(&desktop_plan.program)
            .args(&desktop_plan.arguments)
            .spawn()
            .unwrap();
        let desktop_pid = desktop_child.id();
        let desktop = Running {
            plan: desktop_plan,
            child: desktop_child,
            supervisor: ProcessSupervisorPolicy::new(SupervisorPolicy {
                graceful_stop_ms: 5,
                ..SupervisorPolicy::default()
            }),
        };
        let mut children = vec![wm, desktop];
        let managed_pids = [wm_pid, desktop_pid];
        let mut unrelated = Command::new("/bin/sleep").arg("30").spawn().unwrap();

        let cleanup = stop_all(&mut children);
        let unrelated_status = unrelated.try_wait().unwrap();
        let _ = unrelated.kill();
        let _ = unrelated.wait();

        assert!(cleanup.is_ok(), "managed child cleanup failed: {cleanup:?}");
        assert!(children.is_empty(), "supervisor retained managed children");
        assert!(
            managed_pids
                .iter()
                .all(|pid| !std::path::Path::new(&format!("/proc/{pid}")).exists()),
            "managed child remained alive"
        );
        assert!(
            unrelated_status.is_none(),
            "cleanup affected an unrelated process"
        );
    }

    #[test]
    fn activation_clears_stale_kde_backend_without_exporting_pam_credentials() {
        let live = BTreeMap::from([
            ("DISPLAY".to_owned(), ":0".to_owned()),
            ("XDG_CURRENT_DESKTOP".to_owned(), "FlameWM".to_owned()),
        ]);
        let inherited = BTreeMap::from([
            ("XDG_CURRENT_DESKTOP".to_owned(), "KDE".to_owned()),
            ("KDE_SESSION_VERSION".to_owned(), "5".to_owned()),
            (
                "PAM_KWALLET5_LOGIN".to_owned(),
                "/private/socket".to_owned(),
            ),
        ]);
        let args = activation_assignments(&live, &inherited);
        assert!(args.contains(&"XDG_CURRENT_DESKTOP=FlameWM".to_owned()));
        assert!(args.contains(&"KDE_SESSION_VERSION=".to_owned()));
        assert!(!args.iter().any(|arg| arg.contains("PAM_KWALLET5_LOGIN")));
        let kde6 = BTreeMap::from([("KDE_SESSION_VERSION".to_owned(), "6".to_owned())]);
        let selected = activation_assignments(&kde6, &inherited);
        assert!(selected.contains(&"KDE_SESSION_VERSION=6".to_owned()));
    }

    #[test]
    fn scan_exits_reports_every_exited_child_before_blocking() {
        let mixed = scan_exits(3, |index| {
            Ok(if index == 1 { None } else { Some(index as i32) })
        })
        .unwrap();
        assert_eq!(mixed, vec![(2, 2), (0, 0)]);
        let both = scan_exits(2, |index| Ok(Some(index as i32))).unwrap();
        assert_eq!(both, vec![(1, 1), (0, 0)]);
        assert!(
            scan_exits(2, |_| Ok::<Option<i32>, String>(None))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            scan_exits(2, |_| Err::<Option<i32>, _>("poll".to_owned())),
            Err("poll".to_owned())
        );
    }

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

/// Polls every child without early exit and returns exits in descending index order, so a
/// single pass sees all simultaneous exits before the supervisor decides to block.
fn scan_exits<T>(
    count: usize,
    mut poll: impl FnMut(usize) -> Result<Option<T>, String>,
) -> Result<Vec<(usize, T)>, String> {
    let mut exits = Vec::new();
    for index in (0..count).rev() {
        if let Some(status) = poll(index)? {
            exits.push((index, status));
        }
    }
    Ok(exits)
}

fn supervise(
    children: &mut Vec<Running>,
    parent: rustix::process::Pid,
    role_plans: &[ProcessPlan],
    stop_requested: &AtomicBool,
) -> Result<(), String> {
    loop {
        check_parent(parent, stop_requested)?;
        let exits = scan_exits(children.len(), |index| {
            children[index]
                .child
                .try_wait()
                .map_err(|error| child_error(&children[index], "supervise", error))
        })?;
        if let Some(&(index, status)) = exits.first() {
            if clean_wm_exit(index, status) {
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
                SupervisorDecision::GiveUp => {
                    return Err(format!(
                        "{:?} at {} exhausted its restart budget",
                        children[index].plan.role,
                        children[index].plan.program.display()
                    ));
                }
                SupervisorDecision::StopCleanly => return Ok(()),
                SupervisorDecision::SpawnNow => {
                    return Err(format!(
                        "supervisor invariant violated: {:?} at {} requested immediate spawn after exit",
                        children[index].plan.role,
                        children[index].plan.program.display()
                    ));
                }
            }
        } else {
            // No exit pending: block until SIGCHLD/stop or the safety timeout. A handled exit
            // skips this block and re-scans, so no second simultaneous exit is left as a zombie.
            wait_for_wakeup(Some(SUPERVISOR_SAFETY_TIMEOUT))?;
        }
    }
}

fn wait_backoff(
    delay_ms: u64,
    parent: rustix::process::Pid,
    stop_requested: &AtomicBool,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_millis(delay_ms);
    while Instant::now() < deadline {
        check_parent(parent, stop_requested)?;
        wait_for_wakeup(Some(deadline.saturating_duration_since(Instant::now())))?;
    }
    check_parent(parent, stop_requested)
}

fn stop_dependents(children: &mut Vec<Running>) -> Result<(), String> {
    let mut failures = Vec::new();
    while children.len() > 1 {
        if let Some(mut child) = children.pop() {
            if let Err(error) = stop_role(&mut child) {
                failures.push(error);
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn stop_all(children: &mut Vec<Running>) -> Result<(), String> {
    let mut failures = Vec::new();
    while let Some(mut child) = children.pop() {
        child.supervisor.request_stop();
        if let Err(error) = stop_role(&mut child) {
            failures.push(error);
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn stop_role(child: &mut Running) -> Result<(), String> {
    child.supervisor.request_stop();
    let mut failures = Vec::new();
    let mut exited = match child.child.try_wait() {
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(error) => {
            failures.push(child_error(child, "check before graceful stop", error));
            true
        }
    };
    if !exited {
        if let Some(pid) = Pid::from_raw(child.child.id() as i32) {
            if let Err(error) = kill_process(pid, Signal::TERM) {
                if error != rustix::io::Errno::SRCH {
                    failures.push(format!("terminate {:?}: {error}", child.plan.role));
                }
            }
        }
    }
    let deadline = Instant::now() + Duration::from_millis(child.supervisor.graceful_stop_ms());
    loop {
        if exited {
            break;
        }
        match child.child.try_wait() {
            Ok(Some(_)) => {
                exited = true;
                break;
            }
            Ok(None) if Instant::now() < deadline => {
                if let Err(error) =
                    wait_for_wakeup(Some(deadline.saturating_duration_since(Instant::now())))
                {
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
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
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
    format!(
        "{action} {:?} at {}: {error}",
        child.plan.role,
        child.plan.program.display()
    )
}
