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
use std::process::{Child, Command, ExitStatus, Output, Stdio};
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
    ChildExit, Keyring, ProcessPlan, ProcessSupervisorPolicy, SessionRole, SupervisorDecision,
    build_launch_plan, default_paths_from_home, gnome_keyring_installed,
    installed_kwallet_generations, parse_gnome_keyring_environment, resolve_kwallet_version,
    select_keyring,
};
use rustix::process::{Pid, Signal, getppid, kill_process, set_parent_process_death_signal};
use signal_hook::consts::signal::{SIGCHLD, SIGHUP, SIGINT, SIGTERM};
use signal_hook::flag;
use signal_hook::low_level::pipe;

#[path = "flamewm-session/readiness.rs"]
mod readiness;
#[path = "flamewm-session/session_env.rs"]
mod session_env;
#[path = "flamewm-session/supervision.rs"]
mod supervision;
#[cfg(test)]
#[path = "flamewm-session/tests.rs"]
mod tests;

use readiness::*;
use session_env::*;
use supervision::*;

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
    let (installed5, installed6) = installed_kwallet_generations();
    let keyring_environment = start_session_keyring(select_keyring(
        &existing,
        installed5,
        installed6,
        gnome_keyring_installed(),
    ));
    add_keyring_environment(&mut plan.processes, &keyring_environment);
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

/// The saved left-to-right order belongs to `FlameWM`, not Plasma/KScreen.
/// Restore only when EVERY connector in it is currently active. A stale
/// hotplug preference does not cause a mode-set or block login. One xrandr
/// call places each output right of its predecessor.
fn restore_saved_display_order() {
    let Some(order) = flamewm_session_core::display_order::load_order() else {
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
    if !order.iter().all(|connector| connected(connector)) {
        return;
    }
    let args: Vec<&str> = order
        .windows(2)
        .flat_map(|pair| ["--output", pair[1].as_str(), "--right-of", pair[0].as_str()])
        .collect();
    match Command::new("xrandr").args(&args).status() {
        Ok(status) if status.success() => (),
        Ok(status) => eprintln!("{SESSION_LOG_PREFIX}: monitor order restore rejected: {status}"),
        Err(error) => eprintln!("{SESSION_LOG_PREFIX}: monitor order restore unavailable: {error}"),
    }
}
