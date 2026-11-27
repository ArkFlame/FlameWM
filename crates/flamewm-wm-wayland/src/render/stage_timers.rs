//! Per-stage frame time of the nested (winit) paint, for composition reports: which share of the
//! compositor's own frame time goes to backdrops, element building, readbacks, drawing, submit
//! and frame callbacks, plus a breakdown of the per-window part of the element build. A few
//! relaxed atomic adds and `Instant::now` calls per painted frame.
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
    // Detail of `Elements`, per managed window (not charged while building a backdrop capture).
    Outline,
    Surface,
    WindowBackdrop,
    Titlebar,
}

const NAMES: [&str; 11] = [
    "desktop_backdrop",
    "dock_backdrops",
    "elements",
    "readbacks",
    "draw",
    "submit",
    "callbacks",
    "outline",
    "surface",
    "window_backdrop",
    "titlebar",
];
/// Stages before this index are frame stages; the rest break `Elements` down.
const FRAME_STAGES: usize = 7;

static NANOS: [AtomicU64; 11] = [const { AtomicU64::new(0) }; 11];
static FRAMES: AtomicU64 = AtomicU64::new(0);

/// Marks stage boundaries: each `mark` charges the time since the previous one.
pub(crate) struct Lap {
    last: Instant,
    enabled: bool,
}

impl Lap {
    /// Starts timing one painted frame.
    pub(crate) fn start() -> Self {
        FRAMES.fetch_add(1, Ordering::Relaxed);
        Self {
            last: Instant::now(),
            enabled: true,
        }
    }

    /// Times a sub-stage of the element build. Inert when `enabled` is false, so a backdrop
    /// capture's nested scene build is not counted as the frame's own element work.
    pub(crate) fn detail(enabled: bool) -> Self {
        Self {
            last: Instant::now(),
            enabled,
        }
    }

    pub(crate) fn mark(&mut self, stage: Stage) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        let spent = now.saturating_duration_since(self.last).as_nanos();
        NANOS[stage as usize]
            .fetch_add(u64::try_from(spent).unwrap_or(u64::MAX), Ordering::Relaxed);
        self.last = now;
    }
}

/// Zeroes the counters, so a report covers only what follows.
pub(crate) fn reset() {
    FRAMES.store(0, Ordering::Relaxed);
    for stage in &NANOS {
        stage.store(0, Ordering::Relaxed);
    }
}

/// `frames=N total_ms=.. name=share% ... | elements_detail: name=share% ...`, largest first. The
/// frame shares are of the frame stages' total; the detail shares are of the detail total.
pub(crate) fn report() -> String {
    let frames = FRAMES.load(Ordering::Relaxed);
    let times: Vec<u64> = NANOS.iter().map(|ns| ns.load(Ordering::Relaxed)).collect();
    format_report(frames, &times)
}

fn shares(names: &[&'static str], nanos: &[u64]) -> (u64, String) {
    let total: u64 = nanos.iter().sum();
    let mut ranked: Vec<(&str, u64)> = names.iter().copied().zip(nanos.iter().copied()).collect();
    ranked.sort_by_key(|&(_, ns)| std::cmp::Reverse(ns));
    let mut out = String::new();
    for (name, ns) in ranked {
        if total > 0 && ns > 0 {
            out.push_str(&format!(" {name}={}%", ns * 100 / total));
        }
    }
    (total, out)
}

fn format_report(frames: u64, nanos: &[u64]) -> String {
    let (total, frame) = shares(&NAMES[..FRAME_STAGES], &nanos[..FRAME_STAGES]);
    let (detail_total, detail) = shares(&NAMES[FRAME_STAGES..], &nanos[FRAME_STAGES..]);
    let mut out = format!("frames={frames} total_ms={}{frame}", total / 1_000_000);
    if detail_total > 0 {
        out.push_str(&format!(
            " | elements_detail_ms={}:{detail}",
            detail_total / 1_000_000
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::format_report;

    #[test]
    fn report_ranks_stages_by_share_and_skips_empty_ones() {
        let report = format_report(
            3,
            &[0, 0, 1_000_000, 0, 6_000_000, 3_000_000, 0, 0, 0, 0, 0],
        );
        assert_eq!(
            report,
            "frames=3 total_ms=10 draw=60% submit=30% elements=10%"
        );
    }

    #[test]
    fn detail_shares_are_separate_from_frame_shares() {
        let report = format_report(
            1,
            &[0, 0, 4_000_000, 0, 0, 0, 0, 1_000_000, 0, 0, 3_000_000],
        );
        assert_eq!(
            report,
            "frames=1 total_ms=4 elements=100% | elements_detail_ms=4: titlebar=75% outline=25%"
        );
    }

    #[test]
    fn empty_report_has_no_shares() {
        assert_eq!(format_report(0, &[0; 11]), "frames=0 total_ms=0");
    }
}
