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
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use flamewm_control_core::{ControlRequest, ControlResponse};
use flamewm_control_dbus::ControlClient;
use flamewm_dbus_reactor::BusKind;
use flamewm_session_core::diagnostics::{
    BlastRadius, CHECK_EXIT_FATAL, FailureCode, FailureRecord, Severity, append_record,
    check_exit_code, describe_exit, fallback_plan, fallback_plan_with_cause, format_report,
    format_startup_failure, unix_seconds_now,
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

/// Record a startup-stage failure that is not a supervisor abort. A readable line goes to the
/// session log and the typed failure record is appended to the resolved failure record file (the
/// same session log), built by the pure formatter so stage, code, cause and blast radius stay
/// comparable with supervisor records.
fn record_startup_failure(
    stage: &str,
    cause: &str,
    still_works: &[&str],
    degraded: &[&str],
    fallback_taken: bool,
) {
    let record = format_startup_failure(
        stage,
        FailureCode::SubsystemError,
        cause,
        still_works,
        degraded,
        fallback_taken,
        unix_seconds_now(),
    );
    session_diagnostics::log(stage, &format!("{}: {cause}", record.code.as_str()));
    record_failure(&record);
}

/// Install directory layout and the user's home, shared by the launch and `--check` paths.
fn session_paths() -> Result<(flamewm_session_core::SessionPaths, PathBuf), String> {
    let executable =
        env::current_exe().map_err(|error| format!("locate session executable: {error}"))?;
    let bindir = executable
        .parent()
        .ok_or_else(|| "session executable has no parent directory".to_owned())?
        .to_path_buf();
    let home = match resolve_home(
        env::var_os("HOME"),
        Path::new(PASSWD_PATH),
        Path::new(PROC_STATUS_PATH),
    ) {
        Ok(resolved) => {
            if resolved.from_passwd {
                session_diagnostics::log(
                    "session-paths",
                    &format!(
                        "notice: HOME unset; using passwd home {} (blast_radius: session config and state paths follow this home; child processes keep the inherited environment)",
                        resolved.path.display()
                    ),
                );
            }
            resolved.path
        }
        Err(cause) => {
            record_startup_failure("session-paths", &cause, &[], &["session"], false);
            return Err(cause);
        }
    };
    let mut paths = default_paths_from_home(&bindir, home.clone());
    if let Some(config) = env::var_os("XDG_CONFIG_HOME") {
        paths.config_home = PathBuf::from(config);
    }
    Ok((paths, home))
}

const PASSWD_PATH: &str = "/etc/passwd";
const PROC_STATUS_PATH: &str = "/proc/self/status";

/// A resolved home directory and whether it came from the passwd fallback rather than `HOME`.
#[derive(Debug, PartialEq, Eq)]
struct ResolvedHome {
    path: PathBuf,
    from_passwd: bool,
}

/// The user's home: `HOME` when set and non-empty, else the passwd home of the current real uid
/// (read from `status`, looked up in `passwd`). Fails only when both are unusable. `/root` is never
/// guessed; it is used only when the passwd entry itself names it.
fn resolve_home(
    home: Option<std::ffi::OsString>,
    passwd: &Path,
    status: &Path,
) -> Result<ResolvedHome, String> {
    if let Some(home) = home.filter(|home| !home.is_empty()) {
        return Ok(ResolvedHome {
            path: PathBuf::from(home),
            from_passwd: false,
        });
    }
    let status_text = std::fs::read_to_string(status).map_err(|error| {
        format!(
            "HOME is unset and {} is unreadable: {error}",
            status.display()
        )
    })?;
    let uid = uid_from_status(&status_text)
        .ok_or_else(|| format!("HOME is unset and no Uid line in {}", status.display()))?;
    let passwd_text = std::fs::read_to_string(passwd).map_err(|error| {
        format!(
            "HOME is unset and {} is unreadable: {error}",
            passwd.display()
        )
    })?;
    match home_from_passwd(&passwd_text, uid) {
        Some(path) => Ok(ResolvedHome {
            path,
            from_passwd: true,
        }),
        None => Err(format!(
            "HOME is unset and {} has no home for uid {uid}",
            passwd.display()
        )),
    }
}

/// The real uid from the `Uid:` line of a `/proc/<pid>/status` body (first field).
fn uid_from_status(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|real| real.parse().ok())
}

/// The home field of the first passwd entry (`name:x:uid:gid:gecos:home:shell`) for `uid`.
/// Entries with an empty or relative home are skipped.
fn home_from_passwd(passwd: &str, uid: u32) -> Option<PathBuf> {
    passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        let _name = fields.next()?;
        let _password = fields.next()?;
        let entry_uid: u32 = fields.next()?.parse().ok()?;
        if entry_uid != uid {
            return None;
        }
        let _gid = fields.next()?;
        let _gecos = fields.next()?;
        let home = fields.next()?;
        (home.starts_with('/')).then(|| PathBuf::from(home))
    })
}

#[cfg(test)]
mod home_resolution_tests {
    use super::{ResolvedHome, home_from_passwd, resolve_home, uid_from_status};
    use std::path::{Path, PathBuf};

    const STATUS: &str = "Name:\tsession\nUid:\t1000\t1000\t1000\t1000\nGid:\t1000\t1000\n";

    fn temp_file(name: &str, body: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("flamewm-session-{}-{name}", std::process::id()));
        std::fs::write(&path, body).expect("write temp file");
        path
    }

    #[test]
    fn env_home_wins_and_is_not_a_fallback() {
        let passwd = temp_file("env-wins-passwd", "");
        let status = temp_file("env-wins-status", STATUS);
        assert_eq!(
            resolve_home(Some("/home/user".into()), &passwd, &status),
            Ok(ResolvedHome {
                path: PathBuf::from("/home/user"),
                from_passwd: false,
            })
        );
        let _ = std::fs::remove_file(passwd);
        let _ = std::fs::remove_file(status);
    }

    #[test]
    fn unset_or_empty_home_uses_passwd_entry_for_current_uid() {
        let passwd = temp_file(
            "uid-match-passwd",
            "root:x:0:0:root:/root:/bin/sh\nuser:x:1000:1000::/home/user:/bin/sh\n",
        );
        let status = temp_file("uid-match-status", STATUS);
        let expected = Ok(ResolvedHome {
            path: PathBuf::from("/home/user"),
            from_passwd: true,
        });
        assert_eq!(resolve_home(None, &passwd, &status), expected);
        assert_eq!(
            resolve_home(Some(std::ffi::OsString::new()), &passwd, &status),
            expected
        );
        let _ = std::fs::remove_file(passwd);
        let _ = std::fs::remove_file(status);
    }

    #[test]
    fn fails_when_passwd_has_no_usable_home_and_never_guesses_root() {
        let passwd = temp_file("no-entry-passwd", "root:x:0:0:root:/root:/bin/sh\n");
        let status = temp_file("no-entry-status", STATUS);
        assert!(resolve_home(None, &passwd, &status).is_err());
        let _ = std::fs::remove_file(passwd);
        let _ = std::fs::remove_file(status);
    }

    #[test]
    fn fails_when_status_is_unreadable() {
        let passwd = temp_file("no-status-passwd", "user:x:1000:1000::/home/user:/bin/sh\n");
        let missing = Path::new("/nonexistent/flamewm-status");
        assert!(resolve_home(None, &passwd, missing).is_err());
        let _ = std::fs::remove_file(passwd);
    }

    #[test]
    fn passwd_parsing_matches_uid_exactly_and_skips_unusable_homes() {
        let passwd = "a:x:10:10::/home/a:/bin/sh\n\
                      b:x:1000:1000::relative:/bin/sh\n\
                      c:x:1000:1000::/home/c:/bin/sh\n\
                      d:x:100:100::/home/d:/bin/sh\n";
        assert_eq!(home_from_passwd(passwd, 10), Some(PathBuf::from("/home/a")));
        assert_eq!(
            home_from_passwd(passwd, 100),
            Some(PathBuf::from("/home/d"))
        );
        assert_eq!(
            home_from_passwd(passwd, 1000),
            Some(PathBuf::from("/home/c"))
        );
        assert_eq!(home_from_passwd(passwd, 1), None);
        assert_eq!(
            home_from_passwd("x:x:5:5::/:/bin/sh\n", 5),
            Some(PathBuf::from("/"))
        );
        assert_eq!(home_from_passwd("x:x:5:5::", 5), None);
    }

    #[test]
    fn real_uid_is_the_first_uid_field() {
        assert_eq!(uid_from_status(STATUS), Some(1000));
        assert_eq!(uid_from_status("Name:\tx\n"), None);
    }
}

/// Start the window manager with bounded readiness recovery. A WM that exits before EWMH
/// readiness, or a spawn that fails, is recorded as a typed failure; restarts use the supervisor
/// backoff, and when the budget is exhausted the next backend (the fallback plan) is started so
/// the desktop and shell can still be launched.
fn start_wm_or_fallback(
    running: &mut Vec<Running>,
    role_plans: &mut [ProcessPlan],
    parent: Pid,
    stop_requested: &AtomicBool,
) -> Result<(), String> {
    let mut supervisor = ProcessSupervisorPolicy::default();
    let original = role_plans[0].clone();
    loop {
        check_stop(stop_requested)?;
        let mut child = match spawn(role_plans[0].clone()) {
            Ok(child) => child,
            Err(error) => {
                let decision = supervisor.child_exited(ChildExit::SpawnFailed);
                let recovery = abort_recovery(
                    SessionRole::WindowManager,
                    &error,
                    decision,
                    decision == SupervisorDecision::GiveUp,
                    unix_seconds_now(),
                );
                record_failure(&recovery.record);
                match recovery.decision {
                    SupervisorDecision::GiveUp => {
                        role_plans[0] = fallback_plan(&original, &log_path_text());
                        running.push(spawn(role_plans[0].clone())?);
                        return Ok(());
                    }
                    SupervisorDecision::RestartAfter(delay) => {
                        wait_backoff(delay, parent, stop_requested)?;
                        continue;
                    }
                    SupervisorDecision::StopCleanly => return Err(SHUTDOWN_SIGNAL.to_owned()),
                    SupervisorDecision::SpawnNow => {
                        unreachable!("child_exited never requests SpawnNow")
                    }
                }
            }
        };
        match wait_ready(&mut child, parent, stop_requested) {
            Ok(()) => {
                running.push(child);
                return Ok(());
            }
            Err(error) => {
                let observed = child.child.try_wait().ok().flatten();
                let exit = observed.map_or(ChildExit::ExitCode(1), status_to_exit);
                let _ = stop_role(&mut child);
                let decision = supervisor.child_exited(exit);
                let recovery = abort_recovery(
                    SessionRole::WindowManager,
                    &error,
                    decision,
                    decision == SupervisorDecision::GiveUp,
                    unix_seconds_now(),
                );
                record_failure(&recovery.record);
                match recovery.decision {
                    SupervisorDecision::GiveUp => {
                        role_plans[0] = fallback_plan(&original, &log_path_text());
                        running.push(spawn(role_plans[0].clone())?);
                        return Ok(());
                    }
                    SupervisorDecision::RestartAfter(delay) => {
                        wait_backoff(delay, parent, stop_requested)?;
                    }
                    SupervisorDecision::StopCleanly => return Err(SHUTDOWN_SIGNAL.to_owned()),
                    SupervisorDecision::SpawnNow => {
                        unreachable!("child_exited never requests SpawnNow")
                    }
                }
            }
        }
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
        let cause = format!("preflight failed: {}", fatal.join("; "));
        record_startup_failure("preflight", cause.as_str(), &[], &["session"], false);
        return Err(cause);
    }
    let existing = env::vars().collect::<BTreeMap<_, _>>();
    let args = env::args().skip(1).collect::<Vec<_>>();
    let mut plan = build_launch_plan(&paths, &args, &existing);
    add_app_compatibility_environment(&mut plan.processes, &existing);
    // Generate only FlameWM-owned XDG overlays; never mutate existing Brave
    // profiles, wallet contents, Plasma configuration, or desktop identities.
    if let Err(error) = session_app_compat::configure(&home, &mut plan.processes) {
        let cause = format!("KDE app compatibility degraded: {error}");
        record_startup_failure(
            "app compatibility",
            cause.as_str(),
            &["window manager", "desktop", "shell"],
            &["kde app compatibility"],
            false,
        );
    }
    if plan.processes.len() != 3
        || plan.processes[0].role != SessionRole::WindowManager
        || plan.processes[1].role != SessionRole::Desktop
        || plan.processes[2].role != SessionRole::Shell
    {
        let cause =
            "session launch plan does not contain WM, Desktop, Shell in required order".to_owned();
        record_startup_failure("launch plan", cause.as_str(), &[], &["session"], false);
        return Err(cause);
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
        start_wm_or_fallback(&mut running, &mut role_plans, parent, &stop_requested)?;
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
