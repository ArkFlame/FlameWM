#[rustfmt::skip]
#[path = "../session_app_compat.rs"]
mod session_app_compat;
#[path = "../session_diagnostics.rs"]
mod session_diagnostics;

use std::collections::BTreeMap;
use std::env;
use std::io::{ErrorKind, Read};
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use flamewm_control_core::{ControlRequest, ControlResponse};
use flamewm_control_dbus::ControlClient;
use flamewm_dbus_reactor::BusKind;
use flamewm_session_core::diagnostics::{
    CHECK_EXIT_FATAL, Severity, check_exit_code, describe_exit, fallback_plan, format_report,
};
use flamewm_session_core::{
    ChildExit, ProcessPlan, ProcessSupervisorPolicy, SessionRole, SupervisorDecision,
    build_launch_plan, default_paths_from_home, installed_kwallet_generations,
    resolve_kwallet_version,
};
use rustix::process::{Pid, Signal, getppid, kill_process, set_parent_process_death_signal};
use signal_hook::consts::signal::{SIGCHLD, SIGHUP, SIGINT, SIGTERM};
use signal_hook::flag;
use signal_hook::low_level::pipe;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};

const SESSION_LOG_PREFIX: &str = "flamewm-session";
const SHUTDOWN_SIGNAL: &str = "session shutdown signal received";
const READY_TIMEOUT: Duration = Duration::from_secs(30);
const TICK: Duration = Duration::from_millis(100);
const STARTUP_GRACE: Duration = Duration::from_millis(500);
const KDE_SESSION_IDENTITY: [&str; 3] =
    ["KDE_FULL_SESSION", "KDE_SESSION_VERSION", "KDE_SESSION_UID"];
/// Upper bound on the supervisor's idle block. SIGCHLD normally wakes it; this bounds a missed
/// wakeup (signal-hook drops a wakeup byte if the pipe is full) to one cheap 1 s re-scan.
/// `SIG_IGN` note: signal-hook installs its own SIGCHLD handler, so children never inherit `SIG_IGN`
/// (exec resets caught signals to `SIG_DFL`); auto-reaping cannot hide exits from `try_wait` here.
const SUPERVISOR_SAFETY_TIMEOUT: Duration = Duration::from_secs(1);

struct Running {
    plan: ProcessPlan,
    child: Child,
    supervisor: ProcessSupervisorPolicy,
}

fn main() {
    if env::args().nth(1).as_deref() == Some("--check") {
        std::process::exit(run_check());
    }
    if let Err(error) = execute() {
        session_diagnostics::log(SESSION_LOG_PREFIX, &format!("session failed: {error}"));
        session_diagnostics::show_failure_dialog(&format!(
            "FlameWM could not start: {error}\n\nDetails: {}\nRun flamewm-session --check for a report.",
            log_path_text()
        ));
        std::process::exit(1);
    }
}

/// `flamewm-session --check`: prints the preflight report. Exit status: 0 OK, 1 cannot start,
/// 2 starts with fallbacks.
fn run_check() -> i32 {
    match session_paths() {
        Ok((paths, home)) => {
            session_diagnostics::init_session_log(Some(&home));
            let findings = session_diagnostics::preflight(&paths, &home);
            print!("{}", format_report(&findings));
            println!("session log: {}", log_path_text());
            check_exit_code(&findings)
        }
        Err(error) => {
            eprintln!("{SESSION_LOG_PREFIX}: {error}");
            CHECK_EXIT_FATAL
        }
    }
}

fn log_path_text() -> String {
    session_diagnostics::log_location().map_or_else(
        || "(no writable session log)".to_owned(),
        |(path, _)| path.display().to_string(),
    )
}

/// Install directory layout and the user's home, shared by the launch and `--check` paths.
fn session_paths() -> Result<(flamewm_session_core::SessionPaths, PathBuf), String> {
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
    Ok((paths, home))
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

    let (paths, home) = session_paths()?;
    session_diagnostics::init_session_log(Some(&home));
    let findings = session_diagnostics::preflight(&paths, &home);
    for finding in &findings {
        session_diagnostics::log(
            "preflight",
            &format!(
                "{} {}: {}",
                finding.severity.label(),
                finding.check,
                finding.message
            ),
        );
    }
    let fatal = findings
        .iter()
        .filter(|finding| finding.severity == Severity::Fatal)
        .map(|finding| format!("{}: {}", finding.check, finding.message))
        .collect::<Vec<_>>();
    if !fatal.is_empty() {
        return Err(format!("preflight failed: {}", fatal.join("; ")));
    }
    let existing = env::vars().collect::<BTreeMap<_, _>>();
    let args = env::args().skip(1).collect::<Vec<_>>();
    let mut plan = build_launch_plan(&paths, &args, &existing);
    add_app_compatibility_environment(&mut plan.processes, &existing);
    // Generate only FlameWM-owned XDG overlays; never mutate existing Brave
    // profiles, wallet contents, Plasma configuration, or desktop identities.
    if let Err(error) = session_app_compat::configure(&home, &mut plan.processes) {
        eprintln!("{SESSION_LOG_PREFIX}: KDE app compatibility degraded: {error}");
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
    // Without updating it, a portal or KWallet service activated after login
    // can inherit stale KDE identity, DISPLAY and desktop id.
    // This never starts an additional session bus or modifies saved wallets.
    let activation_environment = plan.processes[0].environment.clone();
    let activation_restore = sync_activation_environment(&activation_environment);
    session_app_compat::finalize_pam_kwallet_login(&activation_environment);
    for process in &mut plan.processes {
        clear_kde_session_identity(&mut process.environment);
    }
    restore_saved_display_order();
    let mut role_plans = plan.processes;
    let mut running: Vec<Running> = Vec::new();

    let result = (|| {
        check_stop(&stop_requested)?;
        running.push(spawn(role_plans[0].clone())?);
        wait_ready(&mut running[0], parent, &stop_requested)?;
        for index in [1, 2] {
            check_stop(&stop_requested)?;
            let started = start_optional(&mut role_plans[index], parent, &stop_requested)?;
            running.push(started);
        }
        supervise(&mut running, parent, &role_plans, &stop_requested)
    })();
    let cleanup = stop_all(&mut running);
    let activation_restore_result =
        activation_restore.map_or(Ok(()), |mut snapshot| snapshot.restore());
    let session_result = match (result, cleanup) {
        (Err(error), Ok(())) if error == SHUTDOWN_SIGNAL => Ok(()),
        (Err(error), Err(cleanup)) => Err(format!("{error}; cleanup failed: {cleanup}")),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(cleanup)) => Err(format!("cleanup failed: {cleanup}")),
        (Ok(()), Ok(())) => Ok(()),
    };
    combine_activation_restore_result(session_result, activation_restore_result)
}

fn add_app_compatibility_environment(
    processes: &mut [ProcessPlan],
    existing_env: &BTreeMap<String, String>,
) {
    let (installed5, installed6) = installed_kwallet_generations();
    if let Some(version) = resolve_kwallet_version(existing_env, installed5, installed6) {
        // The explicit Brave compatibility overlay and PAM handoff need the
        // selected generation; generic session children do not impersonate KDE.
        for process in processes {
            process
                .environment
                .insert("KDE_SESSION_VERSION".to_owned(), version.to_owned());
        }
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

/// A two-connector preference belongs to `FlameWM`, not Plasma/KScreen.
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
        Ok(status) => eprintln!("{SESSION_LOG_PREFIX}: monitor order restore rejected: {status}"),
        Err(error) => eprintln!("{SESSION_LOG_PREFIX}: monitor order restore unavailable: {error}"),
    }
}

/// Publish the SAME session identity and display coordinates to services
/// activated later by Dolphin, Brave, xdg-desktop-portal or `KWallet`.
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
        "KDE_FULL_SESSION",
        "KDE_SESSION_VERSION",
        "KDE_SESSION_UID",
    ];
    KEYS.iter()
        .filter_map(|key| {
            // The selected wallet generation is for explicit Brave/PAM compatibility only.
            if matches!(
                *key,
                "KDE_FULL_SESSION" | "KDE_SESSION_VERSION" | "KDE_SESSION_UID"
            ) {
                return Some(format!("{key}="));
            }
            environment
                .get(*key)
                .or_else(|| inherited.get(*key))
                .filter(|value| !value.is_empty())
                .map(|value| format!("{key}={value}"))
        })
        .collect()
}

fn clear_kde_session_identity(environment: &mut BTreeMap<String, String>) {
    for key in KDE_SESSION_IDENTITY {
        environment.remove(key);
    }
}

fn apply_session_environment(command: &mut Command, environment: &BTreeMap<String, String>) {
    command.envs(environment);
    for key in KDE_SESSION_IDENTITY {
        command.env_remove(key);
    }
}

#[derive(Debug)]
struct ActivationEnvironmentSnapshot {
    keys: Vec<String>,
    previous: Vec<String>,
    restored: bool,
}

impl ActivationEnvironmentSnapshot {
    fn from_show_environment(keys: Vec<String>, output: &str) -> Self {
        let mut values = BTreeMap::new();
        for line in output.lines() {
            if let Some((key, value)) = line.split_once('=') {
                if keys.iter().any(|requested| requested.as_str() == key) {
                    values.insert(key.to_owned(), value.to_owned());
                }
            }
        }
        let previous = keys
            .iter()
            .filter_map(|key| values.get(key).map(|value| format!("{key}={value}")))
            .collect();
        Self {
            keys,
            previous,
            restored: false,
        }
    }

    fn capture(keys: Vec<String>) -> Result<Self, String> {
        if !Command::new("busctl")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
        {
            return Err("busctl unavailable".to_owned());
        }
        let output = Command::new("systemctl")
            .args(["--user", "show-environment"])
            .output()
            .map_err(|_| "systemctl user environment unavailable".to_owned())?;
        if !output.status.success() {
            return Err(format!(
                "systemctl show-environment exited with {}",
                output.status
            ));
        }
        Ok(Self::from_show_environment(
            keys,
            &String::from_utf8_lossy(&output.stdout),
        ))
    }

    fn busctl_arguments(&self) -> Vec<String> {
        let mut arguments = vec![
            "--user".to_owned(),
            "call".to_owned(),
            "org.freedesktop.systemd1".to_owned(),
            "/org/freedesktop/systemd1".to_owned(),
            "org.freedesktop.systemd1.Manager".to_owned(),
            "UnsetAndSetEnvironment".to_owned(),
            "as".to_owned(),
            self.keys.len().to_string(),
        ];
        arguments.extend(self.keys.iter().cloned());
        arguments.push("as".to_owned());
        arguments.push(self.previous.len().to_string());
        arguments.extend(self.previous.iter().cloned());
        arguments
    }

    fn dbus_restore_assignments(&self) -> Vec<String> {
        self.keys
            .iter()
            .map(|key| {
                let value = self
                    .previous
                    .iter()
                    .find_map(|assignment| {
                        let (name, value) = assignment.split_once('=')?;
                        (name == key.as_str()).then_some(value)
                    })
                    .unwrap_or("");
                format!("{key}={value}")
            })
            .collect()
    }

    fn restore(&mut self) -> Result<(), String> {
        self.restore_with(run_bounded_activation_command)
    }

    fn restore_with(
        &mut self,
        mut run: impl FnMut(&str, &[String]) -> Result<(), String>,
    ) -> Result<(), String> {
        if self.restored {
            return Ok(());
        }
        self.restored = true;
        // dbus-daemon has no unset API, so restore known values and clear
        // previously absent keys with empty values before atomically restoring
        // systemd's exact set/unset state.
        let dbus_assignments = self.dbus_restore_assignments();
        let dbus_result = run("dbus-update-activation-environment", &dbus_assignments);
        let systemd_arguments = self.busctl_arguments();
        let systemd_result = run("busctl", &systemd_arguments);
        match (dbus_result, systemd_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(dbus), Ok(())) => Err(format!("D-Bus restore failed: {dbus}")),
            (Ok(()), Err(systemd)) => Err(format!("systemd restore failed: {systemd}")),
            (Err(dbus), Err(systemd)) => Err(format!(
                "D-Bus restore failed: {dbus}; systemd restore failed: {systemd}"
            )),
        }
    }
}

fn run_bounded_activation_command(program: &str, arguments: &[String]) -> Result<(), String> {
    let mut child = Command::new(program)
        .args(arguments)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| format!("{program} could not start"))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("exited with {status}")),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("child status unavailable".to_owned());
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("timed out after 5s".to_owned());
            }
        }
    }
}

impl Drop for ActivationEnvironmentSnapshot {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            eprintln!("{SESSION_LOG_PREFIX}: activation environment restore failed: {error}");
        }
    }
}

fn combine_activation_restore_result(
    session_result: Result<(), String>,
    restore_result: Result<(), String>,
) -> Result<(), String> {
    match (session_result, restore_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(session), Ok(())) => Err(session),
        (Ok(()), Err(restore)) => Err(format!("activation environment restore failed: {restore}")),
        (Err(session), Err(restore)) => Err(format!(
            "{session}; activation environment restore failed: {restore}"
        )),
    }
}

fn activation_assignment_keys(assignments: &[String]) -> Vec<String> {
    let mut keys = Vec::new();
    for assignment in assignments {
        if let Some((key, _)) = assignment.split_once('=') {
            if !keys
                .iter()
                .any(|existing: &String| existing.as_str() == key)
            {
                keys.push(key.to_owned());
            }
        }
    }
    keys
}

fn sync_activation_environment(
    environment: &BTreeMap<String, String>,
) -> Option<ActivationEnvironmentSnapshot> {
    let inherited = env::vars().collect::<BTreeMap<_, _>>();
    let assignments = activation_assignments(environment, &inherited);
    let keys = activation_assignment_keys(&assignments);
    let snapshot = match ActivationEnvironmentSnapshot::capture(keys) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            eprintln!(
                "{SESSION_LOG_PREFIX}: activation environment not synchronized: unable to snapshot user manager ({error})"
            );
            return None;
        }
    };
    let mut command = Command::new("dbus-update-activation-environment");
    command.arg("--systemd").args(assignments);
    match command.status() {
        Ok(status) if status.success() => (),
        Ok(status) => {
            eprintln!(
                "{SESSION_LOG_PREFIX}: D-Bus activation environment not synchronized: {status}"
            );
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            eprintln!(
                "{SESSION_LOG_PREFIX}: dbus-update-activation-environment unavailable; portal session integration degraded"
            );
        }
        Err(error) => eprintln!("{SESSION_LOG_PREFIX}: activation environment: {error}"),
    }
    Some(snapshot)
}

/// Starts a non-critical component (desktop or shell). When its binary is missing, cannot be
/// spawned, or exits during startup, the plan is replaced by the fallback process so the WM and
/// the other components keep running. Supervision errors and parent death still abort.
fn start_optional(
    plan: &mut ProcessPlan,
    parent: Pid,
    stop_requested: &AtomicBool,
) -> Result<Running, String> {
    let label = role_label(plan.role);
    if session_diagnostics::is_runnable(&plan.program) {
        match spawn(plan.clone()) {
            Ok(mut running) => match startup_grace(&mut running, parent, stop_requested) {
                Ok(()) => return Ok(running),
                Err(error) if running.child.try_wait().ok().flatten().is_none() => {
                    let _ = running.child.kill();
                    let _ = running.child.wait();
                    return Err(error);
                }
                Err(error) => session_diagnostics::log(label, &error),
            },
            Err(error) => session_diagnostics::log(label, &error),
        }
    } else {
        session_diagnostics::log(
            label,
            &format!("{} is missing or not executable", plan.program.display()),
        );
    }
    session_diagnostics::log(
        label,
        "starting the fallback instead; the session continues",
    );
    *plan = fallback_plan(plan, &log_path_text());
    spawn(plan.clone())
}

fn role_label(role: SessionRole) -> &'static str {
    match role {
        SessionRole::WindowManager => "window manager",
        SessionRole::Desktop => "desktop",
        SessionRole::Shell => "shell",
    }
}

fn spawn(plan: ProcessPlan) -> Result<Running, String> {
    let mut command = Command::new(&plan.program);
    let (stdout, stderr) = session_diagnostics::child_stdio();
    command
        .args(&plan.arguments)
        .env(
            "FLAMEWM_SESSION_SUPERVISOR_PID",
            std::process::id().to_string(),
        )
        .env_remove("FLAMEWM_SESSION_LOGOUT_OWNER")
        .stdout(stdout)
        .stderr(stderr);
    apply_session_environment(&mut command, &plan.environment);
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
        Err(SHUTDOWN_SIGNAL.to_owned())
    } else {
        Ok(())
    }
}

fn check_parent(parent: rustix::process::Pid, stop_requested: &AtomicBool) -> Result<(), String> {
    check_stop(stop_requested)?;
    if getppid() == Some(parent) {
        Ok(())
    } else {
        Err("session parent exited or is unavailable; stopping session children".to_owned())
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
            let exit = status_to_exit(status);
            session_diagnostics::log(
                role_label(children[index].plan.role),
                &format!(
                    "{} {}",
                    children[index].plan.program.display(),
                    describe_exit(exit)
                ),
            );
            if clean_wm_exit(index, status) {
                children[0].supervisor.request_stop();
                return Ok(());
            }
            match children[index].supervisor.child_exited(exit) {
                SupervisorDecision::GiveUp if index != 0 => {
                    session_diagnostics::log(
                        role_label(children[index].plan.role),
                        "restart budget exhausted; starting the fallback instead",
                    );
                    children[index] = spawn(fallback_plan(&role_plans[index], &log_path_text()))?;
                }
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

#[cfg(test)]
mod tests {
    use super::{
        ActivationEnvironmentSnapshot, ProcessPlan, Running, WAKEUP, activation_assignment_keys,
        activation_assignments, add_app_compatibility_environment, allocator_tuning_defaults,
        apply_session_environment, clean_wm_exit, clear_kde_session_identity,
        combine_activation_restore_result, scan_exits, stop_all, supporting_wm_check_is_valid,
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
    fn activation_clears_kde_identity_and_keeps_only_the_selected_wallet_compatibility_version() {
        let live = BTreeMap::from([
            ("DISPLAY".to_owned(), ":0".to_owned()),
            ("XDG_CURRENT_DESKTOP".to_owned(), "FlameWM".to_owned()),
            ("KDE_SESSION_VERSION".to_owned(), "6".to_owned()),
        ]);
        let inherited = BTreeMap::from([
            ("XDG_CURRENT_DESKTOP".to_owned(), "KDE".to_owned()),
            ("KDE_FULL_SESSION".to_owned(), "true".to_owned()),
            ("KDE_SESSION_VERSION".to_owned(), "5".to_owned()),
            ("KDE_SESSION_UID".to_owned(), "1000".to_owned()),
            (
                "PAM_KWALLET5_LOGIN".to_owned(),
                "/private/socket".to_owned(),
            ),
        ]);
        let args = activation_assignments(&live, &inherited);
        assert!(args.contains(&"XDG_CURRENT_DESKTOP=FlameWM".to_owned()));
        assert!(args.contains(&"KDE_FULL_SESSION=".to_owned()));
        assert!(args.contains(&"KDE_SESSION_VERSION=".to_owned()));
        assert!(!args.contains(&"KDE_SESSION_VERSION=6".to_owned()));
        assert!(args.contains(&"KDE_SESSION_UID=".to_owned()));
        assert!(!args.iter().any(|arg| arg.contains("PAM_KWALLET5_LOGIN")));
        let generic = BTreeMap::from([("XDG_CURRENT_DESKTOP".to_owned(), "FlameWM".to_owned())]);
        let selected = activation_assignments(&generic, &inherited);
        assert!(selected.contains(&"KDE_SESSION_VERSION=".to_owned()));
    }

    #[test]
    fn session_child_environment_removes_inherited_kde_markers_but_keeps_explicit_app_env() {
        let inherited = BTreeMap::from([
            ("KDE_FULL_SESSION".to_owned(), "true".to_owned()),
            ("KDE_SESSION_VERSION".to_owned(), "5".to_owned()),
            ("KDE_SESSION_UID".to_owned(), "1000".to_owned()),
        ]);
        let planned = BTreeMap::from([
            ("KDE_SESSION_VERSION".to_owned(), "6".to_owned()),
            ("FLAMEWM_TEST_ENV".to_owned(), "session".to_owned()),
        ]);
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "printf '%s|%s|%s|%s' \"${KDE_FULL_SESSION:-<unset>}\" \"${KDE_SESSION_VERSION:-<unset>}\" \"${KDE_SESSION_UID:-<unset>}\" \"$FLAMEWM_TEST_ENV\"",
        ]);
        command.envs(&inherited);
        apply_session_environment(&mut command, &planned);
        let output = command.output().unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "<unset>|<unset>|<unset>|session"
        );

        let mut compatibility = planned;
        clear_kde_session_identity(&mut compatibility);
        assert!(!compatibility.contains_key("KDE_FULL_SESSION"));
        assert!(!compatibility.contains_key("KDE_SESSION_VERSION"));
        assert!(!compatibility.contains_key("KDE_SESSION_UID"));
        assert_eq!(
            compatibility.get("FLAMEWM_TEST_ENV").map(String::as_str),
            Some("session")
        );
    }

    #[test]
    fn explicit_brave_wallet_selection_survives_until_compatibility_setup() {
        let existing = BTreeMap::from([(
            "FLAMEWM_BRAVE_PASSWORD_STORE".to_owned(),
            "kwallet5".to_owned(),
        )]);
        let mut processes = [ProcessPlan {
            role: SessionRole::WindowManager,
            program: "/bin/true".into(),
            arguments: Vec::new(),
            environment: BTreeMap::new(),
            required: true,
        }];

        add_app_compatibility_environment(&mut processes, &existing);
        assert_eq!(
            processes[0]
                .environment
                .get("KDE_SESSION_VERSION")
                .map(String::as_str),
            Some("5")
        );
        clear_kde_session_identity(&mut processes[0].environment);
        assert!(!processes[0].environment.contains_key("KDE_SESSION_VERSION"));
    }

    #[test]
    fn activation_snapshot_restores_only_touched_systemd_keys() {
        let keys = vec![
            "DISPLAY".to_owned(),
            "KDE_FULL_SESSION".to_owned(),
            "KDE_SESSION_VERSION".to_owned(),
            "KDE_SESSION_UID".to_owned(),
        ];
        let mut snapshot = ActivationEnvironmentSnapshot::from_show_environment(
            keys.clone(),
            "DISPLAY=:8\nKDE_FULL_SESSION=true\nKDE_SESSION_VERSION=5\nUNRELATED=value\n",
        );
        snapshot.restored = true;
        assert_eq!(snapshot.keys, keys);
        assert_eq!(
            snapshot.previous,
            vec![
                "DISPLAY=:8".to_owned(),
                "KDE_FULL_SESSION=true".to_owned(),
                "KDE_SESSION_VERSION=5".to_owned()
            ]
        );
        assert_eq!(
            snapshot.dbus_restore_assignments(),
            vec![
                "DISPLAY=:8".to_owned(),
                "KDE_FULL_SESSION=true".to_owned(),
                "KDE_SESSION_VERSION=5".to_owned(),
                "KDE_SESSION_UID=".to_owned()
            ]
        );
        let args = snapshot.busctl_arguments();
        assert_eq!(args[5], "UnsetAndSetEnvironment");
        assert_eq!(args[6], "as");
        assert_eq!(args[7], "4");
        assert_eq!(args[12], "as");
        assert_eq!(args[13], "3");
        assert!(args.contains(&"KDE_SESSION_VERSION=5".to_owned()));
        assert!(!args.contains(&"UNRELATED=value".to_owned()));
    }

    #[test]
    fn activation_snapshot_returns_mocked_restore_failure_and_attempts_both_backends() {
        let mut snapshot = ActivationEnvironmentSnapshot::from_show_environment(
            vec!["DISPLAY".to_owned()],
            "DISPLAY=:8\n",
        );
        let mut restored_programs = Vec::new();

        let result = snapshot.restore_with(|program, _arguments| {
            restored_programs.push(program.to_owned());
            if program == "dbus-update-activation-environment" {
                Err("fixture failure".to_owned())
            } else {
                Ok(())
            }
        });

        assert_eq!(
            result,
            Err("D-Bus restore failed: fixture failure".to_owned())
        );
        assert_eq!(
            restored_programs,
            vec![
                "dbus-update-activation-environment".to_owned(),
                "busctl".to_owned()
            ]
        );
    }

    #[test]
    fn activation_restore_failure_fails_success_and_preserves_session_failure() {
        let restore_failure = Err("D-Bus restore failed".to_owned());
        assert_eq!(
            combine_activation_restore_result(Ok(()), restore_failure.clone()),
            Err("activation environment restore failed: D-Bus restore failed".to_owned())
        );
        assert_eq!(
            combine_activation_restore_result(Err("session failed".to_owned()), restore_failure),
            Err(
                "session failed; activation environment restore failed: D-Bus restore failed"
                    .to_owned()
            )
        );
    }

    #[test]
    fn activation_snapshot_keys_come_only_from_published_assignments() {
        let assignments = vec![
            "DISPLAY=:0".to_owned(),
            "KDE_SESSION_VERSION=".to_owned(),
            "DISPLAY=:1".to_owned(),
        ];
        assert_eq!(
            activation_assignment_keys(&assignments),
            vec!["DISPLAY".to_owned(), "KDE_SESSION_VERSION".to_owned()]
        );
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
