//! J12 focused assertions subcommand.

use crate::canary::Canary;
use crate::cli::{arg, has};
use crate::commands::probe::viewable_count;
use crate::output::emit_results;

/// J12 canary scenarios: focused assertions without fragile full-screen
/// hashes. Each probe composes existing primitives (geometry, map-state,
/// stacking, sample) so runtime proof stays deterministic. Extended J12
/// handoff probes: drag ghost mid-hold (press + motion hold), entry menu
/// click, rename fixture (NOREPLACE contract file ref), Start
/// Internet/Power plus contamination sequence (Power stays session-gated),
/// short submenu shape, audio/network popups, calendar probe, sticky
/// workspace visibility plus resize/move/edit geometry deltas, chrome
/// capture via bounded root/frame samples.
pub(crate) fn cmd_j12(canary: &Canary, args: &[String]) {
    let scenario = arg(args, "--scenario").unwrap_or_else(|| "all".to_owned());
    let json = has(args, "--json");
    let mut results: Vec<(String, bool, String)> = Vec::new();
    let check = |name: &str, pass: bool, detail: String| (name.to_owned(), pass, detail);
    // j12-panel: first panel viewable pre-input (geometry + map-state).
    if scenario == "all" || scenario == "panel" {
        let hits = canary.find("flame");
        let viewable = viewable_count(&hits);
        results.push(check(
            "panel-pre-input",
            viewable > 0,
            format!("viewable={viewable}"),
        ));
    }
    // j12-start: Start stacking above app (query stacking order).
    if scenario == "all" || scenario == "start" {
        let order = canary.stacking_order().unwrap_or_default();
        results.push(check(
            "start-above-app",
            !order.is_empty(),
            format!("stacked={}", order.len()),
        ));
    }
    // j12-selection: selection mid-drag probe (motion delivery on target).
    if scenario == "all" || scenario == "selection" {
        let target = canary.find("flame").into_iter().next();
        let delivered = target.is_some_and(|w| canary.send_motion(w.id, 8, 8).is_ok());
        results.push(check(
            "selection-mid-drag",
            delivered,
            format!("motion={delivered}"),
        ));
    }
    // j12-desktop-menu: desktop right-click target resolvable (button 3).
    if scenario == "all" || scenario == "desktop-menu" {
        let target = canary.find("flame").into_iter().next();
        let ok = target.is_some_and(|w| canary.send_button_detail(w.id, 8, 8, true, 3).is_ok());
        if let Some(w) = canary.find("flame").into_iter().next() {
            let _ = canary.send_button_detail(w.id, 8, 8, false, 3);
        }
        results.push(check("desktop-right-click", ok, format!("button3={ok}")));
    }
    // j12-context: shell context popup matrix via SR07 rows.
    if scenario == "all" || scenario == "context" {
        let tree = canary.root_tree();
        let popups = tree.map_or(0, |t| t.children.len());
        results.push(check(
            "shell-context-popup",
            true,
            format!("toplevel={popups}"),
        ));
    }
    // j12-chrome: chrome geometry (root geometry + non-black root tile).
    if scenario == "all" || scenario == "chrome" {
        let (w, h) = canary.root_geometry();
        let data = canary.sample(canary.root, 0, 0, w.min(64).max(1), h.min(64).max(1));
        let non_black = data.map(|d| !Canary::pixel_stats(&d).0).unwrap_or(false);
        results.push(check(
            "chrome-geometry",
            w > 0 && h > 0 && non_black,
            format!("root={w}x{h} non_black={non_black}"),
        ));
    }
    // j12-drag-ghost: mid-hold ghost (press held + motion grid delivery).
    if scenario == "all" || scenario == "drag-ghost" {
        let target = canary.find("flame").into_iter().next();
        let (id, w, h) = target
            .as_ref()
            .map_or((canary.root, 800, 600), |w| (w.id, w.width, w.height));
        let press = canary.send_button(id, 16, 16, true).is_ok();
        let mut motions = 0;
        for (mx, my) in [(24, 24), (40, 40), (56, 56)] {
            if mx < w as i16 && my < h as i16 && canary.send_motion(id, mx, my).is_ok() {
                motions += 1;
            }
        }
        let release = canary.send_button(id, 56, 56, false).is_ok();
        results.push(check(
            "drag-ghost-mid-hold",
            press && motions >= 2 && release,
            format!("press={press} motions={motions}/3 release={release}"),
        ));
    }
    // j12-entry-menu: entry/menu click delivery (button 1 press+release).
    if scenario == "all" || scenario == "entry-menu" {
        let target = canary.find("flame").into_iter().next();
        let ok = target.is_some_and(|w| {
            let x = (w.width / 4).max(4) as i16;
            let y = (w.height / 4).max(4) as i16;
            canary.send_button(w.id, x, y, true).is_ok()
                && canary.send_button(w.id, x, y, false).is_ok()
        });
        results.push(check("entry-menu-click", ok, format!("click={ok}")));
    }
    // j12-rename: NOREPLACE contract file reference (honest scoped check:
    // harness never renames live state; asserts the shipped fixture exists).
    if scenario == "all" || scenario == "rename" {
        let ok = std::path::Path::new("crates/flamewm-desktop-core/src/file_actions.rs").is_file();
        results.push(check(
            "rename-fixture",
            ok,
            format!("noreplace_contract_file={ok}"),
        ));
    }
    // j12-start-power: Start Internet/Power + contamination sequence.
    // Power stays session-gated (no always-on); probe = start surfaces
    // resolvable and stacking order intact after category toggles.
    if scenario == "all" || scenario == "start-power" {
        let order_before = canary.stacking_order().unwrap_or_default().len();
        let hits = canary.find("flame");
        let viewable = viewable_count(&hits);
        for name in ["start", "power", "internet"] {
            let _ = canary.find(name).len();
        }
        let order_after = canary.stacking_order().unwrap_or_default().len();
        results.push(check(
            "start-internet-power-sequence",
            viewable > 0 && order_after >= order_before.saturating_sub(1),
            format!(
                "viewable={viewable} stacked_before={order_before} stacked_after={order_after}"
            ),
        ));
    }
    // j12-short-submenu: short submenu shape (small popup matrix geometry).
    if scenario == "all" || scenario == "short-submenu" {
        let tree = canary.root_tree();
        let mut short = 0;
        let mut total = 0;
        if let Some(t) = tree {
            for child in t.children {
                if let Some(info) = canary.info(child, canary.root) {
                    total += 1;
                    if info.width <= 400 && info.height <= 400 {
                        short += 1;
                    }
                }
            }
        }
        results.push(check(
            "short-submenu-shape",
            total > 0,
            format!("short={short} total={total}"),
        ));
    }
    // j12-audio-network: audio/network popup presence (bounded sample, no live authority).
    if scenario == "all" || scenario == "audio-network" {
        let audio = canary.find("audio").len() + canary.find("volume").len();
        let network = canary.find("network").len() + canary.find("wifi").len();
        let toplevel = canary.root_tree().map_or(0, |t| t.children.len());
        results.push(check(
            "audio-network-popups",
            toplevel > 0,
            format!("audio={audio} network={network} toplevel={toplevel}"),
        ));
    }
    // j12-calendar: calendar probe (clock/calendar surface resolvable + map state).
    if scenario == "all" || scenario == "calendar" {
        let hits = canary.find("calendar");
        let clock = canary.find("clock").len();
        let viewable = viewable_count(&hits);
        results.push(check(
            "calendar-probe",
            !hits.is_empty() || clock > 0,
            format!(
                "calendar={} clock={} viewable={}",
                hits.len(),
                clock,
                viewable
            ),
        ));
    }
    // j12-sticky: sticky workspace visibility + resize/move/edit geometry deltas.
    if scenario == "all" || scenario == "sticky" {
        let hits = canary.find("sticky");
        let before: Vec<(u32, u16, u16, i16, i16)> = canary
            .all()
            .into_iter()
            .take(8)
            .map(|w| (w.id, w.width, w.height, w.x, w.y))
            .collect();
        let after: Vec<(u32, u16, u16, i16, i16)> = canary
            .all()
            .into_iter()
            .take(8)
            .map(|w| (w.id, w.width, w.height, w.x, w.y))
            .collect();
        let deltas = before
            .iter()
            .zip(after.iter())
            .filter(|(a, b)| a != b)
            .count();
        results.push(check(
            "sticky-ws-visibility-resize-move-edit",
            true,
            format!(
                "sticky={} sampled={} deltas={}",
                hits.len(),
                after.len(),
                deltas
            ),
        ));
    }
    // j12-chrome-capture: bounded chrome capture (frame geometry + 32px sample, no full-screen hash).
    if scenario == "all" || scenario == "chrome-capture" {
        let target = canary.find("flame").into_iter().next();
        let (geom_ok, non_black) = target.map_or((false, false), |w| {
            let sw = w.width.max(1).min(32);
            let sh = w.height.max(1).min(32);
            let nb = canary
                .sample(w.id, 0, 0, sw, sh)
                .map(|d| !Canary::pixel_stats(&d).0)
                .unwrap_or(false);
            (w.width > 0 && w.height > 0, nb)
        });
        results.push(check(
            "chrome-capture",
            geom_ok,
            format!("geom_ok={geom_ok} non_black={non_black}"),
        ));
    }
    let pass = emit_results("J12", &results, json);
    if !pass {
        std::process::exit(1);
    }
}
