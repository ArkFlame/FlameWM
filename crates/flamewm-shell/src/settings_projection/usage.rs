//! AI usage settings: which providers the usage card fetches and how often.

use std::time::Duration;

use flamewm_api::settings::{boolean_value, integer_value, SettingsSnapshot};
use flamewm_usage::UsageProvider;

pub const USAGE_REFRESH_MIN_SECONDS: i64 = 60;
pub const USAGE_REFRESH_MAX_SECONDS: i64 = 3600;
pub const USAGE_REFRESH_DEFAULT_SECONDS: i64 = 300;

/// Settings key per provider, in `UsageProvider::ALL` order.
const PROVIDER_KEYS: [(UsageProvider, &str); 5] = [
    (UsageProvider::Codex, "usage.codex"),
    (UsageProvider::Claude, "usage.claude"),
    (UsageProvider::OpenCodeGo, "usage.openCodeGo"),
    (UsageProvider::CommandCode, "usage.commandCode"),
    (UsageProvider::OpenDesign, "usage.openDesign"),
];

/// Enabled providers and the refresh period, clamped to the supported range. A provider that is
/// enabled but not configured is fetched as `NotConfigured` and hidden by the card projection.
#[must_use]
pub fn usage_settings_from(
    enabled: impl Fn(UsageProvider) -> bool,
    refresh_seconds: i64,
) -> (Vec<UsageProvider>, Duration) {
    let providers = PROVIDER_KEYS
        .iter()
        .filter(|(provider, _)| enabled(*provider))
        .map(|(provider, _)| *provider)
        .collect();
    let seconds = refresh_seconds.clamp(USAGE_REFRESH_MIN_SECONDS, USAGE_REFRESH_MAX_SECONDS);
    let seconds = u64::try_from(seconds).unwrap_or(300);
    (providers, Duration::from_secs(seconds))
}

/// Reads the `usage.*` keys; a missing snapshot or key keeps the defaults (all enabled).
#[must_use]
pub fn usage_settings(snapshot: Option<&SettingsSnapshot>) -> (Vec<UsageProvider>, Duration) {
    let Some(snapshot) = snapshot else {
        return usage_settings_from(|_| true, USAGE_REFRESH_DEFAULT_SECONDS);
    };
    usage_settings_from(
        |provider| {
            PROVIDER_KEYS
                .iter()
                .find(|(p, _)| *p == provider)
                .and_then(|(_, key)| boolean_value(snapshot, key))
                .unwrap_or(true)
        },
        integer_value(snapshot, "usage.refreshSeconds").unwrap_or(USAGE_REFRESH_DEFAULT_SECONDS),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // OBSERVABLE: the disabled provider is dropped and the interval clamps to 60 s..1 h.
    #[test]
    fn disabled_providers_drop_and_interval_clamps() {
        let (providers, interval) =
            usage_settings_from(|p| p != UsageProvider::Claude, 10);
        assert!(!providers.contains(&UsageProvider::Claude));
        assert_eq!(providers.len(), 4);
        assert_eq!(interval, Duration::from_secs(60));
        let (providers, interval) = usage_settings_from(|_| true, 99_999);
        assert_eq!(providers.len(), 5);
        assert_eq!(interval, Duration::from_secs(3600));
    }

    #[test]
    fn missing_snapshot_keeps_defaults() {
        let (providers, interval) = usage_settings(None);
        assert_eq!(providers.len(), 5);
        assert_eq!(interval, Duration::from_secs(300));
    }
}
