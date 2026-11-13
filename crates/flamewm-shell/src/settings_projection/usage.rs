//! AI usage settings: which providers the usage card fetches and how often.

use std::time::Duration;

use flamewm_api::settings::{boolean_value, integer_value, SettingsSnapshot};
use flamewm_usage::UsageProvider;

pub const USAGE_REFRESH_MIN_SECONDS: i64 = 60;
pub const USAGE_REFRESH_MAX_SECONDS: i64 = 3600;
pub const USAGE_REFRESH_DEFAULT_SECONDS: i64 = 300;

/// Providers to fetch and the refresh period, clamped to the supported range.
#[must_use]
pub fn usage_settings_from(
    claude: bool,
    command_code: bool,
    refresh_seconds: i64,
) -> (Vec<UsageProvider>, Duration) {
    let mut providers = Vec::new();
    if claude {
        providers.push(UsageProvider::Claude);
    }
    if command_code {
        providers.push(UsageProvider::CommandCode);
    }
    let seconds = refresh_seconds.clamp(USAGE_REFRESH_MIN_SECONDS, USAGE_REFRESH_MAX_SECONDS);
    let seconds = u64::try_from(seconds).unwrap_or(300);
    (providers, Duration::from_secs(seconds))
}

/// Reads the `usage.*` keys; a missing snapshot or key keeps the defaults.
#[must_use]
pub fn usage_settings(snapshot: Option<&SettingsSnapshot>) -> (Vec<UsageProvider>, Duration) {
    let Some(snapshot) = snapshot else {
        return usage_settings_from(true, true, USAGE_REFRESH_DEFAULT_SECONDS);
    };
    usage_settings_from(
        boolean_value(snapshot, "usage.claude").unwrap_or(true),
        boolean_value(snapshot, "usage.commandCode").unwrap_or(true),
        integer_value(snapshot, "usage.refresh_seconds").unwrap_or(USAGE_REFRESH_DEFAULT_SECONDS),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // OBSERVABLE: the disabled provider is dropped and the interval clamps to 60 s..1 h.
    // MUTATION: keeping a disabled provider, or no clamp, fails one of these asserts.
    #[test]
    fn disabled_providers_drop_and_interval_clamps() {
        let (providers, interval) = usage_settings_from(false, true, 10);
        assert_eq!(providers, vec![UsageProvider::CommandCode]);
        assert_eq!(interval, Duration::from_secs(60));
        let (providers, interval) = usage_settings_from(true, true, 99_999);
        assert_eq!(
            providers,
            vec![UsageProvider::Claude, UsageProvider::CommandCode]
        );
        assert_eq!(interval, Duration::from_secs(3600));
    }

    #[test]
    fn missing_snapshot_keeps_defaults() {
        let (providers, interval) = usage_settings(None);
        assert_eq!(providers.len(), 2);
        assert_eq!(interval, Duration::from_secs(300));
    }
}
