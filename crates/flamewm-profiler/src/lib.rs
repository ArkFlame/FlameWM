//! FlameWM profiler: static-label CPU/span/counter/memory aggregation.
//!
//! Frozen seams (R01-R06): no product instrumentation lives here. Product
//! crates opt in later through [`start`], [`ProfilePoint`], [`CounterPoint`],
//! and [`MemoryGauge`]. Steady-state span paths perform no allocation, spawn
//! no thread, and accept only `&'static str` labels.

mod memory;
mod point;
mod present_interval;
mod process;
mod report;
mod span;
mod stats;

pub use memory::{MemoryGauge, SmapsSummary, parse_smaps_text};
pub use point::{CounterPoint, ProfilePoint, SpanGuard, start};
pub use present_interval::record_present;
pub use process::{cpu_percent, process_pss_kb, smaps_rollup_kb, vmrss_fallback_kb};
pub use report::report_window;
pub use span::record_elapsed;
pub use stats::{SpanAgg, by_highest_count, by_highest_total, by_slowest_single};

use std::sync::OnceLock;

pub(crate) fn clock_ns(id: rustix::time::ClockId) -> u64 {
    let ts = rustix::time::clock_gettime(id);
    (ts.tv_sec.max(0) as u64) * 1_000_000_000 + (ts.tv_nsec.max(0) as u64)
}

pub(crate) fn now_cpu_ns() -> u64 {
    clock_ns(rustix::time::ClockId::ThreadCPUTime)
}

pub(crate) fn now_mono_ns() -> u64 {
    clock_ns(rustix::time::ClockId::Monotonic)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_cpu_clock_does_not_advance_while_sleeping() {
        let cpu_start = now_cpu_ns();
        let mono_start = now_mono_ns();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let mono_delta = now_mono_ns() - mono_start;
        let cpu_delta = now_cpu_ns() - cpu_start;
        assert!(
            mono_delta >= 50_000_000,
            "monotonic advanced {mono_delta} ns"
        );
        assert!(
            cpu_delta < 25_000_000,
            "thread CPU clock advanced {cpu_delta} ns during sleep"
        );
    }
}

struct Config {
    pub(crate) enabled: bool,
    pub(crate) interval_secs: u64,
    pub(crate) top: usize,
}

pub(crate) fn config() -> &'static Config {
    static CFG: OnceLock<Config> = OnceLock::new();
    CFG.get_or_init(|| {
        let enabled = std::env::var("FLAMEWM_PROFILE").is_ok_and(|v| v == "1");
        let interval_secs: u64 = std::env::var("FLAMEWM_PROFILE_INTERVAL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60);
        let interval_secs = interval_secs.max(10);
        let top = std::env::var("FLAMEWM_PROFILE_TOP")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8);
        Config {
            enabled,
            interval_secs,
            top,
        }
    })
}

/// Returns true when `FLAMEWM_PROFILE=1` is set.
pub fn enabled() -> bool {
    config().enabled
}

/// Declare the owning process name once (first call wins). Used for the
/// `<process>.profile.log` filename under `FLAMEWM_PROFILE_DIR`.
pub fn init_process(name: &'static str) {
    report::set_process_override(name);
}

/// Report cadence in seconds from `FLAMEWM_PROFILE_INTERVAL` (floor 10s,
/// default 60s). Exposed so wm/desktop/shell reactor timers honor the same
/// interval the profiler labels with instead of hardcoding 60s.
#[must_use]
pub fn profile_interval_secs() -> u64 {
    config().interval_secs
}
