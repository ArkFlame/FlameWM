//! Startup, memory, idle and window profiling subcommands.

use std::time::{Duration, Instant};

use x11rb::protocol::xproto::MapState;

use crate::canary::{Canary, WinInfo};
use crate::cli::{arg, arg_num, has};
use crate::commands::probe::{resolve_target, viewable_count};
use crate::output::{fail, json_str};

/// J11 workload hooks (read-only probes; no daemon, no product changes).
/// profile-startup: time until a named surface is viewable (Start-equivalent gate).
pub(crate) fn cmd_profile_startup(canary: &Canary, args: &[String]) {
    let name = arg(args, "--name").unwrap_or_else(|| "flame".to_owned());
    let timeout: u64 = arg_num(args, "--timeout-secs", 10);
    let start = Instant::now();
    let limit = Duration::from_secs(timeout.max(1));
    loop {
        let hits = canary.find(&name);
        let viewable = viewable_count(&hits);
        let startup_ms = start.elapsed().as_millis();
        if viewable > 0 {
            let first = &hits
                .iter()
                .find(|w| w.map_state == u8::from(MapState::VIEWABLE))
                .cloned()
                .unwrap_or_else(|| hits[0].clone());
            if has(args, "--json") {
                println!(
                    "{{\"startup_ms\":{},\"name\":{},\"matched\":{},\"viewable\":{},\"first_id\":{}}}",
                    startup_ms,
                    json_str(&name),
                    hits.len(),
                    viewable,
                    first.id
                );
            } else {
                println!(
                    "CANARY_PROFILE_STARTUP startup_ms={} name={:?} matched={} viewable={} first_id={}",
                    startup_ms,
                    name,
                    hits.len(),
                    viewable,
                    first.id
                );
            }
            return;
        }
        if start.elapsed() >= limit {
            if has(args, "--json") {
                println!(
                    "{{\"startup_ms\":{},\"name\":{},\"matched\":{},\"viewable\":0,\"timeout\":true}}",
                    startup_ms,
                    json_str(&name),
                    hits.len()
                );
            } else {
                println!(
                    "CANARY_PROFILE_STARTUP startup_ms={} name={:?} matched={} viewable=0 timeout=true",
                    startup_ms,
                    name,
                    hits.len()
                );
            }
            std::process::exit(1);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// profile-pss: read /proc/<pid>/`smaps_rollup` for a pid or name pattern.
pub(crate) fn cmd_profile_pss(args: &[String]) {
    let pid: u32 = if let Some(v) = arg(args, "--pid") {
        v.parse().unwrap_or_else(|_| fail("--pid must be a number"))
    } else {
        let pat = arg(args, "--pattern")
            .unwrap_or_else(|| fail("profile-pss needs --pid <n> or --pattern <substr>"));
        let out = std::process::Command::new("pgrep")
            .arg("-f")
            .arg(&pat)
            .output()
            .unwrap_or_else(|_| fail("pgrep failed"));
        let first = String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_owned();
        if first.is_empty() {
            fail(&format!("no process matches {pat:?}"));
        }
        first
            .parse()
            .unwrap_or_else(|_| fail("pgrep returned non-numeric pid"))
    };
    let path = format!("/proc/{pid}/smaps_rollup");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|_| fail(&format!("cannot read {path}")));
    let mut pss = 0u64;
    let mut rss = 0u64;
    let mut swap = 0u64;
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let key = it.next().unwrap_or("");
        let val: u64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        match key {
            "Pss:" => pss = val,
            "Rss:" => rss = val,
            "Swap:" => swap = val,
            _ => {}
        }
    }
    if has(args, "--json") {
        println!("{{\"pid\":{pid},\"pss_kb\":{pss},\"rss_kb\":{rss},\"swap_kb\":{swap}}}");
    } else {
        println!("CANARY_PROFILE_PSS pid={pid} pss={pss}kB rss={rss}kB swap={swap}kB");
    }
}

/// profile-idle: sleep bounded window to separate idle baseline from workload.
pub(crate) fn cmd_profile_idle(args: &[String]) {
    let secs: u64 = arg_num(args, "--secs", 5);
    let secs = secs.max(1).min(60);
    let start = Instant::now();
    std::thread::sleep(Duration::from_secs(secs));
    let elapsed_ms = start.elapsed().as_millis();
    if has(args, "--json") {
        println!("{{\"idle_secs\":{secs},\"elapsed_ms\":{elapsed_ms}}}");
    } else {
        println!("CANARY_PROFILE_IDLE idle_secs={secs} elapsed_ms={elapsed_ms}");
    }
}

/// profile-window: Start/window-gate snapshot (geometry + map + sample mean).
pub(crate) fn cmd_profile_window(canary: &Canary, args: &[String]) {
    let target = resolve_target(canary, args);
    let w = target.width.max(1).min(64);
    let h = target.height.max(1).min(64);
    let (all_zero, pixels, mr, mg, mb) = canary
        .sample(target.id, 0, 0, w, h)
        .map(|d| Canary::pixel_stats(&d))
        .unwrap_or((true, 0, 0, 0, 0));
    if has(args, "--json") {
        println!(
            "{{\"window\":{},\"name\":{},\"geom\":[{},{},{},{}],\"map_state\":{},\"pixels\":{},\"all_zero\":{},\"mean\":[{},{},{}]}}",
            target.id,
            json_str(&target.name),
            target.x,
            target.y,
            target.width,
            target.height,
            json_str(target.map_name()),
            pixels,
            all_zero,
            mr,
            mg,
            mb
        );
    } else {
        println!(
            "CANARY_PROFILE_WINDOW id={} geom={}x{}+{}+{} map={} pixels={} all_zero={} mean=#{:02x}{:02x}{:02x}",
            target.id,
            target.width,
            target.height,
            target.x,
            target.y,
            target.map_name(),
            pixels,
            all_zero,
            mr,
            mg,
            mb
        );
    }
}

/// Runs `probe` against the first "flame" window; a missing window is a failure.
pub(crate) fn probe_flame(
    canary: &Canary,
    probe: impl FnOnce(&WinInfo) -> (bool, String),
) -> (bool, String) {
    match canary.find("flame").into_iter().next() {
        Some(w) => probe(&w),
        None => (false, "no target".to_owned()),
    }
}
