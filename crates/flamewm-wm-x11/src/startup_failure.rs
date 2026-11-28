//! Stage-classified fatal startup failure for the X11 WM. Each fatal startup step tags its
//! error with its stage through `AnyError`; the binary prints one classified line before exit(1).
use std::error::Error;
use std::fmt;

use crate::atoms::AnyError;

/// A fatal startup error tagged with the stage that raised it. `Display` is the cause only,
/// so the operator-facing `flamewm: {error}` text is unchanged.
#[derive(Debug)]
pub struct X11StartupFailure {
    pub stage: &'static str,
    pub cause: String,
}

impl fmt::Display for X11StartupFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.cause)
    }
}

impl Error for X11StartupFailure {}

/// Tag a fatal startup error with its stage.
pub fn startup_stage<E: fmt::Display>(stage: &'static str, error: E) -> AnyError {
    Box::new(X11StartupFailure {
        stage,
        cause: error.to_string(),
    })
}

/// Pure: the X connect failure cause. Connect failures are almost always a missing or
/// unreachable DISPLAY, so the operator is told that first.
pub fn connect_cause(error: &str) -> String {
    format!("DISPLAY unset or unreachable: {error}")
}

/// Pure: one classified startup-failure line. Unknown stages are reported as `unclassified`.
pub fn startup_failure_line(stage: &str, cause: &str) -> String {
    let (name, blast) = match stage {
        "connect" => ("connect", "WM cannot start; no X11 session is managed"),
        "integrations" => (
            "integrations",
            "session integrations (network, media, power, bluetooth) unavailable; WM cannot start",
        ),
        "catalog" => (
            "catalog",
            "application catalog unavailable; WM cannot start",
        ),
        "become-wm" => ("become-wm", "root window not claimed; WM cannot operate"),
        "scan" => (
            "scan",
            "existing windows could not be adopted; WM cannot start",
        ),
        _ => ("unclassified", "startup stopped before the WM was running"),
    };
    format!("FLAMEWM-X11-STARTUP-FAIL stage={name} cause={cause} blast_radius={blast}")
}

/// Classify a fatal startup error for the binary. Errors without a stage tag are `unclassified`.
pub fn startup_failure_report(error: &(dyn Error + 'static)) -> String {
    match error.downcast_ref::<X11StartupFailure>() {
        Some(failure) => startup_failure_line(failure.stage, &failure.cause),
        None => startup_failure_line("unclassified", &error.to_string()),
    }
}
