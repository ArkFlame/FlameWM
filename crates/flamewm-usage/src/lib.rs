//! Pure AI-subscription usage model. No I/O: hosts fetch payloads and hand them here.
//!
//! Reference: `.vendor/CodexBar-main/Sources/CodexBarCore/Providers/Codex/CodexOAuth/CodexOAuthUsageFetcher.swift`
//! (`wham/usage` payload: `rate_limit.primary_window` / `secondary_window`, each with
//! `used_percent`, `reset_at` (unix seconds) and `limit_window_seconds`).

use std::path::Path;

use serde_json::Value;

mod fetch;
pub use fetch::{CLAUDE_BASE, COMMANDCODE_BASE, TIMEOUT_SECS, fetch_snapshot};

/// What the card shows for one provider. `Failed` carries a fixed, secret-free reason.
#[derive(Clone, Debug, PartialEq)]
pub enum ProviderState {
    NotConfigured,
    Ready(Vec<UsageWindow>),
    Failed(&'static str),
}

/// Typed per-provider result handed from the fetch worker to the shell.
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderSnapshot {
    pub provider: UsageProvider,
    pub state: ProviderState,
    /// Unix seconds when the fetch finished.
    pub updated_at: Option<i64>,
}

/// Access token for Claude from `$CLAUDE_CONFIG_DIR/.credentials.json`, else `<home>/.claude/.credentials.json`.
/// Reads only that file; returns `None` when it is missing or unreadable.
#[must_use]
pub fn claude_token(home: &Path, config_dir: Option<&str>) -> Option<String> {
    let dir = config_dir
        .filter(|d| !d.is_empty())
        .map_or_else(|| home.join(".claude"), Into::into);
    parse_claude_credentials(&std::fs::read_to_string(dir.join(".credentials.json")).ok()?)
}

/// Command Code API key from `COMMAND_CODE_API_KEY`, trimmed; `None` when empty or unset.
#[must_use]
pub fn commandcode_token(api_key: Option<&str>) -> Option<String> {
    api_key
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_owned)
}

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

/// Claude OAuth usage (`GET https://api.anthropic.com/api/oauth/usage`, header `anthropic-beta: oauth-2025-04-20`).
/// Reference: `.vendor/CodexBar-main/Sources/CodexBarCore/Providers/Claude/ClaudeOAuth/ClaudeOAuthUsageFetcher.swift`.
/// Lanes: `five_hour` (5 h) and `seven_day` (7 d); `utilization` is percent, `resets_at` is RFC 3339.
pub fn parse_claude_usage(body: &str) -> Result<Vec<UsageWindow>, UsageParseError> {
    let root: Value = serde_json::from_str(body).map_err(|_| UsageParseError::Malformed)?;
    let windows: Vec<UsageWindow> = [("five_hour", 5 * 3600), ("seven_day", 7 * 86_400)]
        .iter()
        .filter_map(|(key, seconds)| {
            let lane = root.get(*key)?;
            Some(UsageWindow {
                used_percent: lane.get("utilization")?.as_f64()?.clamp(0.0, 100.0),
                window_seconds: Some(*seconds),
                resets_at: lane
                    .get("resets_at")
                    .and_then(Value::as_str)
                    .and_then(rfc3339_unix),
            })
        })
        .collect();
    if windows.is_empty() {
        return Err(UsageParseError::NoWindows);
    }
    Ok(windows)
}

/// Access token from the Claude CLI credentials file (`claudeAiOauth.accessToken`).
/// The file lives at `$CLAUDE_CONFIG_DIR/.credentials.json`, or `~/.claude/.credentials.json` when unset.
#[must_use]
pub fn parse_claude_credentials(body: &str) -> Option<String> {
    let root: Value = serde_json::from_str(body).ok()?;
    let token = root.get("claudeAiOauth")?.get("accessToken")?.as_str()?;
    (!token.is_empty()).then(|| token.to_owned())
}

/// Command Code billing (`GET https://api.commandcode.ai/alpha/billing/credits`, `Authorization: Bearer <key>`).
/// Lanes come from `windowLimits.fiveHour` / `windowLimits.weekly` (under `data` or the root), each with
/// `used`, `cap` and `resetAt`. A lane without a positive `cap` is skipped.
/// Reference: `.vendor/CodexBar-main/Sources/CodexBarCore/Providers/CommandCode/CommandCodeUsageFetcher.swift`.
pub fn parse_commandcode_usage(body: &str) -> Result<Vec<UsageWindow>, UsageParseError> {
    let root: Value = serde_json::from_str(body).map_err(|_| UsageParseError::Malformed)?;
    let root = root.get("data").unwrap_or(&root);
    let limits = root
        .get("windowLimits")
        .or_else(|| root.get("credits").and_then(|c| c.get("windowLimits")))
        .ok_or(UsageParseError::NoWindows)?;
    let windows: Vec<UsageWindow> = [("fiveHour", 5 * 3600), ("weekly", 7 * 86_400)]
        .iter()
        .filter_map(|(key, seconds)| {
            let lane = limits.get(*key)?;
            let cap = lane.get("cap")?.as_f64().filter(|c| *c > 0.0)?;
            let used = lane.get("used").and_then(Value::as_f64).unwrap_or(0.0);
            Some(UsageWindow {
                used_percent: (used / cap * 100.0).clamp(0.0, 100.0),
                window_seconds: Some(*seconds),
                resets_at: lane
                    .get("resetAt")
                    .and_then(Value::as_str)
                    .and_then(rfc3339_unix),
            })
        })
        .collect();
    if windows.is_empty() {
        return Err(UsageParseError::NoWindows);
    }
    Ok(windows)
}

/// Unix seconds for an RFC 3339 timestamp such as `2026-11-12T10:00:00.123Z` or `...+02:00`.
#[must_use]
pub fn rfc3339_unix(text: &str) -> Option<i64> {
    let b = text.as_bytes();
    let num = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let mut rest = &text[19..];
    if let Some(frac) = rest.strip_prefix('.') {
        rest = frac.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    let offset = match rest {
        "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let (oh, om) = (
                rest.get(1..3)?.parse::<i64>().ok()?,
                rest.get(4..6)?.parse::<i64>().ok()?,
            );
            sign * (oh * 3600 + om * 60)
        }
    };
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE_FIXTURE: &str = r#"{"five_hour":{"utilization":37.5,"resets_at":"2026-11-12T10:00:00.000000+00:00"},
        "seven_day":{"utilization":104,"resets_at":"2026-11-15T00:00:00Z"},"extra_usage":null}"#;

    #[test]
    fn claude_usage_maps_five_hour_and_seven_day() {
        let w = parse_claude_usage(CLAUDE_FIXTURE).unwrap();
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].used_percent, 37.5);
        assert_eq!(w[0].lane_name(), "session");
        assert_eq!(w[0].resets_at, rfc3339_unix("2026-11-12T10:00:00Z"));
        assert_eq!(w[1].used_percent, 100.0);
        assert_eq!(w[1].lane_name(), "weekly");
        assert_eq!(
            parse_claude_usage(r#"{"extra_usage":null}"#),
            Err(UsageParseError::NoWindows)
        );
    }

    #[test]
    fn claude_token_reads_home_or_config_dir_only() {
        let root = std::env::temp_dir().join(format!("flamewm-usage-test-{}", std::process::id()));
        let home = root.join("home");
        let config = root.join("config");
        for dir in [home.join(".claude"), config.clone()] {
            std::fs::create_dir_all(&dir).unwrap();
        }
        std::fs::write(
            home.join(".claude/.credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"home-token"}}"#,
        )
        .unwrap();
        std::fs::write(
            config.join(".credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"config-token"}}"#,
        )
        .unwrap();
        assert_eq!(claude_token(&home, None).as_deref(), Some("home-token"));
        assert_eq!(claude_token(&home, Some("")).as_deref(), Some("home-token"));
        assert_eq!(
            claude_token(&home, config.to_str()).as_deref(),
            Some("config-token")
        );
        assert_eq!(claude_token(&root.join("empty"), None), None);
        assert_eq!(commandcode_token(Some("  ")), None);
        assert_eq!(commandcode_token(Some(" key ")).as_deref(), Some("key"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn claude_credentials_yield_only_a_nonempty_token() {
        let body = r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat-fixture","expiresAt":1}}"#;
        assert_eq!(
            parse_claude_credentials(body).as_deref(),
            Some("sk-ant-oat-fixture")
        );
        assert_eq!(
            parse_claude_credentials(r#"{"claudeAiOauth":{"accessToken":""}}"#),
            None
        );
        assert_eq!(parse_claude_credentials("{bad"), None);
    }

    #[test]
    fn commandcode_usage_uses_used_over_cap() {
        let body = r#"{"success":true,"data":{"windowLimits":{
            "fiveHour":{"used":30,"cap":120,"resetAt":"2026-11-12T10:00:00Z"},
            "weekly":{"used":5,"cap":0,"resetAt":"2026-11-15T00:00:00Z"}}}}"#;
        let w = parse_commandcode_usage(body).unwrap();
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].used_percent, 25.0);
        assert_eq!(w[0].lane_name(), "session");
        assert_eq!(
            parse_commandcode_usage(r#"{"credits":{}}"#),
            Err(UsageParseError::NoWindows)
        );
    }

    #[test]
    fn rfc3339_handles_offsets_and_fractions() {
        assert_eq!(rfc3339_unix("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(rfc3339_unix("1970-01-01T01:00:00+01:00"), Some(0));
        assert_eq!(rfc3339_unix("2000-03-01T00:00:00.5Z"), Some(951_868_800));
        assert_eq!(rfc3339_unix("2026-13"), None);
    }

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
