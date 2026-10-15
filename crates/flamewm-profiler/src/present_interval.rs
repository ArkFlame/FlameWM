//! Present-interval recorder: Instant deltas between successive presents.
//!
//! Steady state is one branch when profiling is off. When on, each present
//! takes one short mutex lock and writes one slot of a fixed ring. Percentiles
//! are computed only at report time over the retained ring; `count` and `max`
//! cover every interval in the window.

use std::sync::Mutex;

use crate::{enabled, now_mono_ns};

const RING_CAP: usize = 1024;

pub(crate) struct IntervalState {
    last_ns: Option<u64>,
    count: u64,
    max_ns: u64,
    next: usize,
    ring: [u64; RING_CAP],
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct IntervalSummary {
    pub(crate) count: u64,
    pub(crate) p50_ns: u64,
    pub(crate) p95_ns: u64,
    pub(crate) max_ns: u64,
}

impl IntervalState {
    pub(crate) const fn new() -> Self {
        Self {
            last_ns: None,
            count: 0,
            max_ns: 0,
            next: 0,
            ring: [0; RING_CAP],
        }
    }

    /// Record a present at `now_ns`. The first present only sets the baseline.
    pub(crate) fn record(&mut self, now_ns: u64) {
        if let Some(last) = self.last_ns {
            let delta = now_ns.saturating_sub(last);
            self.count += 1;
            self.max_ns = self.max_ns.max(delta);
            self.ring[self.next] = delta;
            self.next = (self.next + 1) % RING_CAP;
        }
        self.last_ns = Some(now_ns);
    }

    /// Summarize and clear window samples. The baseline survives so the next
    /// window's first delta still spans the boundary.
    pub(crate) fn take_summary(&mut self) -> Option<IntervalSummary> {
        if self.count == 0 {
            return None;
        }
        let retained = self.count.min(RING_CAP as u64) as usize;
        let mut samples = self.ring[..retained].to_vec();
        samples.sort_unstable();
        let summary = IntervalSummary {
            count: self.count,
            p50_ns: nearest_rank(&samples, 50),
            p95_ns: nearest_rank(&samples, 95),
            max_ns: self.max_ns,
        };
        self.count = 0;
        self.max_ns = 0;
        self.next = 0;
        Some(summary)
    }
}

/// Nearest-rank percentile over a sorted, non-empty slice.
fn nearest_rank(sorted: &[u64], pct: usize) -> u64 {
    let rank = (sorted.len() * pct).div_ceil(100).max(1);
    sorted[rank - 1]
}

static STATE: Mutex<IntervalState> = Mutex::new(IntervalState::new());

/// Record one present. No-op beyond one branch when `FLAMEWM_PROFILE` is off.
pub fn record_present() {
    if !enabled() {
        return;
    }
    let now = now_mono_ns();
    STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .record(now);
}

/// Drain the current window's present-interval summary for the report.
pub(crate) fn take_present_interval() -> Option<IntervalSummary> {
    STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take_summary()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_present_sets_baseline_only() {
        let mut s = IntervalState::new();
        s.record(1_000);
        assert!(s.take_summary().is_none());
    }

    #[test]
    fn summarizes_deltas_with_nearest_rank_percentiles() {
        let mut s = IntervalState::new();
        // Deltas: 10, 20, 30, ..., 200 (20 samples).
        let mut t = 0u64;
        s.record(t);
        for i in 1..=20u64 {
            t += i * 10;
            s.record(t);
        }
        let sum = s.take_summary().expect("summary");
        assert_eq!(sum.count, 20);
        assert_eq!(sum.p50_ns, 100);
        assert_eq!(sum.p95_ns, 190);
        assert_eq!(sum.max_ns, 200);
    }

    #[test]
    fn take_resets_window_but_keeps_baseline() {
        let mut s = IntervalState::new();
        s.record(0);
        s.record(500);
        assert!(s.take_summary().is_some());
        assert!(s.take_summary().is_none());
        // Next window's first delta spans the boundary from t=500.
        s.record(700);
        let sum = s.take_summary().expect("summary");
        assert_eq!(sum.count, 1);
        assert_eq!(sum.p50_ns, 200);
        assert_eq!(sum.max_ns, 200);
    }

    #[test]
    fn ring_wrap_keeps_total_count_and_true_max() {
        let mut s = IntervalState::new();
        let mut t = 0u64;
        s.record(t);
        // One large spike first, then enough small deltas to evict it from the ring.
        t += 9_000;
        s.record(t);
        for _ in 0..RING_CAP {
            t += 1;
            s.record(t);
        }
        let sum = s.take_summary().expect("summary");
        assert_eq!(sum.count, RING_CAP as u64 + 1);
        assert_eq!(sum.max_ns, 9_000);
        assert_eq!(sum.p95_ns, 1);
    }

    #[test]
    fn saturating_delta_never_underflows() {
        let mut s = IntervalState::new();
        s.record(100);
        s.record(50);
        let sum = s.take_summary().expect("summary");
        assert_eq!(sum.max_ns, 0);
    }
}
