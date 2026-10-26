//! Process memory readers backed by `/proc/self/smaps` plus process CPU.

use crate::memory::{SmapsSummary, parse_smaps_text};
use std::sync::atomic::{AtomicU64, Ordering};

/// Read and parse `/proc/self/smaps`, returning PSS/RSS/swap rollups.
pub fn smaps_rollup_kb() -> Option<SmapsSummary> {
    let text = std::fs::read_to_string("/proc/self/smaps").ok()?;
    Some(parse_smaps_text(&text))
}

/// Return current process PSS in KiB, if readable.
pub fn process_pss_kb() -> Option<u64> {
    smaps_rollup_kb().map(|s| s.pss_kb)
}

/// VmRSS fallback (KiB) from `/proc/self/status` when smaps is unreadable.
pub fn vmrss_fallback_kb() -> Option<u64> {
    parse_status_vmrss(&std::fs::read_to_string("/proc/self/status").ok()?)
}

pub(crate) fn parse_status_vmrss(text: &str) -> Option<u64> {
    for line in text.lines() {
        let (key, rest) = line.split_once(':')?;
        if key.trim() == "VmRSS" {
            let mut parts = rest.split_whitespace();
            return parts.next()?.parse::<u64>().ok();
        }
    }
    None
}

fn now_process_cpu_ns() -> u64 {
    crate::clock_ns(rustix::time::ClockId::ProcessCPUTime)
}

/// One-core CPU percent: `cpu_delta / window * 100`. 100% = one core.
pub fn cpu_percent(cpu_delta_ns: u64, window_ns: u64) -> f64 {
    if window_ns == 0 {
        return 0.0;
    }
    (cpu_delta_ns as f64) * 100.0 / (window_ns as f64)
}

static LAST_PROC_CPU: AtomicU64 = AtomicU64::new(0);
static LAST_PROC_WALL: AtomicU64 = AtomicU64::new(0);

/// Interval process-CPU delta: (cpu_delta_ns, wall_delta_ns, one-core pct).
/// First call anchors the baseline and reports a zero delta.
pub(crate) fn process_delta(window_fallback_ns: u64) -> (u64, u64, f64) {
    let cpu = now_process_cpu_ns();
    let wall = crate::now_mono_ns();
    let last_cpu = LAST_PROC_CPU.swap(cpu, Ordering::Relaxed);
    let last_wall = LAST_PROC_WALL.swap(wall, Ordering::Relaxed);
    if last_cpu == 0 || last_wall == 0 {
        return (0, window_fallback_ns, 0.0);
    }
    let cpu_delta = cpu.saturating_sub(last_cpu);
    let wall_delta = wall.saturating_sub(last_wall).max(1);
    (cpu_delta, wall_delta, cpu_percent(cpu_delta, wall_delta))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_formula_one_core() {
        let _guard = crate::span::TEST_LOCK.lock().unwrap();
        assert!((cpu_percent(500_000_000, 1_000_000_000) - 50.0).abs() < 1e-9);
        assert!((cpu_percent(2_000_000_000, 1_000_000_000) - 200.0).abs() < 1e-9);
        assert_eq!(cpu_percent(1, 0), 0.0);
    }

    #[test]
    fn smaps_parse_sums() {
        let _guard = crate::span::TEST_LOCK.lock().unwrap();
        let text = "Pss: 10 kB\nRss: 20 kB\nSwap: 3 kB\nPss: 5 kB\nSize: 99 kB\n";
        let s = parse_smaps_text(text);
        assert_eq!(s.pss_kb, 15);
        assert_eq!(s.rss_kb, 20);
        assert_eq!(s.swap_kb, 3);
    }

    #[test]
    fn status_rss_fallback_parses() {
        let _guard = crate::span::TEST_LOCK.lock().unwrap();
        let text = "Name:\tx\nVmRSS:\t   12345 kB\n";
        assert_eq!(parse_status_vmrss(text), Some(12345));
        assert_eq!(parse_status_vmrss("Name:\tx\n"), None);
    }

    #[test]
    fn process_cpu_clock_counts_other_threads() {
        let process_start = now_process_cpu_ns();
        let thread_start = crate::clock_ns(rustix::time::ClockId::ThreadCPUTime);
        std::thread::spawn(|| {
            let start = std::time::Instant::now();
            while start.elapsed() < std::time::Duration::from_millis(30) {
                std::hint::black_box(());
            }
        })
        .join()
        .unwrap();
        let process_delta = now_process_cpu_ns() - process_start;
        let thread_delta = crate::clock_ns(rustix::time::ClockId::ThreadCPUTime) - thread_start;
        assert!(
            process_delta >= 20_000_000,
            "process CPU clock advanced only {process_delta} ns"
        );
        assert!(
            thread_delta < 10_000_000,
            "calling thread CPU clock advanced {thread_delta} ns"
        );
    }
}
