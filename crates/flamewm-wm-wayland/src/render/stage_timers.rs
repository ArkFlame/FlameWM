//! Per-stage frame time of the nested (winit) paint, for composition reports: which share of the
//! compositor's own frame time goes to backdrops, element building, readbacks, drawing, submit
//! and frame callbacks. Seven relaxed atomic adds and `Instant::now` calls per painted frame.
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

#[derive(Clone, Copy)]
pub(crate) enum Stage {
    DesktopBackdrop,
    DockBackdrops,
    Elements,
    Readbacks,
    Draw,
    Submit,
    Callbacks,
}

const NAMES: [&str; 7] = [
    "desktop_backdrop",
    "dock_backdrops",
    "elements",
    "readbacks",
    "draw",
    "submit",
    "callbacks",
];

static NANOS: [AtomicU64; 7] = [const { AtomicU64::new(0) }; 7];
static FRAMES: AtomicU64 = AtomicU64::new(0);

/// Marks stage boundaries within one frame: each `mark` charges the time since the previous one.
pub(crate) struct Lap(Instant);

impl Lap {
    pub(crate) fn start() -> Self {
        FRAMES.fetch_add(1, Ordering::Relaxed);
        Self(Instant::now())
    }

    pub(crate) fn mark(&mut self, stage: Stage) {
        let now = Instant::now();
        let spent = now.saturating_duration_since(self.0).as_nanos();
        NANOS[stage as usize]
            .fetch_add(u64::try_from(spent).unwrap_or(u64::MAX), Ordering::Relaxed);
        self.0 = now;
    }
}

/// Zeroes the counters, so a report covers only what follows.
pub(crate) fn reset() {
    FRAMES.store(0, Ordering::Relaxed);
    for stage in &NANOS {
        stage.store(0, Ordering::Relaxed);
    }
}

/// `frames=N name=share% ...` over the time charged since the last `reset`, largest share first.
pub(crate) fn report() -> String {
    let frames = FRAMES.load(Ordering::Relaxed);
    let times: Vec<u64> = NANOS.iter().map(|ns| ns.load(Ordering::Relaxed)).collect();
    format_report(frames, &times)
}

fn format_report(frames: u64, nanos: &[u64]) -> String {
    let total: u64 = nanos.iter().sum();
    let mut ranked: Vec<(&str, u64)> = NAMES.iter().copied().zip(nanos.iter().copied()).collect();
    ranked.sort_by_key(|&(_, ns)| std::cmp::Reverse(ns));
    let mut out = format!("frames={frames} total_ms={}", total / 1_000_000);
    for (name, ns) in ranked {
        if total > 0 && ns > 0 {
            out.push_str(&format!(" {name}={}%", ns * 100 / total));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::format_report;

    #[test]
    fn report_ranks_stages_by_share_and_skips_empty_ones() {
        let report = format_report(3, &[0, 0, 1_000_000, 0, 6_000_000, 3_000_000, 0]);
        assert_eq!(
            report,
            "frames=3 total_ms=10 draw=60% submit=30% elements=10%"
        );
    }

    #[test]
    fn empty_report_has_no_shares() {
        assert_eq!(format_report(0, &[0; 7]), "frames=0 total_ms=0");
    }
}
