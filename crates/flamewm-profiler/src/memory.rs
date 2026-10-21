//! Memory gauges and `/proc/self/smaps` parser.

use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};

const MAX_GAUGES: usize = 32;

struct GaugeSlot {
    label: RwLock<Option<&'static str>>,
    current: AtomicU64,
    peak: AtomicU64,
}

impl GaugeSlot {
    const fn new() -> Self {
        Self {
            label: RwLock::new(None),
            current: AtomicU64::new(0),
            peak: AtomicU64::new(0),
        }
    }
}

macro_rules! gauge_slots {
    ($($n:expr),*) => {
        [$( { let _ = $n; GaugeSlot::new() } ),*]
    };
}

static GAUGES: [GaugeSlot; MAX_GAUGES] = gauge_slots!(
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    26, 27, 28, 29, 30, 31
);

fn gauge_slot(label: &'static str) -> usize {
    for (i, slot) in GAUGES.iter().enumerate() {
        if slot.label.read().map(|g| *g).unwrap_or(None) == Some(label) {
            return i;
        }
    }
    for (i, slot) in GAUGES.iter().enumerate() {
        let mut guard = slot.label.write().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(label);
            return i;
        }
        if *guard == Some(label) {
            return i;
        }
    }
    MAX_GAUGES - 1
}

/// Current/peak memory gauge under a static label (bytes).
pub struct MemoryGauge {
    idx: usize,
}

impl MemoryGauge {
    /// Bind a gauge to a static label.
    pub fn new(label: &'static str) -> Self {
        Self {
            idx: gauge_slot(label),
        }
    }

    /// Set the current value; peak tracks the maximum.
    pub fn set(&self, bytes: u64) {
        GAUGES[self.idx].current.store(bytes, Ordering::Relaxed);
        let _ = GAUGES[self.idx].peak.fetch_max(bytes, Ordering::Relaxed);
    }

    /// Read the current value.
    pub fn current(&self) -> u64 {
        GAUGES[self.idx].current.load(Ordering::Relaxed)
    }

    /// Read the peak value.
    pub fn peak(&self) -> u64 {
        GAUGES[self.idx].peak.load(Ordering::Relaxed)
    }
}

pub(crate) fn gauge_snapshot() -> Vec<(&'static str, u64, u64)> {
    let mut out = Vec::new();
    for slot in GAUGES.iter() {
        if let Some(label) = slot.label.read().map(|g| *g).unwrap_or(None) {
            let cur = slot.current.load(Ordering::Relaxed);
            let peak = slot.peak.load(Ordering::Relaxed);
            if cur > 0 || peak > 0 {
                out.push((label, cur, peak));
            }
        }
    }
    out
}

/// Parsed smaps rollup.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SmapsSummary {
    /// Sum of `Pss:` lines in KiB.
    pub pss_kb: u64,
    /// Sum of `Rss:` lines in KiB.
    pub rss_kb: u64,
    /// Sum of `Swap:` lines in KiB.
    pub swap_kb: u64,
}

/// Parse smaps-format text, summing Pss/Rss/Swap lines (values in kB).
pub fn parse_smaps_text(text: &str) -> SmapsSummary {
    let mut out = SmapsSummary::default();
    for line in text.lines() {
        let line = line.trim_start();
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let mut parts = rest.trim().split_whitespace();
        let value: u64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        match key {
            "Pss" => out.pss_kb = out.pss_kb.saturating_add(value),
            "Rss" => out.rss_kb = out.rss_kb.saturating_add(value),
            "Swap" => out.swap_kb = out.swap_kb.saturating_add(value),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gauge_registration_tracks_current_and_peak_in_snapshot() {
        let _guard = crate::span::TEST_LOCK.lock().unwrap();
        let gauge = MemoryGauge::new("t_gauge_current_peak");
        gauge.set(13);
        gauge.set(8);

        assert_eq!(gauge.current(), 8);
        assert_eq!(gauge.peak(), 13);
        assert!(gauge_snapshot().iter().any(|(label, current, peak)| {
            *label == "t_gauge_current_peak" && *current == 8 && *peak == 13
        }));
    }
}
