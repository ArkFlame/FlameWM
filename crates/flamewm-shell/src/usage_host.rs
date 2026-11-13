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
    claude_token, commandcode_token, fetch_snapshot, ProviderSnapshot, UsageProvider, CLAUDE_BASE,
    COMMANDCODE_BASE,
};

use crate::weather_host::spawn_worker;

/// How often enabled providers are refetched while the shell runs.
pub const USAGE_REFRESH_INTERVAL: Duration = Duration::from_secs(300);

/// Providers fetched by default. `OpenCodeGo` (cookie-only) and `OpenDesign` (no usage source)
/// are not in this list; they stay "not configured".
pub const DEFAULT_PROVIDERS: [UsageProvider; 2] =
    [UsageProvider::Claude, UsageProvider::CommandCode];

type UsageResult = Vec<ProviderSnapshot>;

#[derive(Debug)]
pub struct UsageHost {
    pub enabled: Vec<UsageProvider>,
    pub snapshots: UsageResult,
    last_started: Option<Instant>,
    inflight: Option<mpsc::Receiver<UsageResult>>,
}

impl Default for UsageHost {
    fn default() -> Self {
        Self {
            enabled: DEFAULT_PROVIDERS.to_vec(),
            snapshots: Vec::new(),
            last_started: None,
            inflight: None,
        }
    }
}

impl UsageHost {
    /// True when no fetch is running and the refresh interval has elapsed (or none ran yet).
    #[must_use]
    pub fn due(&self, now: Instant) -> bool {
        self.inflight.is_none()
            && self
                .last_started
                .is_none_or(|t| now.duration_since(t) >= USAGE_REFRESH_INTERVAL)
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
                    self.snapshots = snapshots;
                    self.inflight = None;
                    changed = true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => self.inflight = None,
            }
        }
        if self.due(now) {
            self.last_started = Some(now);
            self.inflight = Some(fetch(self.enabled.clone()));
        }
        changed
    }
}

/// Production fetch on a worker thread: one request per enabled provider, bounded by the
/// fetch timeout. Tokens come from `$CLAUDE_CONFIG_DIR` or `$HOME` and `COMMAND_CODE_API_KEY`.
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
        let now = chrono::Utc::now().timestamp();
        providers
            .into_iter()
            .map(|provider| {
                let (token, base) = match provider {
                    UsageProvider::Claude => {
                        (claude_token(&home, config_dir.as_deref()), CLAUDE_BASE)
                    }
                    UsageProvider::CommandCode => (
                        commandcode_token(std::env::var("COMMAND_CODE_API_KEY").ok().as_deref()),
                        COMMANDCODE_BASE,
                    ),
                    _ => (None, ""),
                };
                fetch_snapshot(provider, token.as_deref(), base, now)
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
        tx.send(vec![snapshot(UsageProvider::Claude)]).unwrap();
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
        assert!(host.snapshots.is_empty());
    }
}
