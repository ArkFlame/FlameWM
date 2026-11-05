//! SR05/SR07/SR08/SR15 scenario subcommands.

use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, MapState};

use crate::canary::{Canary, WinInfo};
use crate::cli::{arg, arg_num, has};
use crate::commands::probe::{resolve_target, viewable_count};
use crate::output::{fail, json_str};

/// SR05 first-present: wait until a named surface is mapped/viewable.
pub(crate) fn cmd_sr05(canary: &Canary, args: &[String]) {
    let name = arg(args, "--name").unwrap_or_else(|| "flame".to_owned());
    let timeout: u64 = arg_num(args, "--timeout-secs", 10);
    let start = Instant::now();
    let limit = Duration::from_secs(timeout.max(1));
    loop {
        let hits = canary.find(&name);
        let viewable = viewable_count(&hits);
        if viewable > 0 {
            let first = &hits
                .iter()
                .find(|w| w.map_state == u8::from(MapState::VIEWABLE))
                .cloned()
                .unwrap_or_else(|| hits[0].clone());
            if has(args, "--json") {
                println!(
                    "{{\"scenario\":\"SR05\",\"pass\":true,\"name\":{},\"matched\":{},\
                     \"viewable\":{},\"first\":{},\"elapsed_ms\":{}}}",
                    json_str(&name),
                    hits.len(),
                    viewable,
                    first.to_json(),
                    start.elapsed().as_millis()
                );
            } else {
                println!(
                    "CANARY_SR05 pass=true name={:?} matched={} viewable={} \
                     first_id={} geom={}x{}+{}+{} elapsed_ms={}",
                    name,
                    hits.len(),
                    viewable,
                    first.id,
                    first.width,
                    first.height,
                    first.x,
                    first.y,
                    start.elapsed().as_millis()
                );
            }
            return;
        }
        if start.elapsed() >= limit {
            if has(args, "--json") {
                println!(
                    "{{\"scenario\":\"SR05\",\"pass\":false,\"name\":{},\"matched\":{},\
                     \"viewable\":0,\"elapsed_ms\":{}}}",
                    json_str(&name),
                    hits.len(),
                    start.elapsed().as_millis()
                );
            } else {
                println!(
                    "CANARY_SR05 pass=false name={:?} matched={} viewable=0 elapsed_ms={}",
                    name,
                    hits.len(),
                    start.elapsed().as_millis()
                );
            }
            std::process::exit(1);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// SR07 popup matrix: enumerate transient/override-redirect-style small
/// top-level windows under root and report geometry/map matrix.
pub(crate) fn cmd_sr07(canary: &Canary, args: &[String]) {
    let tree = canary
        .conn
        .query_tree(canary.root)
        .map_err(|e| e.to_string())
        .unwrap_or_else(|e| fail(&e))
        .reply()
        .map_err(|e| e.to_string())
        .unwrap_or_else(|e| fail(&e));
    let mut rows: Vec<WinInfo> = Vec::new();
    for child in tree.children {
        if let Some(info) = canary.info(child, canary.root) {
            let small = info.width <= 600 && info.height <= 600;
            if small || info.map_state != u8::from(MapState::VIEWABLE) {
                rows.push(info);
            }
        }
    }
    if let Some(parent) = arg(args, "--parent") {
        let needle = parent.to_lowercase();
        rows.retain(|w| {
            w.name.to_lowercase().contains(&needle) || w.class.to_lowercase().contains(&needle)
        });
    }
    let viewable = viewable_count(&rows);
    if has(args, "--json") {
        let items: Vec<String> = rows.iter().map(WinInfo::to_json).collect();
        println!(
            "{{\"scenario\":\"SR07\",\"popups\":{},\"viewable\":{},\"matrix\":[{}]}}",
            rows.len(),
            viewable,
            items.join(",")
        );
    } else {
        for w in &rows {
            println!(
                "popup id={} geom={}x{}+{}+{} map={} name={:?}",
                w.id,
                w.width,
                w.height,
                w.x,
                w.y,
                w.map_name(),
                w.name
            );
        }
        println!("CANARY_SR07 popups={} viewable={}", rows.len(), viewable);
    }
}

/// SR08 black-pixel: sample root (tiled) or a named window; fail on all-black.
pub(crate) fn cmd_sr08(canary: &Canary, args: &[String]) {
    let json = has(args, "--json");
    let mut samples: Vec<(u32, i16, i16, u16, u16, Vec<u8>)> = Vec::new();
    if let Some(id) = arg(args, "--window") {
        let id: u32 = id
            .parse()
            .unwrap_or_else(|_| fail("--window must be a number"));
        let info = canary
            .by_id(id)
            .unwrap_or_else(|| fail(&format!("window {id} not found")));
        let w = info.width.max(1).min(256);
        let h = info.height.max(1).min(256);
        let data = canary.sample(id, 0, 0, w, h).unwrap_or_else(|e| fail(&e));
        samples.push((id, 0, 0, w, h, data));
    } else if let Some(name) = arg(args, "--name") {
        let target = resolve_target(canary, &["x".to_owned(), "--name".to_owned(), name]);
        let w = target.width.max(1).min(256);
        let h = target.height.max(1).min(256);
        let data = canary
            .sample(target.id, 0, 0, w, h)
            .unwrap_or_else(|e| fail(&e));
        samples.push((target.id, 0, 0, w, h, data));
    } else {
        // Tile root: 4 corners + center at 64x64.
        let setup = &canary.conn.setup().roots[canary.screen];
        let sw = i32::from(setup.width_in_pixels);
        let sh = i32::from(setup.height_in_pixels);
        let tile: i16 = 64;
        let points = [
            (0, 0),
            (sw.saturating_sub(64) as i16, 0),
            (0, sh.saturating_sub(64) as i16),
            (sw.saturating_sub(64) as i16, sh.saturating_sub(64) as i16),
            ((sw / 2 - 32) as i16, (sh / 2 - 32) as i16),
        ];
        for (x, y) in points {
            match canary.sample(canary.root, x.max(0), y.max(0), tile as u16, tile as u16) {
                Ok(data) => samples.push((canary.root, x, y, 64, 64, data)),
                Err(e) => fail(&e),
            }
        }
    }
    let mut all_zero_total = 0;
    for (win, x, y, w, h, data) in &samples {
        let (all_zero, pixels, mr, mg, mb) = Canary::pixel_stats(data);
        if all_zero {
            all_zero_total += 1;
        }
        if json {
            println!(
                "{{\"scenario\":\"SR08\",\"window\":{win},\"x\":{x},\"y\":{y},\
                 \"width\":{w},\"height\":{h},\"pixels\":{pixels},\
                 \"all_zero\":{all_zero},\"mean_r\":{mr},\"mean_g\":{mg},\"mean_b\":{mb}}}"
            );
        } else {
            println!(
                "CANARY_SR08 window={win} rect={w}x{h}+{x}+{y} pixels={pixels} \
                 all_zero={all_zero} mean=#{mr:02x}{mg:02x}{mb:02x}"
            );
        }
    }
    let pass = all_zero_total < samples.len();
    if json {
        println!(
            "{{\"scenario\":\"SR08\",\"pass\":{pass},\"samples\":{},\"all_zero_tiles\":{all_zero_total}}}",
            samples.len()
        );
    } else {
        println!(
            "CANARY_SR08 pass={pass} samples={} all_zero_tiles={all_zero_total}",
            samples.len()
        );
    }
    if !pass {
        std::process::exit(1);
    }
}

/// SR15 input matrix: warp-free synthetic motion grid + click per point on
/// the target window; reports per-point delivery.
pub(crate) fn cmd_sr15(canary: &Canary, args: &[String]) {
    let target = resolve_target(canary, args);
    let points: Vec<(i16, i16)> = if let Some(spec) = arg(args, "--points") {
        spec.split(';')
            .filter(|p| !p.trim().is_empty())
            .map(|p| {
                let mut it = p.split(',');
                let x: i16 = it
                    .next()
                    .unwrap_or("0")
                    .trim()
                    .parse()
                    .unwrap_or_else(|_| fail("--points must be x,y;..."));
                let y: i16 = it
                    .next()
                    .unwrap_or("0")
                    .trim()
                    .parse()
                    .unwrap_or_else(|_| fail("--points must be x,y;..."));
                (x, y)
            })
            .collect()
    } else {
        // 3x3 grid over the window.
        let w = i32::from(target.width.max(1));
        let h = i32::from(target.height.max(1));
        let mut pts = Vec::new();
        for ry in [1, 2, 3] {
            for rx in [1, 2, 3] {
                pts.push(((w * rx / 4) as i16, (h * ry / 4) as i16));
            }
        }
        pts
    };
    let mut ok = 0;
    let mut results: Vec<String> = Vec::new();
    for (x, y) in &points {
        let m = canary.send_motion(target.id, *x, *y);
        let c = if m.is_ok() {
            canary
                .send_button(target.id, *x, *y, true)
                .and_then(|()| canary.send_button(target.id, *x, *y, false))
        } else {
            Err("motion failed".to_owned())
        };
        let pass = c.is_ok();
        if pass {
            ok += 1;
        } else if let Err(e) = c {
            eprintln!("CANARY_SR15 point x={x} y={y} deliver_failed: {e}");
        }
        results.push(format!("{{\"x\":{x},\"y\":{y},\"ok\":{pass}}}"));
    }
    if has(args, "--json") {
        println!(
            "{{\"scenario\":\"SR15\",\"window\":{},\"points\":{},\"delivered\":{},\"matrix\":[{}]}}",
            target.id,
            points.len(),
            ok,
            results.join(",")
        );
    } else {
        println!(
            "CANARY_SR15 window={} points={} delivered={}",
            target.id,
            points.len(),
            ok
        );
    }
    if ok == 0 {
        std::process::exit(1);
    }
}
