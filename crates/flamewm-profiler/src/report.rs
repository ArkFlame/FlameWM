//! Report window: snapshot-and-reset aggregates, process header, file append.
//!
//! Profiler owns aggregate formatting and per-report file append via
//! `FLAMEWM_PROFILE_DIR` (`<process>.profile.log`). No per-span I/O, no
//! reporter thread, no dynamic labels.

use crate::memory::gauge_snapshot;
use crate::process::{process_delta, smaps_rollup_kb, vmrss_fallback_kb};
use crate::stats::{SpanAgg, by_highest_count, by_highest_total, by_slowest_single};
use std::sync::OnceLock;

static PROCESS_OVERRIDE: OnceLock<&'static str> = OnceLock::new();

/// Set the owning process name once (first call wins). On the winning call,
/// truncate `$FLAMEWM_PROFILE_DIR/<process>.profile.log` once so each
/// process generation starts fresh; later calls in the same generation
/// never wipe appended windows. A restart is a new generation and
/// truncates again, keeping generations separated.
pub(crate) fn set_process_override(name: &'static str) {
    if PROCESS_OVERRIDE.set(name).is_ok() {
        truncate_profile_log_once(name);
    }
}

fn fmt_ms(ns: u64) -> String {
    format!("{:.2}ms", ns as f64 / 1_000_000.0)
}

fn process_name() -> String {
    if let Some(name) = PROCESS_OVERRIDE.get() {
        return (*name).to_owned();
    }
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.file_stem().and_then(|s| s.to_str()).map(str::to_owned))
            .unwrap_or_else(|| "flamewm".to_owned())
    })
    .clone()
}

/// Sanitize to `[A-Za-z0-9_.-]`; empty becomes `flamewm`.
pub(crate) fn profile_filename(process: &str) -> String {
    let mut clean: String = process
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if clean.is_empty() {
        clean.push_str("flamewm");
    }
    format!("{clean}.profile.log")
}

/// Render one window: snapshot-and-reset spans/counters, retain memory
/// peaks, prepend process CPU/memory header, append to
/// `$FLAMEWM_PROFILE_DIR/<process>.profile.log` when set.
/// Returns empty string when disabled or no samples exist.
pub fn report_window() -> String {
    if !crate::enabled() {
        return String::new();
    }
    let top = crate::config().top.clamp(1, 8);
    let interval = crate::config().interval_secs;
    let spans = crate::stats::span_aggs_reset();
    let counters = crate::stats::counter_aggs_reset();
    let mut gauges = gauge_snapshot();
    gauges.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(b.0)));
    let presents = crate::present_interval::take_present_interval();

    if spans.is_empty() && counters.is_empty() && gauges.is_empty() && presents.is_none() {
        // Still advance the process-CPU baseline so the next window's delta
        // covers only its own interval.
        let _ = process_delta(interval.saturating_mul(1_000_000_000));
        return String::new();
    }

    let (proc_cpu_ns, proc_wall_ns, proc_pct) =
        process_delta(interval.saturating_mul(1_000_000_000));

    let mut out = String::from("FLAMEWM_PROFILE_SUMMARY\n");
    out.push_str(&format!(
        "PROCESS name={} cpu_total={} cpu_pct={:.1} window={}\n",
        process_name(),
        fmt_ms(proc_cpu_ns),
        proc_pct,
        fmt_ms(proc_wall_ns),
    ));
    if let Some(smaps) = smaps_rollup_kb() {
        out.push_str(&format!(
            " smaps pss={}kB rss={}kB swap={}kB\n",
            smaps.pss_kb, smaps.rss_kb, smaps.swap_kb
        ));
    } else if let Some(rss) = vmrss_fallback_kb() {
        out.push_str(&format!(" status rss={rss}kB (smaps unreadable)\n"));
    }

    let slowest = by_slowest_single(spans_into(&spans));
    out.push_str("SLOWEST_SINGLE:\n");
    for row in slowest.iter().take(top) {
        out.push_str(&format!(
            " {} count={} max_wall={} p95_wall={} total_wall={} self_wall={} max_cpu={} over16ms={}\n",
            row.label,
            row.calls,
            fmt_ms(row.max_wall),
            fmt_ms(row.p95_wall),
            fmt_ms(row.total_wall),
            fmt_ms(row.self_wall),
            fmt_ms(row.max_cpu),
            row.over_16ms
        ));
    }

    let highest = by_highest_total(spans_into(&spans));
    out.push_str("HIGHEST_TOTAL:\n");
    for row in highest.iter().take(top) {
        out.push_str(&format!(
            " {} count={} total_wall={} self_wall={} total_cpu={} cpu_pct={:.1} max_wall={}\n",
            row.label,
            row.calls,
            fmt_ms(row.total_wall),
            fmt_ms(row.self_wall),
            fmt_ms(row.total_cpu),
            row.cpu_percent_one_core(proc_wall_ns.max(1)),
            fmt_ms(row.max_wall)
        ));
    }

    let frequent = by_highest_count(spans_into(&spans));
    out.push_str("HIGHEST_CALL_COUNT:\n");
    for row in frequent.iter().take(top) {
        out.push_str(&format!(
            " {} count={} total_wall={} self_wall={}\n",
            row.label,
            row.calls,
            fmt_ms(row.total_wall),
            fmt_ms(row.self_wall)
        ));
    }

    if !counters.is_empty() {
        out.push_str("COUNTERS:\n");
        for (label, value) in counters.iter().take(top) {
            out.push_str(&format!(" {label} value={value}\n"));
        }
    }

    if let Some(p) = presents {
        out.push_str("PRESENT_INTERVAL:\n");
        out.push_str(&format!(
            " present-interval count={} p50={} p95={} max={}\n",
            p.count,
            fmt_ms(p.p50_ns),
            fmt_ms(p.p95_ns),
            fmt_ms(p.max_ns)
        ));
    }

    out.push_str("MEMORY_TOP:\n");
    for (label, cur, peak) in gauges.iter().take(top) {
        out.push_str(&format!(" {label} current={cur}B peak={peak}B\n"));
    }

    out.push_str(&format!(
        "INTERVAL_SECS={} TOP={}\n",
        crate::config().interval_secs,
        top
    ));

    append_to_profile_dir(&out);
    out
}

fn spans_into(rows: &[SpanAgg]) -> Vec<SpanAgg> {
    rows.iter()
        .map(|r| SpanAgg {
            label: r.label,
            calls: r.calls,
            total_wall: r.total_wall,
            self_wall: r.self_wall,
            max_wall: r.max_wall,
            p95_wall: r.p95_wall,
            total_cpu: r.total_cpu,
            self_cpu: r.self_cpu,
            max_cpu: r.max_cpu,
            over_16ms: r.over_16ms,
        })
        .collect()
}

fn append_to_profile_dir(text: &str) {
    let Some(dir) = std::env::var_os("FLAMEWM_PROFILE_DIR").filter(|d| !d.is_empty()) else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let path = dir.join(profile_filename(&process_name()));
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::io::Write as _;
        let _ = file.write_all(text.as_bytes());
    }
}

/// Truncate the per-process profile log once per process generation.
/// Only the winning `init_process` call truncates; a fresh process
/// (restart) is a new generation and truncates again.
fn truncate_profile_log_once(name: &str) {
    let Some(dir) = std::env::var_os("FLAMEWM_PROFILE_DIR").filter(|d| !d.is_empty()) else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let path = dir.join(profile_filename(name));
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&path)
    {
        drop(file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_sanitizes() {
        let _guard = crate::span::TEST_LOCK.lock().unwrap();
        assert_eq!(profile_filename("flamewm-wm"), "flamewm-wm.profile.log");
        assert_eq!(profile_filename("a/b c"), "a_b_c.profile.log");
        assert_eq!(profile_filename(""), "flamewm.profile.log");
    }

    #[test]
    fn disabled_reports_nothing() {
        let _guard = crate::span::TEST_LOCK.lock().unwrap();
        // FLAMEWM_PROFILE is unset in test env; enabled() must be false and
        // report_window must not create files or drain state.
        if crate::enabled() {
            return;
        }
        assert_eq!(report_window(), "");
    }
}
