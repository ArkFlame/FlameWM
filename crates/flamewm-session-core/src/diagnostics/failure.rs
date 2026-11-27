//! Failure reports, panic boundaries and crash-loop-limited subsystem restarts.
//!
//! This is the typed owner for "something failed and the session kept going". Records are pure
//! values; [`FailureRecord::to_log_line`] is a stable one-line `key=value` serialization and
//! [`append_record`] is the only filesystem write. [`guard`] is the opt-in panic boundary that
//! keeps the process alive; [`install_panic_hook`] records panics, including ones that escape a
//! guard, with the name of the subsystem active on that thread.

use std::any::Any;
use std::cell::RefCell;
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::panic::{self, AssertUnwindSafe, PanicHookInfo};
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::{ChildExit, ProcessSupervisorPolicy, SupervisorDecision, SupervisorPolicy};

/// Stable machine-readable failure codes. The string form is part of the log contract: adding a
/// variant must not change the text of an existing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureCode {
    /// A panic was captured by [`guard`] or observed by the installed hook.
    Panic,
    /// A restart was refused because the per-window limit was reached.
    RestartLimit,
    /// A subsystem returned a recoverable error.
    SubsystemError,
}

impl FailureCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Panic => "panic",
            Self::RestartLimit => "restart_limit",
            Self::SubsystemError => "subsystem_error",
        }
    }
}

/// What the failure took down. Names are subsystem ids (for example `wm`, `panel`, `dock`), not
/// free-form prose, so a reader can diff one record against another.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlastRadius {
    /// Subsystems still fully working after the failure.
    pub still_works: Vec<String>,
    /// Subsystems that remain present but with lost functionality.
    pub degraded: Vec<String>,
}

impl BlastRadius {
    #[must_use]
    pub fn new(
        still_works: impl IntoIterator<Item = impl Into<String>>,
        degraded: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            still_works: still_works.into_iter().map(Into::into).collect(),
            degraded: degraded.into_iter().map(Into::into).collect(),
        }
    }
}

/// One failure in a session. `timestamp` is unix seconds; callers that build records away from a
/// clock can pass an injected value and keep the rest of the module pure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureRecord {
    pub stage: String,
    pub code: FailureCode,
    pub cause: String,
    pub blast_radius: BlastRadius,
    pub fallback_taken: bool,
    pub timestamp: u64,
}

impl FailureRecord {
    #[must_use]
    pub fn new(
        stage: impl Into<String>,
        code: FailureCode,
        cause: impl Into<String>,
        blast_radius: BlastRadius,
        fallback_taken: bool,
        timestamp: u64,
    ) -> Self {
        Self {
            stage: stage.into(),
            code,
            cause: cause.into(),
            blast_radius,
            fallback_taken,
            timestamp,
        }
    }

    /// Convenience constructor using [`unix_seconds_now`] for the timestamp.
    #[must_use]
    pub fn at_now(
        stage: impl Into<String>,
        code: FailureCode,
        cause: impl Into<String>,
        blast_radius: BlastRadius,
        fallback_taken: bool,
    ) -> Self {
        Self::new(
            stage,
            code,
            cause,
            blast_radius,
            fallback_taken,
            unix_seconds_now(),
        )
    }

    /// One line, no embedded newline: `timestamp=<u64> stage=<quoted> code=<code>
    /// cause=<quoted> still_works=<quoted-list> degraded=<quoted-list> fallback_taken=<bool>`.
    /// Quoted values escape `\`, `"`, tab, CR, LF and other control characters; list items are
    /// comma-separated with commas inside a name escaped as `\,`.
    #[must_use]
    pub fn to_log_line(&self) -> String {
        format!(
            "timestamp={} stage={} code={} cause={} still_works={} degraded={} fallback_taken={}",
            self.timestamp,
            quote(&self.stage),
            self.code.as_str(),
            quote(&self.cause),
            quote_names(&self.blast_radius.still_works),
            quote_names(&self.blast_radius.degraded),
            self.fallback_taken
        )
    }
}

fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other if other.is_control() => {
                let code = u32::from(other);
                let _ = write!(out, "\\u{code:04x}");
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn quote_names(names: &[String]) -> String {
    let joined = names
        .iter()
        .map(|name| {
            name.replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace(',', "\\,")
                .replace('\n', "\\n")
                .replace('\r', "\\r")
                .replace('\t', "\\t")
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("\"{joined}\"")
}

#[must_use]
pub fn unix_seconds_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Append one failure record to a log file, creating the file (and its parent directory) if
/// needed. The session log path comes from [`super::session_log_path`]; this helper never rotates
/// or truncates it. Failure to write is returned to the caller, never raised as a panic.
pub fn append_record(path: &Path, record: &FailureRecord) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{}", record.to_log_line())
}

thread_local! {
    /// Subsystem whose panic boundary is active on this thread; the hook reads it while the
    /// panicking thread is still unwinding.
    static CURRENT_SUBSYSTEM: RefCell<Option<String>> = RefCell::new(None);
}

static HOOK_INSTALLED: Once = Once::new();

fn current_subsystem() -> String {
    CURRENT_SUBSYSTEM.with(|slot| {
        slot.borrow()
            .as_ref()
            .map_or_else(|| "process".to_owned(), |name| name.clone())
    })
}

fn payload_text(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "panic without a readable message".to_owned()
    }
}

/// Install a panic hook that appends one [`FailureRecord`] with code `panic` to `log_path` for
/// every panic, then calls the previously installed hook. The subsystem is the name passed to the
/// innermost active [`guard`], or `process` for a panic outside any guard. Installation is
/// idempotent: the first call wins, so pass the resolved `session_log_path` once at process start.
/// The hook records and continues; keeping the process alive is the caller's opt-in via [`guard`].
pub fn install_panic_hook(log_path: impl Into<PathBuf>) {
    let path = log_path.into();
    HOOK_INSTALLED.call_once(move || {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info: &PanicHookInfo<'_>| {
            let record = FailureRecord::new(
                current_subsystem(),
                FailureCode::Panic,
                payload_text(info.payload()),
                BlastRadius::default(),
                false,
                unix_seconds_now(),
            );
            let _ = append_record(&path, &record);
            previous(info);
        }));
    });
}

/// Run `operation` behind an unwind boundary. A panic becomes `Err(FailureRecord)` instead of
/// killing the process, so the caller can start a fallback or skip just this subsystem. The
/// subsystem name is visible to an installed panic hook for the duration of the call; nesting is
/// supported and restores the outer name on return.
pub fn guard<F, T>(subsystem: impl Into<String>, operation: F) -> Result<T, FailureRecord>
where
    F: FnOnce() -> T,
{
    let subsystem = subsystem.into();
    let previous = CURRENT_SUBSYSTEM.with(|slot| slot.borrow_mut().replace(subsystem.clone()));
    let outcome = panic::catch_unwind(AssertUnwindSafe(operation));
    CURRENT_SUBSYSTEM.with(|slot| {
        *slot.borrow_mut() = previous;
    });
    outcome.map_err(|payload| {
        FailureRecord::new(
            subsystem,
            FailureCode::Panic,
            payload_text(payload.as_ref()),
            BlastRadius::default(),
            false,
            unix_seconds_now(),
        )
    })
}

/// Outcome of one subsystem failure under [`RestartPolicy`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartDecision {
    /// Restart after this many milliseconds of backoff.
    Restart { delay_ms: u64 },
    /// The crash-loop guard refused another restart; record a [`FailureCode::RestartLimit`].
    GiveUp,
}

/// Crash-loop guard for in-process subsystem restarts. It reuses the session's canonical
/// supervisor policy, so the window and backoff semantics stay identical to child-process
/// supervision. Decisions are clock-injected via [`RestartPolicy::note_failure_at`] for tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestartPolicy {
    supervisor: ProcessSupervisorPolicy,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self::new(5, 60_000)
    }
}

impl RestartPolicy {
    #[must_use]
    pub fn new(maximum_restarts: u8, window_ms: u64) -> Self {
        let policy = SupervisorPolicy {
            maximum_restarts,
            restart_window_ms: window_ms,
            ..SupervisorPolicy::default()
        };
        Self {
            supervisor: ProcessSupervisorPolicy::new(policy),
        }
    }

    pub fn note_failure_at(&mut self, now: Instant) -> RestartDecision {
        let exit = ChildExit::ExitCode(1);
        match self.supervisor.child_exited_at(exit, now) {
            SupervisorDecision::RestartAfter(delay_ms) => RestartDecision::Restart { delay_ms },
            SupervisorDecision::SpawnNow => RestartDecision::Restart { delay_ms: 0 },
            SupervisorDecision::GiveUp | SupervisorDecision::StopCleanly => RestartDecision::GiveUp,
        }
    }

    pub fn note_failure(&mut self) -> RestartDecision {
        self.note_failure_at(Instant::now())
    }

    #[must_use]
    pub const fn restarts(&self) -> u8 {
        self.supervisor.restarts()
    }

    #[must_use]
    pub const fn gave_up(&self) -> bool {
        self.supervisor.gave_up()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn code_strings_are_stable() {
        assert_eq!(FailureCode::Panic.as_str(), "panic");
        assert_eq!(FailureCode::RestartLimit.as_str(), "restart_limit");
        assert_eq!(FailureCode::SubsystemError.as_str(), "subsystem_error");
    }

    #[test]
    fn record_serializes_as_one_key_value_line() {
        let record = FailureRecord::new(
            "wm",
            FailureCode::RestartLimit,
            "line one\nline \"two\"",
            BlastRadius::new(["x11", "shell"], ["compositor"]),
            true,
            42,
        );
        assert_eq!(
            record.to_log_line(),
            "timestamp=42 stage=\"wm\" code=restart_limit cause=\"line one\\nline \\\"two\\\"\" still_works=\"x11,shell\" degraded=\"compositor\" fallback_taken=true"
        );
        assert!(!record.to_log_line().contains('\n'));
    }

    #[test]
    fn names_with_delimiters_stay_inside_one_value() {
        let record = FailureRecord::new(
            "panel",
            FailureCode::SubsystemError,
            "back\\slash",
            BlastRadius::new(["dock,tray"], Vec::<&str>::new()),
            false,
            7,
        );
        assert_eq!(
            record.to_log_line(),
            "timestamp=7 stage=\"panel\" code=subsystem_error cause=\"back\\\\slash\" still_works=\"dock\\,tray\" degraded=\"\" fallback_taken=false"
        );
    }

    #[test]
    fn guard_returns_value_when_nothing_panics() {
        assert_eq!(guard("shell", || 40 + 2), Ok(42));
        CURRENT_SUBSYSTEM.with(|slot| assert!(slot.borrow().is_none()));
    }

    #[test]
    fn guard_captures_panic_as_a_failure_record() {
        let result: Result<(), FailureRecord> = guard("panel", || panic!("panel thread died"));
        let record = result.expect_err("the panic is captured");
        assert_eq!(record.stage, "panel");
        assert_eq!(record.code, FailureCode::Panic);
        assert!(record.cause.contains("panel thread died"));
        assert_eq!(record.blast_radius, BlastRadius::default());
        assert!(!record.fallback_taken);
        CURRENT_SUBSYSTEM.with(|slot| assert!(slot.borrow().is_none()));
    }

    #[test]
    fn nested_guard_restores_the_outer_subsystem() {
        let outcome = guard("shell", || {
            assert_eq!(guard("tray", || 1), Ok(1));
            let failed: Result<(), FailureRecord> = guard("dock", || panic!("dock failed"));
            assert_eq!(failed.expect_err("captured").stage, "dock");
            CURRENT_SUBSYSTEM.with(|slot| assert_eq!(slot.borrow().as_deref(), Some("shell")));
        });
        assert!(outcome.is_ok());
        CURRENT_SUBSYSTEM.with(|slot| assert!(slot.borrow().is_none()));
    }

    #[test]
    fn crash_loop_stops_after_the_window_limit() {
        let mut policy = RestartPolicy::new(3, 1_000);
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        assert_eq!(
            policy.note_failure_at(at(0)),
            RestartDecision::Restart { delay_ms: 500 }
        );
        assert_eq!(
            policy.note_failure_at(at(100)),
            RestartDecision::Restart { delay_ms: 1_000 }
        );
        assert_eq!(
            policy.note_failure_at(at(200)),
            RestartDecision::Restart { delay_ms: 2_000 }
        );
        assert_eq!(policy.note_failure_at(at(300)), RestartDecision::GiveUp);
        assert!(policy.gave_up());
        assert_eq!(policy.restarts(), 3);
    }

    #[test]
    fn a_healthy_window_resets_the_crash_counter() {
        let mut policy = RestartPolicy::new(2, 1_000);
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        for ms in [0, 100, 1_100, 1_200] {
            assert!(matches!(
                policy.note_failure_at(at(ms)),
                RestartDecision::Restart { .. }
            ));
        }
        assert_eq!(policy.note_failure_at(at(1_300)), RestartDecision::GiveUp);
        assert_eq!(policy.restarts(), 4);
    }

    #[test]
    fn default_policy_matches_session_supervision() {
        let policy = RestartPolicy::default();
        assert_eq!(policy.restarts(), 0);
        assert!(!policy.gave_up());
    }
}
