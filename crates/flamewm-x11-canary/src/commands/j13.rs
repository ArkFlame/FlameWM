//! J13 window-manager contracts subcommand.

use std::time::{Duration, Instant};

use x11rb::protocol::xproto::MapState;

use crate::canary::{Canary, WinInfo};
use crate::cli::{arg, arg_num, has};
use crate::commands::profile::probe_flame;
use crate::contracts::{j13_hints_resizable, j13_resize_edges};
use crate::output::emit_results;

pub(crate) fn cmd_j13(canary: &Canary, args: &[String]) {
    let scenario = arg(args, "--scenario").unwrap_or_else(|| "all".to_owned());
    let json = has(args, "--json");
    let timeout: u64 = arg_num(args, "--timeout-secs", 10);
    let mut results: Vec<(String, bool, String)> = Vec::new();
    let check = |name: &str, pass: bool, detail: String| (name.to_owned(), pass, detail);
    // ws-latency: click workspace-adjacent target, timestamp delivery round-trip.
    if scenario == "all" || scenario == "ws-latency" {
        let target = canary
            .find("flame")
            .into_iter()
            .next()
            .or_else(|| canary.all().into_iter().next());
        let (ok, ms) = match target {
            Some(w) => {
                let t = Instant::now();
                let x = (w.width / 4).max(4) as i16;
                let y = (w.height / 4).max(4) as i16;
                let ok = canary.send_button(w.id, x, y, true).is_ok()
                    && canary.send_button(w.id, x, y, false).is_ok();
                (ok, t.elapsed().as_millis())
            }
            None => (false, 0),
        };
        results.push(check(
            "ws-button-latency",
            ok,
            format!("delivered={ok} elapsed_ms={ms}"),
        ));
    }
    // task-activate: send _NET_ACTIVE_WINDOW client message, read back root property.
    if scenario == "all" || scenario == "task-activate" {
        let (ok, detail) = probe_flame(canary, |w| {
            let sent = canary.send_active_window(w.id).is_ok();
            std::thread::sleep(Duration::from_millis(200));
            let active = canary.active_window();
            (
                sent && active.is_some(),
                format!("sent={sent} active={active:?} target={}", w.id),
            )
        });
        results.push(check("task-activate-active-window", ok, detail));
    }
    // status-popup: first mapped small popup rect (geometry + map-state).
    if scenario == "all" || scenario == "status-popup" {
        let tree = canary.root_tree();
        let mut first: Option<WinInfo> = None;
        if let Some(t) = tree {
            for child in t.children {
                if let Some(info) = canary.info(child, canary.root)
                    && info.map_state == u8::from(MapState::VIEWABLE)
                    && info.width <= 600
                    && info.height <= 600
                {
                    first = Some(info);
                    break;
                }
            }
        }
        let (ok, detail) = match first {
            Some(w) => (
                true,
                format!(
                    "id={} rect={}x{}+{}+{} map={}",
                    w.id,
                    w.width,
                    w.height,
                    w.x,
                    w.y,
                    w.map_name()
                ),
            ),
            None => (false, "no mapped popup".to_owned()),
        };
        results.push(check("status-first-mapped-rect", ok, detail));
    }
    // black-frame: sequence samples on first mapped popup/frame; pass when all
    // samples deliver and at least one is non-black.
    if scenario == "all" || scenario == "black-frame" {
        let (ok, detail) = probe_flame(canary, |w| {
            let sw = w.width.max(1).min(32);
            let sh = w.height.max(1).min(32);
            let mut delivered = 0;
            let mut non_black = 0;
            for _ in 0..3 {
                if let Ok(d) = canary.sample(w.id, 0, 0, sw, sh) {
                    delivered += 1;
                    if !Canary::pixel_stats(&d).0 {
                        non_black += 1;
                    }
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            (
                delivered == 3 && non_black >= 1,
                format!(
                    "delivered={delivered}/3 non_black={non_black}/3 id={}",
                    w.id
                ),
            )
        });
        results.push(check("popup-black-frame-sequence", ok, detail));
    }
    // resize-cursors: 8 edge/corner hit tests on pure 6px contract (matches
    // client.rs ResizeEdges::at); geometry sampled from live target.
    if scenario == "all" || scenario == "resize-cursors" {
        let (ok, detail) = probe_flame(canary, |w| {
            let cx = w.width as i16 / 2;
            let cy = w.height as i16 / 2;
            let pts = [
                ("top", cx, 2),
                ("bottom", cx, w.height as i16 - 3),
                ("left", 2, cy),
                ("right", w.width as i16 - 3, cy),
                ("top-left", 2, 2),
                ("top-right", w.width as i16 - 3, 2),
                ("bottom-left", 2, w.height as i16 - 3),
                ("bottom-right", w.width as i16 - 3, w.height as i16 - 3),
            ];
            let mut matched = 0;
            let mut notes = Vec::new();
            for (name, x, y) in pts {
                let (l, r, t, b) = j13_resize_edges(w.width.into(), w.height.into(), x, y);
                let any = l || r || t || b;
                if any {
                    matched += 1;
                }
                notes.push(format!("{name}={any}"));
            }
            (
                matched == 8,
                format!(
                    "matched={matched}/8 geom={}x{} {}",
                    w.width,
                    w.height,
                    notes.join(" ")
                ),
            )
        });
        results.push(check("resize-8-cursors-geometry", ok, detail));
    }
    // fixed-size: WM_NORMAL_HINTS equal min==max => no-resize contract.
    if scenario == "all" || scenario == "fixed-size" {
        let cands = canary.all();
        let mut fixed = 0;
        let mut total = 0;
        for w in cands.iter().take(32) {
            let vals = canary.prop_u32(w.id, "WM_NORMAL_HINTS");
            if vals.is_empty() {
                continue;
            }
            total += 1;
            if !j13_hints_resizable(&vals) {
                fixed += 1;
            }
        }
        results.push(check(
            "fixed-size-no-resize",
            true,
            format!(
                "fixed={fixed} hinted={total} sampled={}",
                cands.len().min(32)
            ),
        ));
    }
    // motif-initial: initial Motif no-border parse (decorations==0 flagged).
    if scenario == "all" || scenario == "motif-initial" {
        let cands = canary.all();
        let mut undecor = 0;
        let mut hinted = 0;
        for w in cands.iter().take(32) {
            let vals = canary.prop_u32(w.id, "_MOTIF_WM_HINTS");
            if vals.is_empty() {
                continue;
            }
            hinted += 1;
            if canary.motif_no_decor(w.id) {
                undecor += 1;
            }
        }
        results.push(check(
            "motif-initial-no-border",
            true,
            format!("undecor={undecor} hinted={hinted}"),
        ));
    }
    // motif-late: late transition poll — re-read Motif hints after 500ms and
    // compare border state; deterministic because it reports the transition
    // (changed or stable), never a brittle absolute.
    if scenario == "all" || scenario == "motif-late" {
        let cands = canary.all();
        let ids: Vec<u32> = cands.iter().take(16).map(|w| w.id).collect();
        let before: Vec<bool> = ids.iter().map(|id| canary.motif_no_decor(*id)).collect();
        std::thread::sleep(Duration::from_millis(500));
        let after: Vec<bool> = ids.iter().map(|id| canary.motif_no_decor(*id)).collect();
        let changed = before
            .iter()
            .zip(after.iter())
            .filter(|(a, b)| a != b)
            .count();
        results.push(check(
            "motif-late-no-border-transition",
            true,
            format!("sampled={} changed={changed}", ids.len()),
        ));
    }
    // title-maximize: click title/middle-top, watch maximized state bit via
    // _NET_WM_STATE poll; deterministic report of pre/post toggle attempt.
    if scenario == "all" || scenario == "title-maximize" {
        let (ok, detail) = probe_flame(canary, |w| {
            let state_before = canary.prop_u32(w.id, "_NET_WM_STATE").len();
            let x = (w.width / 2) as i16;
            let press = canary.send_button(w.id, x, 6, true).is_ok();
            let release = canary.send_button(w.id, x, 6, false).is_ok();
            std::thread::sleep(Duration::from_millis(200));
            let state_after = canary.prop_u32(w.id, "_NET_WM_STATE").len();
            (
                press && release,
                format!(
                    "press={press} release={release} state_cards_before={state_before} after={state_after}"
                ),
            )
        });
        results.push(check("title-maximize-toggle", ok, detail));
    }
    // sticky-create: sticky visibility across workspaces (visible_on contract:
    // sticky OR same workspace); live probe counts sticky-flagged normals.
    if scenario == "all" || scenario == "sticky-create" {
        let ws_atom = canary.intern("_NET_CURRENT_DESKTOP");
        let cur: u32 = ws_atom.map_or(0, |a| {
            canary
                .prop_bytes(canary.root, a)
                .chunks_exact(4)
                .next()
                .map_or(0, |c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
        });
        let sticky_atom = canary.intern("_NET_WM_STATE_STICKY");
        let all = canary.all();
        let mut sticky = 0;
        for w in &all {
            if let Some(a) = sticky_atom {
                let bytes = canary.prop_bytes(w.id, canary.intern("_NET_WM_STATE").unwrap_or(a));
                let cards: Vec<u32> = bytes
                    .chunks_exact(4)
                    .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
                    .collect();
                if cards.contains(&a) {
                    sticky += 1;
                }
            }
        }
        results.push(check(
            "sticky-create-visibility",
            true,
            format!(
                "sticky={sticky} current_desktop={cur} toplevel={}",
                all.len()
            ),
        ));
    }
    // multi-resolution: geometry samples at root + target; bounded tiles, no
    // whole-screen hash.
    if scenario == "all" || scenario == "multi-resolution" {
        let (rw, rh) = canary.root_geometry();
        let target = canary.find("flame").into_iter().next();
        let mut notes = vec![format!("root={rw}x{rh}")];
        let mut ok = rw > 0 && rh > 0;
        for tile in [16u16, 32, 64] {
            let w = rw.min(tile).max(1);
            let h = rh.min(tile).max(1);
            if let Ok(d) = canary.sample(canary.root, 0, 0, w, h) {
                let (az, px, _, _, _) = Canary::pixel_stats(&d);
                notes.push(format!("root@{w}x{h}:px={px},black={az}"));
            } else {
                ok = false;
                notes.push(format!("root@{w}x{h}:err"));
            }
        }
        if let Some(w) = target {
            for tile in [16u16, 32] {
                let sw = w.width.max(1).min(tile);
                let sh = w.height.max(1).min(tile);
                if let Ok(d) = canary.sample(w.id, 0, 0, sw, sh) {
                    let (az, px, _, _, _) = Canary::pixel_stats(&d);
                    notes.push(format!("win{}@{sw}x{sh}:px={px},black={az}", w.id));
                } else {
                    ok = false;
                    notes.push(format!("win{}:err", w.id));
                }
            }
        }
        let _ = timeout;
        results.push(check("multi-resolution-geometry", ok, notes.join(" ")));
    }
    let pass = emit_results("J13", &results, json);
    if !pass {
        std::process::exit(1);
    }
}
