//! One typed failure record shared by both sessions and their tooling.
//!
//! This module is the canonical owner of the failure-record schema and of its single text line
//! format. It deliberately contains no filesystem write: step 1 of the remaster (see
//! `docs/swarm/wave2/w41-remaster-failure-records-design.md`) is the pure type and formatter, so it
//! compiles and tests without changing a caller. The durable append helper moves here in a later
//! step.
//!
//! Field names, `FailureCode` strings and the [`FailureRecord::to_log_line`] layout are copied from
//! `flamewm_session_core::diagnostics::failure` so the later re-export migration keeps every
//! existing log line byte-identical.

use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

/// Stable machine-readable failure codes. The string form is part of the log contract: adding a
/// variant must not change the text of an existing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureCode {
    /// A panic was observed by an installed hook or an unwind boundary.
    Panic,
    /// A restart was refused because a per-window limit was reached.
    RestartLimit,
    /// A subsystem returned a recoverable error.
    SubsystemError,
}

impl FailureCode {
    /// Stable log text for this code.
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
    /// Build a blast radius from stable subsystem ids.
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
/// clock can pass an injected value and keep the formatter pure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureRecord {
    /// Startup or subsystem stage that failed, as a stable id.
    pub stage: String,
    /// Machine-readable failure code.
    pub code: FailureCode,
    /// Operator-facing cause text.
    pub cause: String,
    /// Subsystems that survived and that degraded.
    pub blast_radius: BlastRadius,
    /// Whether a fallback was taken after the failure.
    pub fallback_taken: bool,
    /// Unix seconds at the time of the failure.
    pub timestamp: u64,
}

impl FailureRecord {
    /// Build a record with an explicit timestamp.
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

/// Format a startup-stage failure into the canonical typed record. Pure: `timestamp` is injected so
/// the caller owns the clock and this function never touches the filesystem. The blast radius takes
/// subsystem ids for what still works and what is degraded, matching records produced elsewhere so
/// failures from different stages can be compared field by field.
#[must_use]
pub fn format_startup_failure(
    stage: impl Into<String>,
    code: FailureCode,
    cause: impl Into<String>,
    still_works: &[&str],
    degraded: &[&str],
    fallback_taken: bool,
    timestamp: u64,
) -> FailureRecord {
    FailureRecord::new(
        stage,
        code,
        cause,
        BlastRadius::new(still_works.iter().copied(), degraded.iter().copied()),
        fallback_taken,
        timestamp,
    )
}

/// Unix seconds for callers of [`FailureRecord::at_now`]. The only clock reader in this module;
/// [`format_startup_failure`] stays pure with an injected timestamp.
#[must_use]
pub fn unix_seconds_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_strings_are_stable() {
        assert_eq!(FailureCode::Panic.as_str(), "panic");
        assert_eq!(FailureCode::RestartLimit.as_str(), "restart_limit");
        assert_eq!(FailureCode::SubsystemError.as_str(), "subsystem_error");
    }

    #[test]
    fn format_startup_failure_matches_the_existing_line_byte_for_byte() {
        let record = format_startup_failure(
            "preflight",
            FailureCode::SubsystemError,
            "preflight failed: no DISPLAY",
            &[],
            &["session"],
            false,
            21,
        );
        assert_eq!(record.stage, "preflight");
        assert_eq!(record.code, FailureCode::SubsystemError);
        assert_eq!(record.cause, "preflight failed: no DISPLAY");
        assert!(record.blast_radius.still_works.is_empty());
        assert_eq!(record.blast_radius.degraded, ["session"]);
        assert!(!record.fallback_taken);
        assert_eq!(record.timestamp, 21);
        assert_eq!(
            record.to_log_line(),
            "timestamp=21 stage=\"preflight\" code=subsystem_error cause=\"preflight failed: no DISPLAY\" still_works=\"\" degraded=\"session\" fallback_taken=false"
        );
    }

    #[test]
    fn line_escapes_controls_and_list_delimiters() {
        let record = FailureRecord::new(
            "wm",
            FailureCode::RestartLimit,
            "line one\nline \"two\"\ttab",
            BlastRadius::new(["x11", "shell,extra"], ["comp\\ositor"]),
            true,
            42,
        );
        let line = record.to_log_line();
        assert_eq!(
            line,
            "timestamp=42 stage=\"wm\" code=restart_limit cause=\"line one\\nline \\\"two\\\"\\ttab\" still_works=\"x11,shell\\,extra\" degraded=\"comp\\\\ositor\" fallback_taken=true"
        );
        assert!(!line.contains('\n'));
        assert!(!line.contains('\r'));
        assert!(!line.contains('\t'));
    }

    #[test]
    fn at_now_fills_the_same_fields_with_a_real_timestamp() {
        let record = FailureRecord::at_now(
            "startup",
            FailureCode::SubsystemError,
            "no DISPLAY",
            BlastRadius::new([] as [&str; 0], ["wm"]),
            false,
        );
        assert_eq!(record.stage, "startup");
        assert_eq!(record.code, FailureCode::SubsystemError);
        assert_eq!(record.cause, "no DISPLAY");
        assert_eq!(record.blast_radius.degraded, ["wm"]);
        assert!(!record.fallback_taken);
        assert!(record.timestamp > 0);
        assert!(record.to_log_line().starts_with("timestamp="));
    }
}
