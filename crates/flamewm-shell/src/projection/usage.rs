//! AI usage card projection: one row per provider with name, percent bar and reset text.

use chrono::{Local, TimeZone};
use flamewm_ui_x11::UiDocumentAccess;
use flamewm_usage::{reset_countdown, ProviderSnapshot, ProviderState, UsageProvider};

/// Usable track width in px: 300 px card minus 16 px side padding on each side.
const USAGE_TRACK_WIDTH: f32 = 268.0;
const USAGE_FILL_HEIGHT: f32 = 6.0;

/// Element-id slug for a provider row (`usage-<slug>-*`), matching `ui/shell/usage.html`.
fn slug(provider: UsageProvider) -> &'static str {
    match provider {
        UsageProvider::Codex => "codex",
        UsageProvider::Claude => "claude",
        UsageProvider::OpenCodeGo => "opencode-go",
        UsageProvider::CommandCode => "command-code",
        UsageProvider::OpenDesign => "opendesign",
    }
}

/// Percent (clamped, `None` when unavailable) and reset caption for one provider state.
#[must_use]
pub fn row_values(state: Option<&ProviderState>, now: i64) -> (Option<f64>, String) {
    match state {
        Some(ProviderState::Ready(windows)) => match windows.first() {
            Some(window) => {
                let percent = window.used_percent.clamp(0.0, 100.0);
                let lane = window.lane_name();
                let caption = match reset_countdown(now, window.resets_at) {
                    Some(text) => format!("{lane}: {text}"),
                    None => lane.to_owned(),
                };
                (Some(percent), caption)
            }
            None => (None, "No usage reported".to_owned()),
        },
        Some(ProviderState::Failed(reason)) => (None, (*reason).to_owned()),
        Some(ProviderState::NotConfigured) | None => (None, "Not configured".to_owned()),
    }
}

/// Projects every provider row and the footer timestamp. Unknown or missing providers stay
/// "not configured", so the card always lists all five providers.
pub fn project_usage(
    document: &mut impl UiDocumentAccess,
    snapshots: &[ProviderSnapshot],
    now: i64,
) -> Result<(), String> {
    for provider in UsageProvider::ALL {
        let slug = slug(provider);
        let state = snapshots
            .iter()
            .find(|snapshot| snapshot.provider == provider)
            .map(|snapshot| &snapshot.state);
        let (percent, caption) = row_values(state, now);
        let value = percent.unwrap_or(0.0);
        document.text(
            &format!("usage-{slug}-percent"),
            percent.map_or_else(|| "--".to_owned(), |p| format!("{p:.0}%")),
        )?;
        document.size(
            &format!("usage-{slug}-fill"),
            USAGE_TRACK_WIDTH * (value as f32) / 100.0,
            USAGE_FILL_HEIGHT,
        )?;
        document.attribute(
            &format!("usage-{slug}-track"),
            "aria-valuenow",
            &format!("{value:.0}"),
        )?;
        document.text(&format!("usage-{slug}-reset"), caption)?;
    }
    let updated = snapshots.iter().filter_map(|s| s.updated_at).max();
    let line = match updated.and_then(|t| Local.timestamp_opt(t, 0).single()) {
        Some(time) => format!("Updated {}", time.format("%H:%M")),
        None => "Not updated yet".to_owned(),
    };
    document.text("usage-updated", line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_usage::UsageWindow;

    #[test]
    fn row_values_clamp_and_describe_each_state() {
        let ready = ProviderState::Ready(vec![UsageWindow {
            used_percent: 140.0,
            window_seconds: Some(5 * 3600),
            resets_at: Some(1_000 + 90),
        }]);
        let (percent, caption) = row_values(Some(&ready), 1_000);
        assert_eq!(percent, Some(100.0));
        assert!(caption.starts_with("session"), "{caption}");
        assert_eq!(
            row_values(Some(&ProviderState::Failed("HTTP 500")), 0),
            (None, "HTTP 500".to_owned())
        );
        assert_eq!(row_values(None, 0), (None, "Not configured".to_owned()));
    }
}
