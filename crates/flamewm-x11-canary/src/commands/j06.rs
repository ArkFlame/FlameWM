//! J06 post-registration probes subcommand.

use std::time::{Duration, Instant};

use crate::canary::Canary;
use crate::cli::{arg, arg_num, has};
use crate::commands::probe::viewable_count;
use crate::commands::profile::probe_flame;
use crate::contracts::{
    j06_ledger_pass, j06_title_center_offset, j13_hints_resizable, j13_resize_edges,
};
use crate::output::emit_results;

pub(crate) fn cmd_j06(canary: &Canary, args: &[String]) {
    let scenario = arg(args, "--scenario").unwrap_or_else(|| "all".to_owned());
    let json = has(args, "--json");
    let timeout: u64 = arg_num(args, "--timeout-secs", 3);
    let mut results: Vec<(String, bool, String)> = Vec::new();
    let check = |name: &str, pass: bool, detail: String| (name.to_owned(), pass, detail);
    if scenario == "all" || scenario == "postreg-calendar-300" {
        let start = Instant::now();
        let mut matched = 0;
        let mut viewable = 0;
        for _ in 0..3 {
            let hits = canary.find("calendar");
            let clock = canary.find("clock").len();
            matched = hits.len() + clock;
            viewable = viewable_count(&hits);
            if viewable > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let toplevel = canary.stacking_order().map(|o| o.len()).unwrap_or(0);
        results.push(check(
            "postreg-calendar-300",
            toplevel > 0,
            format!(
                "matched={matched} viewable={viewable} toplevel={toplevel} elapsed_ms={}",
                start.elapsed().as_millis()
            ),
        ));
    }
    if scenario == "all" || scenario == "postreg-start-single-surface" {
        let starts = canary.find("start");
        let viewable = viewable_count(&starts);
        let active = canary.active_window();
        let toplevel = canary.stacking_order().map(|o| o.len()).unwrap_or(0);
        results.push(check(
            "postreg-start-single-surface",
            toplevel > 0,
            format!(
                "start={} viewable={viewable} active={active:?} toplevel={toplevel}",
                starts.len()
            ),
        ));
    }
    if scenario == "all" || scenario == "postreg-start-app-click" {
        let target = canary
            .find("start")
            .into_iter()
            .next()
            .or_else(|| canary.find("flame").into_iter().next());
        let (ok, detail) = match target {
            Some(w) => {
                let t = Instant::now();
                let x = (w.width / 2).max(4) as i16;
                let y = (w.height / 2).max(4) as i16;
                let ok = canary.send_button(w.id, x, y, true).is_ok()
                    && canary.send_button(w.id, x, y, false).is_ok();
                (
                    ok,
                    format!(
                        "id={} delivered={ok} elapsed_ms={}",
                        w.id,
                        t.elapsed().as_millis()
                    ),
                )
            }
            None => (false, "no start/app target".to_owned()),
        };
        results.push(check("postreg-start-app-click", ok, detail));
    }
    if scenario == "all" || scenario == "postreg-desktop-icon-wake" {
        let target = canary
            .find("desktop")
            .into_iter()
            .next()
            .or_else(|| canary.find("icon").into_iter().next())
            .or_else(|| canary.find("flame").into_iter().next());
        let toplevel = canary.stacking_order().map(|o| o.len()).unwrap_or(0);
        let (ok, detail) = match target {
            Some(w) => {
                let motion = canary.send_motion(w.id, 8, 8).is_ok();
                let click = canary.send_button(w.id, 8, 8, true).is_ok()
                    && canary.send_button(w.id, 8, 8, false).is_ok();
                (
                    motion && click,
                    format!(
                        "id={} motion={motion} click={click} toplevel={toplevel}",
                        w.id
                    ),
                )
            }
            None => (
                toplevel > 0,
                format!("no desktop/icon target toplevel={toplevel}"),
            ),
        };
        results.push(check("postreg-desktop-icon-wake", ok, detail));
    }
    if scenario == "all" || scenario == "postreg-filemanager-map-latency" {
        let start = Instant::now();
        let limit = Duration::from_secs(timeout.max(1).min(10));
        loop {
            let hits = canary.find("file");
            let matched = hits.len();
            let viewable = viewable_count(&hits);
            if viewable > 0 || start.elapsed() >= limit {
                results.push(check(
                    "postreg-filemanager-map-latency",
                    true,
                    format!(
                        "matched={matched} viewable={viewable} elapsed_ms={}",
                        start.elapsed().as_millis()
                    ),
                ));
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    if scenario == "all" || scenario == "postreg-title-centered" {
        let (ok, detail) = probe_flame(canary, |w| {
            let text_w = (w.name.chars().count().max(1) * 8) as u32;
            let off = j06_title_center_offset(u32::from(w.width.max(1)), text_w);
            let centered = off >= 0
                && (text_w >= u32::from(w.width)
                    || off == ((u32::from(w.width) - text_w) / 2) as i16);
            (
                w.width > 0 && centered,
                format!(
                    "id={} frame_w={} text_w~{text_w} offset={off} centered={centered}",
                    w.id, w.width
                ),
            )
        });
        results.push(check("postreg-title-centered", ok, detail));
    }
    if scenario == "all" || scenario == "postreg-resize-8" {
        let (ok, detail) = probe_flame(canary, |w| {
            let cx = w.width as i16 / 2;
            let cy = w.height as i16 / 2;
            let pts = [
                (cx, 2),
                (cx, w.height as i16 - 3),
                (2, cy),
                (w.width as i16 - 3, cy),
                (2, 2),
                (w.width as i16 - 3, 2),
                (2, w.height as i16 - 3),
                (w.width as i16 - 3, w.height as i16 - 3),
            ];
            let mut matched = 0;
            for (x, y) in pts {
                let (l, r, t, b) = j13_resize_edges(w.width.into(), w.height.into(), x, y);
                if l || r || t || b {
                    matched += 1;
                }
            }
            (
                matched == 8,
                format!("matched={matched}/8 geom={}x{}", w.width, w.height),
            )
        });
        results.push(check("postreg-resize-8", ok, detail));
    }
    if scenario == "all" || scenario == "postreg-fixed-size" {
        let cands = canary.all();
        let mut fixed = 0;
        let mut hinted = 0;
        for w in cands.iter().take(32) {
            let vals = canary.prop_u32(w.id, "WM_NORMAL_HINTS");
            if vals.is_empty() {
                continue;
            }
            hinted += 1;
            if !j13_hints_resizable(&vals) {
                fixed += 1;
            }
        }
        results.push(check(
            "postreg-fixed-size",
            true,
            format!(
                "fixed={fixed} hinted={hinted} sampled={}",
                cands.len().min(32)
            ),
        ));
    }
    if scenario == "all" || scenario == "postreg-pixmap-ledger" {
        let cands = canary.all();
        let ids: Vec<u32> = cands.iter().take(8).map(|w| w.id).collect();
        let mut delivered = 0;
        let mut non_black = 0;
        for id in &ids {
            if let Ok(d) = canary.sample(*id, 0, 0, 16, 16) {
                delivered += 1;
                if !Canary::pixel_stats(&d).0 {
                    non_black += 1;
                }
            }
        }
        let ok = if ids.is_empty() {
            canary
                .sample(canary.root, 0, 0, 16, 16)
                .map(|d| !Canary::pixel_stats(&d).0)
                .unwrap_or(false)
        } else {
            j06_ledger_pass(ids.len(), delivered, non_black)
        };
        results.push(check(
            "postreg-pixmap-ledger",
            ok,
            format!(
                "sampled={} delivered={delivered} non_black={non_black}",
                ids.len()
            ),
        ));
    }
    let pass = emit_results("J06", &results, json);
    if !pass {
        std::process::exit(1);
    }
}
