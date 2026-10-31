//! Session-side I/O for preflight, the persistent session log and child stdio.
//!
//! Included by `bin/flamewm-session.rs` (like `session_app_compat.rs`). Classification and
//! formatting live in `flamewm_session_core::diagnostics`; this file only gathers facts.

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use flamewm_session_core::SessionPaths;
use flamewm_session_core::diagnostics::{
    DirState, Finding, LOG_ROTATE_BYTES, Severity, X_EXTENSIONS, component_finding, config_dir_for,
    dir_finding, display_finding, extension_finding, fallback_log_path, log_line, rotated_log_path,
    runtime_dir_finding, session_log_path, tool_finding,
};
use rustix::fs::{Access, access};
use rustix::process::getuid;
use x11rb::protocol::xproto::ConnectionExt as _;

struct SessionLog {
    file: File,
    path: PathBuf,
    /// True when the per-user state directory was unusable and `/tmp` received the log.
    fallback: bool,
}

static SESSION_LOG: OnceLock<SessionLog> = OnceLock::new();

/// Opens the persistent log, creating its directory and rotating an oversized previous log. A
/// failure in the state directory falls back to `/tmp`; with neither, the session still runs and
/// logs only to its own stderr. Idempotent: the first call wins.
pub fn init_session_log(home: Option<&Path>) {
    if SESSION_LOG.get().is_some() {
        return;
    }
    let fallback_path = fallback_log_path(getuid().as_raw());
    let primary = session_log_path(
        env::var_os("XDG_STATE_HOME").as_deref(),
        home,
        &fallback_path,
    );
    let opened = open_log(&primary)
        .map(|file| (file, primary.clone(), false))
        .or_else(|_| open_log(&fallback_path).map(|file| (file, fallback_path.clone(), true)));
    if let Ok((file, path, fallback)) = opened {
        let _ = SESSION_LOG.set(SessionLog {
            file,
            path,
            fallback,
        });
        log(
            "session",
            &format!(
                "start pid={} uid={} DISPLAY={}",
                std::process::id(),
                getuid().as_raw(),
                env::var("DISPLAY").unwrap_or_else(|_| "(unset)".to_owned())
            ),
        );
    }
}

fn open_log(path: &Path) -> std::io::Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::metadata(path).is_ok_and(|meta| meta.len() > LOG_ROTATE_BYTES) {
        let _ = fs::rename(path, rotated_log_path(path));
    }
    OpenOptions::new().create(true).append(true).open(path)
}

/// Writes one line to the session log and to stderr (which the display manager captures).
pub fn log(component: &str, message: &str) {
    let line = log_line(unix_seconds(), component, message);
    eprintln!("{line}");
    if let Some(session_log) = SESSION_LOG.get() {
        let mut file: &File = &session_log.file;
        let _ = writeln!(file, "{line}");
    }
}

/// Stdout and stderr for a supervised child: both appended to the session log when it is open.
pub fn child_stdio() -> (Stdio, Stdio) {
    let cloned = SESSION_LOG
        .get()
        .and_then(|log| log.file.try_clone().ok())
        .and_then(|out| out.try_clone().ok().map(|err| (out, err)));
    match cloned {
        Some((out, err)) => (Stdio::from(out), Stdio::from(err)),
        None => (Stdio::inherit(), Stdio::inherit()),
    }
}

/// Path of the session log and whether it fell back to `/tmp`.
pub fn log_location() -> Option<(PathBuf, bool)> {
    SESSION_LOG
        .get()
        .map(|log| (log.path.clone(), log.fallback))
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// An executable regular file.
pub fn is_runnable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

pub fn find_on_path(name: &str) -> bool {
    env::var_os("PATH")
        .is_some_and(|path| env::split_paths(&path).any(|dir| is_runnable(&dir.join(name))))
}

pub fn dir_state(path: &Path) -> DirState {
    match fs::metadata(path) {
        Err(_) => DirState::Missing,
        Ok(meta) if !meta.is_dir() => DirState::NotDirectory,
        Ok(_) if access(path, Access::WRITE_OK).is_ok() => DirState::Writable,
        Ok(_) => DirState::NotWritable,
    }
}

/// Existing directory state, or whether a missing directory could be created by its nearest
/// existing ancestor.
fn writable_or_creatable(path: &Path) -> DirState {
    if path.exists() {
        return dir_state(path);
    }
    match path.ancestors().find(|ancestor| ancestor.exists()) {
        Some(ancestor) if dir_state(ancestor) == DirState::Writable => DirState::Writable,
        Some(_) => DirState::NotWritable,
        None => DirState::Missing,
    }
}

/// Probes the X server and the components. Any `Fatal` finding means the session must not start.
pub fn preflight(paths: &SessionPaths, home: &Path) -> Vec<Finding> {
    let mut findings = Vec::new();
    let display = env::var("DISPLAY").ok().filter(|value| !value.is_empty());
    match display.as_deref() {
        None => findings.push(display_finding(None, None)),
        Some(name) => match x11rb::connect(None) {
            Ok((connection, _)) => {
                findings.push(display_finding(Some(name), None));
                for requirement in X_EXTENSIONS {
                    let present = connection
                        .query_extension(requirement.name.as_bytes())
                        .ok()
                        .and_then(|cookie| cookie.reply().ok())
                        .is_some_and(|reply| reply.present);
                    findings.push(extension_finding(requirement, present));
                }
            }
            Err(error) => findings.push(display_finding(Some(name), Some(&error.to_string()))),
        },
    }

    let wm = paths.flamewm_binary();
    findings.push(component_finding(
        "window manager",
        &wm,
        is_runnable(&wm),
        Severity::Fatal,
        "the session cannot start",
    ));
    let desktop = paths.desktop_binary();
    findings.push(component_finding(
        "desktop",
        &desktop,
        is_runnable(&desktop),
        Severity::Degraded,
        "a plain background and a notice are shown instead",
    ));
    let shell = paths.shell_binary();
    findings.push(component_finding(
        "shell",
        &shell,
        is_runnable(&shell),
        Severity::Degraded,
        "a plain background replaces the panel and launcher",
    ));

    findings.push(tool_finding(
        "xsetroot",
        find_on_path("xsetroot"),
        "no plain background fallback",
    ));
    findings.push(tool_finding(
        "xmessage",
        find_on_path("xmessage"),
        "fallback notices go only to the session log",
    ));

    let runtime = env::var_os("XDG_RUNTIME_DIR").filter(|value| !value.is_empty());
    let runtime_state = runtime
        .as_deref()
        .map_or(DirState::Missing, |value| dir_state(Path::new(value)));
    findings.push(runtime_dir_finding(runtime.as_deref(), runtime_state));

    let config = config_dir_for(
        env::var_os("FLAMEWM_CONFIG_DIR").as_deref(),
        paths.config_dir(),
    );
    findings.push(dir_finding(
        "config",
        &config,
        writable_or_creatable(&config),
        Severity::Degraded,
    ));

    match log_location() {
        Some((path, false)) => findings.push(Finding::new(
            Severity::Ok,
            "session log",
            format!("{} (writable)", path.display()),
        )),
        Some((path, true)) => findings.push(Finding::new(
            Severity::Degraded,
            "session log",
            format!(
                "state directory unusable; logging to {} instead",
                path.display()
            ),
        )),
        None => findings.push(Finding::new(
            Severity::Degraded,
            "session log",
            format!(
                "no writable location; logs go only to stderr (home {})",
                home.display()
            ),
        )),
    }
    findings
}

/// Shows a best-effort message window. `sh` backgrounds `xmessage` so it outlives this process:
/// a direct child would receive the supervisor's parent-death signal when the session exits.
pub fn show_failure_dialog(message: &str) {
    if env::var_os("DISPLAY").is_none() {
        return;
    }
    let _ = Command::new("/bin/sh")
        .arg("-c")
        .arg("command -v xmessage >/dev/null 2>&1 && xmessage -center \"$1\" >/dev/null 2>&1 &")
        .arg("sh")
        .arg(message)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}
