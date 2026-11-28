//! Typed policy for absent optional D-Bus services.
//!
//! One owner for "the service is missing" degradation: a rate-limited diagnostic record and a
//! bounded exponential retry backoff. Providers consume this facade through `lib.rs`; they never
//! duplicate the backoff arithmetic or open a second timing loop. The state machine is pure — it
//! only computes the next wait, and the caller performs that wait on its existing pump/worker
//! boundary — so no new dependency or async runtime is introduced.

use std::time::Duration;

/// Bounded exponential backoff for re-contacting an absent D-Bus service.
///
/// `next_wait()` advances the budget and returns the delay before the next attempt, or `None` once
/// the budget is spent. The caller owns the actual wait; this type never sleeps or touches a bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AbsentRetry {
    base_ms: u64,
    cap_ms: u64,
    retries: u8,
    used: u8,
}

impl AbsentRetry {
    /// Portal default: three waits (20ms, 40ms, 80ms), each capped at 2s.
    pub const PORTAL: Self = Self::new(20, 2_000, 3);

    /// Build a policy with a base delay, a per-wait cap and a fixed number of retries.
    pub const fn new(base_ms: u64, cap_ms: u64, retries: u8) -> Self {
        Self {
            base_ms,
            cap_ms,
            retries,
            used: 0,
        }
    }

    /// Advance the budget and return the next wait, or `None` after the last retry.
    pub fn next_wait(&mut self) -> Option<Duration> {
        if self.used >= self.retries {
            return None;
        }
        let shift = u32::from(self.used);
        self.used += 1;
        let doubled = self
            .base_ms
            .saturating_mul(1u64.checked_shl(shift).unwrap_or(u64::MAX));
        Some(Duration::from_millis(doubled.min(self.cap_ms)))
    }

    /// How many waits have been issued; useful to diagnostics and tests.
    pub const fn used(&self) -> u8 {
        self.used
    }
}

impl Default for AbsentRetry {
    fn default() -> Self {
        Self::PORTAL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_until_cap_then_stops() {
        let mut retry = AbsentRetry::new(10, 25, 4);
        assert_eq!(retry.next_wait(), Some(Duration::from_millis(10)));
        assert_eq!(retry.next_wait(), Some(Duration::from_millis(20)));
        assert_eq!(retry.next_wait(), Some(Duration::from_millis(25)));
        assert_eq!(retry.next_wait(), Some(Duration::from_millis(25)));
        assert_eq!(retry.next_wait(), None);
        assert_eq!(retry.used(), 4);
    }

    #[test]
    fn portal_default_retries_three_times_within_cap() {
        let mut retry = AbsentRetry::PORTAL;
        let waits: Vec<Duration> = core::iter::from_fn(|| retry.next_wait()).collect();
        assert_eq!(
            waits,
            [
                Duration::from_millis(20),
                Duration::from_millis(40),
                Duration::from_millis(80),
            ]
        );
    }
}
