//! AI usage refresh scheduler for the tray card. Mirrors [`crate::weather_host::WeatherHost`]:
//! network work runs on a worker thread, the owner only polls the channel in `tick`.
//! Credentials are read on the worker from the provider's documented file or env var and are
//! never stored on this type or rendered.

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use flamewm_usage::{
    claude_token, codex_credentials, commandcode_token, fetch_snapshot, ProviderSnapshot,
    ProviderState, UsageProvider, CLAUDE_BASE, CODEX_BASE, COMMANDCODE_BASE,
};

use crate::weather_host::spawn_worker;

/// How often enabled providers are refetched while the shell runs.
pub const USAGE_REFRESH_INTERVAL: Duration = Duration::from_secs(300);

/// Cap for retrying a refresh whose snapshots were all failed or unconfigured. CodexBar's
/// `.vendor/CodexBar-main/Sources/CodexBarCore/Providers/Claude/ClaudeOAuth/ClaudeOAuthRefreshFailureGate.swift`
/// uses the same pattern: a transient base delay that doubles up to a cap.
pub const USAGE_BACKOFF_CAP: Duration = Duration::from_secs(30 * 60);

/// Floor for the retry delay, so even a misconfigured zero interval cannot busy-poll.
const USAGE_RETRY_MIN: Duration = Duration::from_secs(60);

/// Providers fetched by default. `OpenCodeGo` (cookie-only) and `OpenDesign` (no usage source)
/// are not in this list; they stay "not configured".
pub const DEFAULT_PROVIDERS: [UsageProvider; 3] = [
    UsageProvider::Codex,
    UsageProvider::Claude,
    UsageProvider::CommandCode,
];

type UsageResult = Vec<ProviderSnapshot>;

#[derive(Debug)]
pub struct UsageHost {
    pub enabled: Vec<UsageProvider>,
    interval: Duration,
    pub snapshots: UsageResult,
    last_started: Option<Instant>,
    /// Earliest instant another fetch may start after a failed/unconfigured result.
    retry_at: Option<Instant>,
    /// Consecutive unsuccessful refreshes; drives the bounded doubling delay.
    failures: u8,
    inflight: Option<mpsc::Receiver<UsageResult>>,
    /// False while the usage popup is closed: no fetch starts, no credentials are read, and no
    /// wake is scheduled. Results already in flight are still polled.
    needed: bool,
}

impl Default for UsageHost {
    fn default() -> Self {
        Self {
            enabled: DEFAULT_PROVIDERS.to_vec(),
            interval: USAGE_REFRESH_INTERVAL,
            snapshots: Vec::new(),
            last_started: None,
            retry_at: None,
            failures: 0,
            inflight: None,
            needed: true,
        }
    }
}

impl UsageHost {
    /// Marks whether the usage popup currently needs data. Fetches start only while needed.
    pub fn set_needed(&mut self, needed: bool) {
        self.needed = needed;
    }

    /// True when at least one provider is enabled, the popup needs data, no fetch is running,
    /// and both the normal interval and any failure backoff have elapsed.
    #[must_use]
    pub fn due(&self, now: Instant) -> bool {
        self.needed
            && !self.enabled.is_empty()
            && self.inflight.is_none()
            && self.retry_at.is_none_or(|at| now >= at)
            && self
                .last_started
                .is_none_or(|t| now.duration_since(t) >= self.interval)
    }

    /// Applies the saved provider list and refresh interval; takes effect on the next due check.
    /// A changed selection clears the backoff so a newly added credential is picked up promptly.
    pub fn configure(&mut self, enabled: Vec<UsageProvider>, interval: Duration) {
        if self.enabled != enabled || self.interval != interval {
            self.enabled = enabled;
            self.interval = interval;
            self.last_started = None;
            self.retry_at = None;
            self.failures = 0;
        }
    }

    /// Instant the next refresh becomes due, for the reactor wake. `None` when no provider is
    /// enabled (idle wakeups stay at zero) or while a fetch runs: its worker writes the wake.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        if !self.needed || self.enabled.is_empty() || self.inflight.is_some() {
            return None;
        }
        let interval_at = self.last_started.map(|started| started + self.interval);
        match (interval_at, self.retry_at) {
            (Some(interval_at), Some(retry_at)) => Some(interval_at.max(retry_at)),
            (interval_at, retry_at) => interval_at.or(retry_at),
        }
    }

    /// Polls a finished fetch, then starts a new one when due. Returns `true` when snapshots changed.
    pub fn tick(
        &mut self,
        now: Instant,
        fetch: impl FnOnce(Vec<UsageProvider>) -> mpsc::Receiver<UsageResult>,
    ) -> bool {
        let mut changed = false;
        if let Some(rx) = self.inflight.as_ref() {
            match rx.try_recv() {
                Ok(snapshots) => {
                    let usable = snapshots
                        .iter()
                        .any(|snapshot| matches!(snapshot.state, ProviderState::Ready(_)));
                    self.record_outcome(now, usable);
                    self.snapshots = snapshots;
                    self.inflight = None;
                    changed = true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.record_outcome(now, false);
                    self.inflight = None;
                }
            }
        }
        if self.due(now) {
            self.last_started = Some(now);
            self.inflight = Some(fetch(self.enabled.clone()));
        }
        changed
    }

    /// Advances the retry schedule: no usable snapshot means every enabled provider failed or
    /// lacks credentials, so the next attempt waits longer, bounded by [`USAGE_BACKOFF_CAP`].
    fn record_outcome(&mut self, now: Instant, usable: bool) {
        if usable {
            self.failures = 0;
            self.retry_at = None;
            return;
        }
        self.failures = self.failures.saturating_add(1);
        let shift = u32::from(self.failures.min(5));
        let delay = self
            .interval
            .saturating_mul(1_u32 << shift)
            .clamp(USAGE_RETRY_MIN, USAGE_BACKOFF_CAP);
        self.retry_at = Some(now + delay);
    }
}

/// Production fetch on a worker thread: one request per enabled provider, bounded by the
/// fetch timeout. Tokens come from `$CODEX_HOME`, `$CLAUDE_CONFIG_DIR` or `$HOME`, and
/// `COMMAND_CODE_API_KEY`.
#[must_use]
pub fn spawn_usage_fetch(
    providers: Vec<UsageProvider>,
    waker: Option<Arc<UnixStream>>,
) -> mpsc::Receiver<UsageResult> {
    spawn_worker("flamewm-usage", waker, move || {
        // Proof seam: a fixture file replaces the network and credential reads entirely.
        if let Some(path) = std::env::var_os("FLAMEWM_USAGE_FIXTURE") {
            let now = chrono::Utc::now().timestamp();
            return std::fs::read_to_string(path)
                .ok()
                .and_then(|body| flamewm_usage::parse_fixture(&body, now).ok())
                .unwrap_or_default();
        }
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let config_dir = std::env::var("CLAUDE_CONFIG_DIR").ok();
        let codex_home = std::env::var("CODEX_HOME").ok();
        let now = chrono::Utc::now().timestamp();
        providers
            .into_iter()
            .map(|provider| {
                let (token, account, base) = match provider {
                    UsageProvider::Codex => {
                        let (token, account) = codex_credentials(&home, codex_home.as_deref())
                            .map_or((None, None), |(token, account)| (Some(token), account));
                        (token, account, CODEX_BASE)
                    }
                    UsageProvider::Claude => (
                        claude_token(&home, config_dir.as_deref()),
                        None,
                        CLAUDE_BASE,
                    ),
                    UsageProvider::CommandCode => (
                        commandcode_token(std::env::var("COMMAND_CODE_API_KEY").ok().as_deref()),
                        None,
                        COMMANDCODE_BASE,
                    ),
                    _ => (None, None, ""),
                };
                fetch_snapshot(provider, token.as_deref(), account.as_deref(), base, now)
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_usage::ProviderState;

    fn snapshot(provider: UsageProvider) -> ProviderSnapshot {
        ProviderSnapshot {
            provider,
            state: ProviderState::NotConfigured,
            updated_at: Some(1),
        }
    }

    #[test]
    fn tick_starts_one_fetch_then_waits_for_the_interval() {
        let mut host = UsageHost::default();
        let start = Instant::now();
        let (tx, rx) = mpsc::channel();
        let mut calls = 0;
        assert!(!host.tick(start, |providers| {
            calls += 1;
            assert_eq!(providers, DEFAULT_PROVIDERS.to_vec());
            rx
        }));
        assert!(!host.due(start), "a fetch is in flight");
        tx.send(vec![ProviderSnapshot {
            provider: UsageProvider::Claude,
            state: ProviderState::Ready(vec![]),
            updated_at: Some(1),
        }])
        .unwrap();
        assert!(host.tick(start, |_| panic!(
            "no fetch while in flight or inside the interval"
        )));
        assert_eq!(host.snapshots.len(), 1);
        assert!(!host.due(
            start
                + USAGE_REFRESH_INTERVAL
                    .checked_sub(Duration::from_secs(1))
                    .unwrap()
        ));
        assert!(host.due(start + USAGE_REFRESH_INTERVAL));
        assert_eq!(calls, 1);
    }

    #[test]
    fn disconnected_worker_clears_inflight_without_snapshots() {
        let mut host = UsageHost::default();
        let start = Instant::now();
        let (tx, rx) = mpsc::channel::<UsageResult>();
        drop(tx);
        host.tick(start, |_| rx);
        assert!(!host.tick(start, |_| mpsc::channel().1));
        assert_eq!(host.snapshots.len(), 0);
    }

    #[test]
    fn disabled_usage_never_fetches_or_arms_a_wake() {
        let mut host = UsageHost::default();
        let start = Instant::now();
        host.configure(Vec::new(), Duration::from_secs(1));
        assert!(!host.due(start));
        assert_eq!(host.next_deadline(), None);
        assert!(!host.tick(start, |_| panic!("disabled usage must not spawn a worker")));
    }

    #[test]
    fn unconfigured_result_backs_off_then_a_usable_snapshot_restores_cadence() {
        let mut host = UsageHost::default();
        let start = Instant::now();
        let (tx, rx) = mpsc::channel();
        host.tick(start, |_| rx);
        tx.send(vec![snapshot(UsageProvider::Claude)]).unwrap();
        host.tick(start, |_| panic!("inside interval"));
        assert_eq!(
            host.next_deadline(),
            Some(start + 2 * USAGE_REFRESH_INTERVAL)
        );
        assert!(!host.due(start + USAGE_REFRESH_INTERVAL));
        let retry_at = start + 2 * USAGE_REFRESH_INTERVAL;
        let (tx2, rx2) = mpsc::channel();
        assert!(!host.tick(retry_at, |_| rx2));
        tx2.send(vec![ProviderSnapshot {
            provider: UsageProvider::Claude,
            state: ProviderState::Ready(vec![]),
            updated_at: Some(2),
        }])
        .unwrap();
        assert!(host.tick(retry_at, |_| panic!("in flight")));
        assert_eq!(
            host.next_deadline(),
            Some(retry_at + USAGE_REFRESH_INTERVAL)
        );
    }

    #[test]
    fn zero_interval_after_disconnect_cannot_busy_poll() {
        let mut host = UsageHost::default();
        let start = Instant::now();
        host.configure(vec![UsageProvider::Codex], Duration::ZERO);
        let (tx, rx) = mpsc::channel::<UsageResult>();
        drop(tx);
        host.tick(start, |_| rx);
        assert!(!host.due(start));
        assert!(!host.tick(start, |_| panic!("no immediate retry")));
        assert_eq!(host.next_deadline(), Some(start + USAGE_RETRY_MIN));
    }
}
