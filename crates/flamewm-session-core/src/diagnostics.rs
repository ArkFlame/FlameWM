//! Session preflight and launch diagnostics.
//!
//! Pure classification only: callers gather facts (X extensions, paths, exit statuses) and this
//! module decides severity, formats the human report, selects the log file and builds the fallback
//! process used when a non-critical component is missing. No I/O happens here.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::{ChildExit, ProcessPlan, SessionRole};

/// Severity ordered from least to most serious.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Ok,
    Info,
    Degraded,
    Fatal,
}

impl Severity {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Info => "info",
            Self::Degraded => "DEGRADED",
            Self::Fatal => "FATAL",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    pub check: String,
    pub message: String,
}

impl Finding {
    #[must_use]
    pub fn new(severity: Severity, check: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity,
            check: check.into(),
            message: message.into(),
        }
    }
}

/// Exit status of `flamewm-session --check`: 0 when nothing is wrong, 2 when only optional parts
/// are degraded, 1 when the session cannot start.
pub const CHECK_EXIT_OK: i32 = 0;
pub const CHECK_EXIT_FATAL: i32 = 1;
pub const CHECK_EXIT_DEGRADED: i32 = 2;

#[must_use]
pub fn check_exit_code(findings: &[Finding]) -> i32 {
    match findings.iter().map(|finding| finding.severity).max() {
        Some(Severity::Fatal) => CHECK_EXIT_FATAL,
        Some(Severity::Degraded) => CHECK_EXIT_DEGRADED,
        _ => CHECK_EXIT_OK,
    }
}

/// Human-readable report: one line per finding, then a verdict line.
#[must_use]
pub fn format_report(findings: &[Finding]) -> String {
    let mut out = String::from("FlameWM session check\n");
    for finding in findings {
        out.push_str(&format!(
            "  [{:<8}] {}: {}\n",
            finding.severity.label(),
            finding.check,
            finding.message
        ));
    }
    let verdict = match check_exit_code(findings) {
        CHECK_EXIT_FATAL => "FAILED: FlameWM cannot start until the FATAL items are fixed",
        CHECK_EXIT_DEGRADED => "DEGRADED: FlameWM starts with fallbacks; see the DEGRADED items",
        _ => "OK: session prerequisites are present",
    };
    out.push_str(verdict);
    out.push('\n');
    out
}

/// One X extension the session relies on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtensionRequirement {
    pub name: &'static str,
    /// Severity when the extension is absent.
    pub missing: Severity,
    pub impact: &'static str,
}

/// Extensions named by the WM sources (`flamewm-wm-x11`) and renderer. RANDR is the only one the
/// WM cannot run without; the others have explicit fallbacks in the code that uses them.
pub const X_EXTENSIONS: &[ExtensionRequirement] = &[
    ExtensionRequirement {
        name: "RANDR",
        missing: Severity::Fatal,
        impact: "the WM cannot enumerate outputs or set modes",
    },
    ExtensionRequirement {
        name: "SHAPE",
        missing: Severity::Degraded,
        impact: "rounded window corners and snap previews are not shaped",
    },
    ExtensionRequirement {
        name: "COMPOSITE",
        missing: Severity::Degraded,
        impact: "frames are not redirected; transitions may be missing",
    },
    ExtensionRequirement {
        name: "DAMAGE",
        missing: Severity::Degraded,
        impact: "window-transition damage tracking is disabled",
    },
    ExtensionRequirement {
        name: "XInputExtension",
        missing: Severity::Degraded,
        impact: "raw escape keys are disabled; passive grabs still work",
    },
    ExtensionRequirement {
        name: "RENDER",
        missing: Severity::Degraded,
        impact: "XRender-based text and image drawing falls back to core X",
    },
    ExtensionRequirement {
        name: "XFIXES",
        missing: Severity::Info,
        impact: "not used by the WM sources; reported for completeness",
    },
    ExtensionRequirement {
        name: "XKEYBOARD",
        missing: Severity::Info,
        impact: "keyboard mapping uses core X when absent",
    },
];

#[must_use]
pub fn extension_finding(requirement: &ExtensionRequirement, present: bool) -> Finding {
    let check = format!("x-extension {}", requirement.name);
    if present {
        Finding::new(Severity::Ok, check, "present")
    } else {
        Finding::new(
            requirement.missing,
            check,
            format!("missing; {}", requirement.impact),
        )
    }
}

#[must_use]
pub fn display_finding(display: Option<&str>, connect_error: Option<&str>) -> Finding {
    match (display, connect_error) {
        (None, _) => Finding::new(
            Severity::Fatal,
            "display",
            "DISPLAY is not set; start FlameWM from the display manager or inside a running X server",
        ),
        (Some(name), Some(error)) => Finding::new(
            Severity::Fatal,
            "display",
            format!(
                "X display {name} is not reachable ({error}); check that the X server runs and that XAUTHORITY allows this user"
            ),
        ),
        (Some(name), None) => Finding::new(
            Severity::Ok,
            "display",
            format!("X display {name} reachable"),
        ),
    }
}

/// Result of probing a directory the session writes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirState {
    Writable,
    NotWritable,
    NotDirectory,
    Missing,
}

/// A required executable component. A missing non-critical component is replaced by a fallback.
#[must_use]
pub fn component_finding(
    role: &str,
    path: &Path,
    runnable: bool,
    missing: Severity,
    consequence: &str,
) -> Finding {
    let check = format!("component {role}");
    if runnable {
        Finding::new(Severity::Ok, check, format!("{} present", path.display()))
    } else {
        Finding::new(
            missing,
            check,
            format!(
                "{} is missing or not executable; {consequence}",
                path.display()
            ),
        )
    }
}

/// An optional helper tool found or absent on PATH.
#[must_use]
pub fn tool_finding(name: &str, found: bool, consequence: &str) -> Finding {
    let check = format!("tool {name}");
    if found {
        Finding::new(Severity::Ok, check, "found")
    } else {
        Finding::new(Severity::Info, check, format!("not found; {consequence}"))
    }
}

#[must_use]
pub fn dir_finding(label: &str, path: &Path, state: DirState, missing: Severity) -> Finding {
    let check = format!("directory {label}");
    let shown = path.display();
    match state {
        DirState::Writable => Finding::new(Severity::Ok, check, format!("{shown} writable")),
        DirState::Missing => Finding::new(
            missing,
            check,
            format!("{shown} does not exist and could not be created"),
        ),
        DirState::NotDirectory => Finding::new(
            missing,
            check,
            format!("{shown} exists but is not a directory"),
        ),
        DirState::NotWritable => Finding::new(
            missing,
            check,
            format!("{shown} is not writable by this user"),
        ),
    }
}

#[must_use]
pub fn runtime_dir_finding(value: Option<&OsStr>, state: DirState) -> Finding {
    match value {
        None => Finding::new(
            Severity::Degraded,
            "XDG_RUNTIME_DIR",
            "not set; D-Bus session services and control sockets may fail to start",
        ),
        Some(value) => dir_finding(
            "XDG_RUNTIME_DIR",
            Path::new(value),
            state,
            Severity::Degraded,
        ),
    }
}

/// The config directory preflight reports: an inherited non-empty `FLAMEWM_CONFIG_DIR` wins over
/// the session default, as it does in the environment handed to the window manager.
#[must_use]
pub fn config_dir_for(config_override: Option<&OsStr>, default: PathBuf) -> PathBuf {
    config_override
        .filter(|value| !value.is_empty())
        .map_or(default, PathBuf::from)
}

/// The persistent session log: `$XDG_STATE_HOME/flamewm/session.log`, then
/// `$HOME/.local/state/flamewm/session.log`, then `fallback`.
#[must_use]
pub fn session_log_path(
    xdg_state_home: Option<&OsStr>,
    home: Option<&Path>,
    fallback: &Path,
) -> PathBuf {
    let base = xdg_state_home
        .filter(|value| !value.is_empty() && Path::new(value).is_absolute())
        .map(PathBuf::from)
        .or_else(|| home.map(|home| home.join(".local").join("state")));
    match base {
        Some(base) => base.join("flamewm").join("session.log"),
        None => fallback.to_path_buf(),
    }
}

/// Per-user fallback log under `/tmp` when the state directory is unusable.
#[must_use]
pub fn fallback_log_path(uid: u32) -> PathBuf {
    PathBuf::from(format!("/tmp/flamewm-session-{uid}.log"))
}

/// Log size at which the previous log is rotated to `<name>.1` before a new session starts.
pub const LOG_ROTATE_BYTES: u64 = 1 << 20;

#[must_use]
pub fn rotated_log_path(path: &Path) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(".1");
    PathBuf::from(name)
}

/// One timestamped log line: `[unix-seconds] component: message`.
#[must_use]
pub fn log_line(unix_seconds: u64, component: &str, message: &str) -> String {
    format!("[{unix_seconds}] {component}: {message}")
}

#[must_use]
pub fn describe_exit(exit: ChildExit) -> String {
    match exit {
        ChildExit::Success => "exited with status 0".to_owned(),
        ChildExit::ExitCode(code) => format!("exited with status {code}"),
        ChildExit::Signal(signal) => format!("killed by signal {signal}"),
        ChildExit::SpawnFailed => "could not be spawned".to_owned(),
    }
}

/// POSIX single-quote escaping so `text` is one literal word in `sh -c`.
#[must_use]
pub fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Script run by the replacement for a missing desktop or shell component. It paints a plain
/// root background (when `xsetroot` exists), prints the reason to stderr (the session log), shows
/// an `xmessage` notice for the desktop role, and then stays alive so the supervisor's role
/// indexing and WM readiness are unchanged.
#[must_use]
pub fn fallback_script(role: &str, binary: &str, log: &str, dialog: bool) -> String {
    let message = format!(
        "FlameWM: the {role} component ({binary}) is missing or failed. The window manager is running without it. Details: {log}. Run flamewm-session --check for a full report."
    );
    let quoted = shell_quote(&message);
    let mut script = format!(
        "msg={quoted}\n\
         command -v xsetroot >/dev/null 2>&1 && xsetroot -solid '#1e1e1e' 2>/dev/null\n\
         echo \"$msg\" >&2\n"
    );
    if dialog {
        script.push_str(
            "command -v xmessage >/dev/null 2>&1 && xmessage -center \"$msg\" >/dev/null 2>&1 &\n",
        );
    }
    script.push_str("exec sleep 2147483647\n");
    script
}

/// Replacement plan for a non-critical component that cannot run.
#[must_use]
pub fn fallback_plan(original: &ProcessPlan, log: &str) -> ProcessPlan {
    let (role_name, dialog) = match original.role {
        SessionRole::Desktop => ("desktop", true),
        SessionRole::Shell => ("shell", false),
        SessionRole::WindowManager => ("window manager", false),
    };
    let binary = original.program.display().to_string();
    let script = fallback_script(role_name, &binary, log, dialog);
    ProcessPlan {
        role: original.role,
        program: PathBuf::from("/bin/sh"),
        arguments: vec![
            "-c".to_owned(),
            script,
            format!("flamewm-fallback-{role_name}"),
        ],
        environment: original.environment.clone(),
        required: false,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn plan(role: SessionRole, program: &str) -> ProcessPlan {
        ProcessPlan {
            role,
            program: PathBuf::from(program),
            arguments: Vec::new(),
            environment: BTreeMap::from([("DISPLAY".to_owned(), ":0".to_owned())]),
            required: true,
        }
    }

    #[test]
    fn check_exit_code_orders_fatal_over_degraded_over_ok() {
        let ok = [Finding::new(Severity::Ok, "a", "x")];
        let degraded = [
            Finding::new(Severity::Ok, "a", "x"),
            Finding::new(Severity::Degraded, "b", "y"),
        ];
        let fatal = [
            Finding::new(Severity::Degraded, "b", "y"),
            Finding::new(Severity::Fatal, "c", "z"),
        ];
        assert_eq!(check_exit_code(&[]), CHECK_EXIT_OK);
        assert_eq!(check_exit_code(&ok), CHECK_EXIT_OK);
        assert_eq!(
            check_exit_code(&[Finding::new(Severity::Info, "t", "m")]),
            CHECK_EXIT_OK
        );
        assert_eq!(check_exit_code(&degraded), CHECK_EXIT_DEGRADED);
        assert_eq!(check_exit_code(&fatal), CHECK_EXIT_FATAL);
    }

    #[test]
    fn report_lists_every_finding_and_verdict() {
        let findings = [
            Finding::new(Severity::Ok, "display", "X display :0 reachable"),
            Finding::new(Severity::Degraded, "component desktop", "missing"),
        ];
        let report = format_report(&findings);
        assert!(report.starts_with("FlameWM session check\n"));
        assert!(report.contains("[ok      ] display: X display :0 reachable\n"));
        assert!(report.contains("[DEGRADED] component desktop: missing\n"));
        assert!(
            report.ends_with("DEGRADED: FlameWM starts with fallbacks; see the DEGRADED items\n")
        );
    }

    #[test]
    fn report_verdict_for_fatal_names_the_failure() {
        let report = format_report(&[display_finding(None, None)]);
        assert!(report.contains("[FATAL   ] display: DISPLAY is not set"));
        assert!(report.ends_with("FAILED: FlameWM cannot start until the FATAL items are fixed\n"));
    }

    #[test]
    fn display_finding_covers_unset_unreachable_and_reachable() {
        assert_eq!(display_finding(None, None).severity, Severity::Fatal);
        let unreachable = display_finding(Some(":191"), Some("connection refused"));
        assert_eq!(unreachable.severity, Severity::Fatal);
        assert!(unreachable.message.contains(":191"));
        assert!(unreachable.message.contains("connection refused"));
        assert_eq!(display_finding(Some(":191"), None).severity, Severity::Ok);
    }

    #[test]
    fn randr_absence_is_fatal_and_optional_extensions_degrade() {
        let randr = X_EXTENSIONS
            .iter()
            .find(|e| e.name == "RANDR")
            .expect("RANDR listed");
        assert_eq!(extension_finding(randr, false).severity, Severity::Fatal);
        assert_eq!(extension_finding(randr, true).severity, Severity::Ok);
        let composite = X_EXTENSIONS
            .iter()
            .find(|e| e.name == "COMPOSITE")
            .expect("COMPOSITE listed");
        assert_eq!(
            extension_finding(composite, false).severity,
            Severity::Degraded
        );
        assert_eq!(
            extension_finding(composite, false).check,
            "x-extension COMPOSITE"
        );
    }

    #[test]
    fn component_and_tool_findings_follow_runnable_state() {
        let missing = component_finding(
            "desktop",
            Path::new("/usr/bin/flamewm-desktop"),
            false,
            Severity::Degraded,
            "a fallback runs",
        );
        assert_eq!(missing.severity, Severity::Degraded);
        assert!(
            missing
                .message
                .contains("/usr/bin/flamewm-desktop is missing")
        );
        assert_eq!(
            component_finding(
                "WM",
                Path::new("/usr/bin/flamewm"),
                true,
                Severity::Fatal,
                "x"
            )
            .severity,
            Severity::Ok
        );
        assert_eq!(
            tool_finding("xmessage", false, "no dialog").severity,
            Severity::Info
        );
        assert_eq!(tool_finding("xsetroot", true, "x").severity, Severity::Ok);
    }

    #[test]
    fn runtime_dir_unset_or_unwritable_is_degraded() {
        assert_eq!(
            runtime_dir_finding(None, DirState::Missing).severity,
            Severity::Degraded
        );
        let bad = runtime_dir_finding(Some(OsStr::new("/run/user/1000")), DirState::NotWritable);
        assert_eq!(bad.severity, Severity::Degraded);
        assert_eq!(
            runtime_dir_finding(Some(OsStr::new("/run/user/1000")), DirState::Writable).severity,
            Severity::Ok
        );
    }

    #[test]
    fn config_dir_honors_inherited_override_only_when_non_empty() {
        let default = PathBuf::from("/home/u/.config/flamewm");
        assert_eq!(
            config_dir_for(Some(OsStr::new("/scratch/cfg")), default.clone()),
            PathBuf::from("/scratch/cfg")
        );
        assert_eq!(
            config_dir_for(Some(OsStr::new("")), default.clone()),
            default
        );
        assert_eq!(config_dir_for(None, default.clone()), default);
    }

    #[test]
    fn log_path_prefers_xdg_state_then_home_then_fallback() {
        let fallback = fallback_log_path(1000);
        assert_eq!(
            session_log_path(
                Some(OsStr::new("/state")),
                Some(Path::new("/home/u")),
                &fallback
            ),
            PathBuf::from("/state/flamewm/session.log")
        );
        assert_eq!(
            session_log_path(
                Some(OsStr::new("relative")),
                Some(Path::new("/home/u")),
                &fallback
            ),
            PathBuf::from("/home/u/.local/state/flamewm/session.log")
        );
        assert_eq!(
            session_log_path(None, None, &fallback),
            PathBuf::from("/tmp/flamewm-session-1000.log")
        );
    }

    #[test]
    fn rotated_path_appends_suffix() {
        assert_eq!(
            rotated_log_path(Path::new("/s/flamewm/session.log")),
            PathBuf::from("/s/flamewm/session.log.1")
        );
    }

    #[test]
    fn exit_descriptions_are_human_readable() {
        assert_eq!(
            describe_exit(ChildExit::ExitCode(3)),
            "exited with status 3"
        );
        assert_eq!(describe_exit(ChildExit::Signal(9)), "killed by signal 9");
        assert_eq!(
            describe_exit(ChildExit::SpawnFailed),
            "could not be spawned"
        );
        assert_eq!(log_line(7, "desktop", "started"), "[7] desktop: started");
    }

    #[test]
    fn shell_quote_keeps_single_quotes_literal() {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
        assert_eq!(shell_quote("$(rm)"), "'$(rm)'");
    }

    #[test]
    fn fallback_plan_keeps_role_environment_and_stays_alive() {
        let original = plan(SessionRole::Desktop, "/usr/bin/flamewm-desktop");
        let fallback = fallback_plan(&original, "/state/flamewm/session.log");
        assert_eq!(fallback.role, SessionRole::Desktop);
        assert_eq!(fallback.program, PathBuf::from("/bin/sh"));
        assert_eq!(fallback.environment, original.environment);
        assert_eq!(fallback.arguments[0], "-c");
        let script = &fallback.arguments[1];
        assert!(script.contains("the desktop component (/usr/bin/flamewm-desktop)"));
        assert!(script.contains("xsetroot -solid"));
        assert!(script.contains("xmessage -center"));
        assert!(script.ends_with("exec sleep 2147483647\n"));
    }

    #[test]
    fn shell_fallback_has_no_dialog() {
        let original = plan(SessionRole::Shell, "/usr/bin/flamewm-shell");
        let fallback = fallback_plan(&original, "/log");
        assert!(!fallback.arguments[1].contains("xmessage"));
        assert!(fallback.arguments[1].contains("the shell component"));
    }

    #[test]
    fn fallback_message_quotes_hostile_paths() {
        let script = fallback_script("desktop", "/x/it's $(bad)", "/log", false);
        assert!(script.starts_with("msg='FlameWM: the desktop component (/x/it'\\''s $(bad))"));
    }
}
