//! Pure AI-subscription usage model. No I/O: hosts fetch payloads and hand them here.
//!
//! Reference: `.vendor/CodexBar-main/Sources/CodexBarCore/Providers/Codex/CodexOAuth/CodexOAuthUsageFetcher.swift`
//! (`wham/usage` payload: `rate_limit.primary_window` / `secondary_window`, each with
//! `used_percent`, `reset_at` (unix seconds) and `limit_window_seconds`).

use serde_json::Value;

/// Providers the usage card can show. Only `Codex` has a parser so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageProvider {
    Codex,
    Claude,
    OpenCodeGo,
    CommandCode,
    OpenDesign,
}

impl UsageProvider {
    pub const ALL: [Self; 5] = [
        Self::Codex,
        Self::Claude,
        Self::OpenCodeGo,
        Self::CommandCode,
        Self::OpenDesign,
    ];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "ChatGPT / Codex",
            Self::Claude => "Claude",
            Self::OpenCodeGo => "OpenCode Go",
            Self::CommandCode => "Command Code",
            Self::OpenDesign => "OpenDesign",
        }
    }
}

/// One rate-limit lane, for example the 5-hour session or the weekly allowance.
#[derive(Clone, Debug, PartialEq)]
pub struct UsageWindow {
    /// Percent used, clamped to 0..=100 for display.
    pub used_percent: f64,
    /// Length of the window in seconds, when the provider reports it.
    pub window_seconds: Option<i64>,
    /// Reset instant in unix seconds, when the provider reports it.
    pub resets_at: Option<i64>,
}

impl UsageWindow {
    /// Human name for the lane, derived from its length: `session` (<= 6 h), `weekly` (>= 6 days), else `window`.
    #[must_use]
    pub fn lane_name(&self) -> &'static str {
        match self.window_seconds {
            Some(s) if s <= 6 * 3600 => "session",
            Some(s) if s >= 6 * 86_400 => "weekly",
            _ => "window",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum UsageParseError {
    /// The payload is not JSON.
    Malformed,
    /// The payload is JSON but has no usable rate-limit lane.
    NoWindows,
}

/// Parses a Codex `wham/usage` payload into its primary and secondary lanes.
/// Lanes missing `used_percent` are skipped; a payload with no lanes is `NoWindows`.
pub fn parse_codex_usage(body: &str) -> Result<Vec<UsageWindow>, UsageParseError> {
    let root: Value = serde_json::from_str(body).map_err(|_| UsageParseError::Malformed)?;
    let limits = root.get("rate_limit").ok_or(UsageParseError::NoWindows)?;
    let windows: Vec<UsageWindow> = ["primary_window", "secondary_window"]
        .iter()
        .filter_map(|key| limits.get(*key))
        .filter_map(codex_window)
        .collect();
    if windows.is_empty() {
        return Err(UsageParseError::NoWindows);
    }
    Ok(windows)
}

fn codex_window(lane: &Value) -> Option<UsageWindow> {
    let used = lane.get("used_percent")?.as_f64()?;
    Some(UsageWindow {
        used_percent: used.clamp(0.0, 100.0),
        window_seconds: lane.get("limit_window_seconds").and_then(Value::as_i64),
        resets_at: lane.get("reset_at").and_then(Value::as_i64),
    })
}

/// Reset text for the card: `resets in 2h 5m`, `resets in 3d 4h`, `resets now`, or `None` when unknown.
#[must_use]
pub fn reset_countdown(now: i64, resets_at: Option<i64>) -> Option<String> {
    let remaining = resets_at? - now;
    if remaining <= 0 {
        return Some("resets now".to_owned());
    }
    let minutes = (remaining + 59) / 60;
    let (days, hours, mins) = (minutes / 1440, minutes / 60 % 24, minutes % 60);
    Some(match (days, hours) {
        (0, 0) => format!("resets in {mins}m"),
        (0, _) => format!("resets in {hours}h {mins}m"),
        _ => format!("resets in {days}d {hours}h"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{"plan_type":"plus","rate_limit":{
        "primary_window":{"used_percent":42,"reset_at":1000000,"limit_window_seconds":18000},
        "secondary_window":{"used_percent":130,"reset_at":1500000,"limit_window_seconds":604800}}}"#;

    #[test]
    fn parses_both_lanes_and_clamps_percent() {
        let w = parse_codex_usage(FIXTURE).unwrap();
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].used_percent, 42.0);
        assert_eq!(w[0].resets_at, Some(1_000_000));
        assert_eq!(w[0].lane_name(), "session");
        assert_eq!(w[1].used_percent, 100.0);
        assert_eq!(w[1].lane_name(), "weekly");
    }

    #[test]
    fn rejects_garbage_and_empty_limits() {
        assert_eq!(
            parse_codex_usage("not json"),
            Err(UsageParseError::Malformed)
        );
        assert_eq!(
            parse_codex_usage(r#"{"rate_limit":{}}"#),
            Err(UsageParseError::NoWindows)
        );
        assert_eq!(
            parse_codex_usage(r#"{"other":1}"#),
            Err(UsageParseError::NoWindows)
        );
        assert_eq!(
            parse_codex_usage(r#"{"rate_limit":{"primary_window":{"reset_at":5}}}"#),
            Err(UsageParseError::NoWindows)
        );
    }

    #[test]
    fn countdown_formats_remaining_time() {
        assert_eq!(reset_countdown(0, None), None);
        assert_eq!(
            reset_countdown(100, Some(100)).as_deref(),
            Some("resets now")
        );
        assert_eq!(
            reset_countdown(0, Some(59 * 60 + 1)).as_deref(),
            Some("resets in 1h 0m")
        );
        assert_eq!(
            reset_countdown(0, Some(2 * 3600 + 5 * 60)).as_deref(),
            Some("resets in 2h 5m")
        );
        assert_eq!(
            reset_countdown(0, Some(3 * 86_400 + 4 * 3600)).as_deref(),
            Some("resets in 3d 4h")
        );
    }

    #[test]
    fn provider_labels_are_distinct() {
        let mut labels: Vec<_> = UsageProvider::ALL.iter().map(|p| p.label()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), UsageProvider::ALL.len());
    }
}
