//! Explicit-only layout probe (`explicit-window-overflow`, job V7 A14): a 600x400
//! client is dragged by its titlebar with real XTEST input far left, right, up and
//! down. Each reached position must stay at least 1/4 visible on both axes. The
//! window is then recentred, snapped to the top-left corner and maximized to check
//! that snap and maximize still use the work area. Screenshots and the case list
//! go to --shot-dir; tools/flamewm_layout_probe.py samples the taskbar pixels.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{
    AtomEnum, ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, PropMode, WindowClass,
};

use super::interaction_regressions::root_work_area;
use super::snap_border::{fail, maximize_msg, send_root_message};
use super::window_interaction::input_surface;
use super::xtest_drag_path;

const PATH: &str = "path=real-pointer";
const CLIENT_NAME: &str = "overflow-canary";
const CLIENT_BG: u32 = 0x0000_ff00;
const CLIENT_AT: (i16, i16) = (100, 100);
const CLIENT_SIZE: (u16, u16) = (600, 400);
/// Canonical frame layout (mirrors snap_border): 6px resize strip, 31px titlebar, 114px controls.
const RESIZE_T: i32 = 6;
const TITLEBAR_H: i32 = 31;
const CONTROLS_W: i32 = 114;
const DRAG_STEPS: u32 = 8;
/// Pointer margins: grabs and releases stay off the screen edges (edge snap zones), and the
/// grab band stays clear of the window controls (right side of the titlebar).
const MARGIN_X: i32 = 40;
const MARGIN_Y: i32 = 40;
const GRAB_LEFT: i32 = 20;
const GRAB_RIGHT_RESERVE: i32 = 150;
const GRAB_TOP: i32 = 10;
const GRAB_BOTTOM: i32 = 26;
const MAX_DRAGS: u32 = 60;
const PANEL: &str = "FlameWM panel";

/// Outer frame rectangle (x, y, w, h) as laid out by the X server.
type Rect = (i32, i32, u32, u32);

fn rect(r: Rect) -> String {
    format!("{}x{}+{}+{}", r.2, r.3, r.0, r.1)
}

/// Minimum visible extent on one axis for an outer frame extent `extent` (WM keeps ceil(extent / 4)).
fn min_visible(extent: i32) -> i32 {
    (extent.max(0) + 3) / 4
}

fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// Outer frame rect. X `geometry.x/y` is the inner origin; the outer edge is `x - bw`.
fn frame_of(canary: &crate::Canary, client: u32) -> Option<Rect> {
    let info = canary.by_id(client)?;
    let frame = input_surface(canary, &info)?;
    let geom = canary.conn.get_geometry(frame.id).ok()?.reply().ok()?;
    let bw = u32::from(geom.border_width);
    let b = bw as i32;
    Some((
        i32::from(geom.x) - b,
        i32::from(geom.y) - b,
        u32::from(geom.width) + 2 * bw,
        u32::from(geom.height) + 2 * bw,
    ))
}

/// Poll for the managed frame of a new client (about 2 s).
fn wait_frame(canary: &crate::Canary, client: u32) -> Option<Rect> {
    (0..50).find_map(|_| {
        sleep_ms(40);
        frame_of(canary, client)
    })
}

/// Shot directory from `--shot-dir`, created on demand.
fn shot_dir(args: &[String]) -> Result<std::path::PathBuf, (bool, String)> {
    let Some(dir) = crate::arg(args, "--shot-dir") else {
        return Err(fail("--shot-dir <path> required"));
    };
    let dir = std::path::PathBuf::from(dir);
    fs::create_dir_all(&dir).map_err(|e| fail(format!("shot dir: {e}")))?;
    Ok(dir)
}

/// Poll until the frame equals `expected` or the deadline passes.
fn settle_frame(canary: &crate::Canary, client: u32, expected: Rect) -> Option<Rect> {
    let deadline = Instant::now() + Duration::from_millis(2_000);
    loop {
        let value = frame_of(canary, client);
        if value == Some(expected) || Instant::now() >= deadline {
            return value;
        }
        sleep_ms(25);
    }
}

/// Canonical titlebar point of the frame, in root coordinates.
fn title_point(f: Rect) -> (i16, i16) {
    let x = f.0 + RESIZE_T + (f.2 as i32 - 2 * RESIZE_T - CONTROLS_W) / 2;
    let y = f.1 + RESIZE_T + (TITLEBAR_H - RESIZE_T) / 2;
    (x as i16, y as i16)
}

/// Pick a titlebar grab point on screen that moves the frame toward `target` as far as
/// the pointer allows, and the pointer destination. None when no titlebar point is on screen.
fn drag_step(f: Rect, screen: (i32, i32), target: (i32, i32)) -> Option<((i16, i16), (i16, i16))> {
    let (sw, sh) = screen;
    let (dx, dy) = (target.0 - f.0, target.1 - f.1);
    let xlo = (f.0 + GRAB_LEFT).max(MARGIN_X);
    let xhi = (f.0 + f.2 as i32 - GRAB_RIGHT_RESERVE).min(sw - 1 - MARGIN_X);
    let ylo = (f.1 + GRAB_TOP).max(MARGIN_Y);
    let yhi = (f.1 + GRAB_BOTTOM).min(sh - 1 - MARGIN_Y);
    if xlo > xhi || ylo > yhi {
        return None;
    }
    let px = match dx.signum() {
        -1 => xhi,
        1 => xlo,
        _ => (xlo + xhi) / 2,
    };
    let py = match dy.signum() {
        -1 => yhi,
        1 => ylo,
        _ => (ylo + yhi) / 2,
    };
    let mx = dx.clamp(MARGIN_X - px, sw - 1 - MARGIN_X - px);
    let my = dy.clamp(MARGIN_Y - py, sh - 1 - MARGIN_Y - py);
    Some(((px as i16, py as i16), ((px + mx) as i16, (py + my) as i16)))
}

/// Repeated real titlebar drags toward `target`; stops when the WM stops moving the frame.
fn move_frame(canary: &crate::Canary, client: u32, target: (i32, i32)) -> Option<Rect> {
    let mut f = frame_of(canary, client)?;
    let screen = canary.root_geometry();
    let screen = (i32::from(screen.0), i32::from(screen.1));
    let mut stuck = 0;
    for _ in 0..MAX_DRAGS {
        if (f.0, f.1) == target {
            break;
        }
        let Some((from, to)) = drag_step(f, screen, target) else {
            break;
        };
        if !xtest_drag_path(canary, from, to, 1, DRAG_STEPS) {
            break;
        }
        sleep_ms(120);
        let next = frame_of(canary, client)?;
        stuck = if (next.0, next.1) == (f.0, f.1) {
            stuck + 1
        } else {
            0
        };
        f = next;
        if stuck >= 2 {
            break;
        }
    }
    Some(f)
}

fn screenshot(dir: &Path, name: &str) -> Result<String, String> {
    let out = dir.join(name);
    let status = Command::new("import")
        .args(["-window", "root"])
        .arg(&out)
        .status()
        .map_err(|e| format!("import unavailable: {e}"))?;
    if status.success() {
        Ok(out.display().to_string())
    } else {
        Err(format!("import failed for {name}"))
    }
}

fn create_client(canary: &crate::Canary) -> Result<u32, String> {
    let wid = canary.conn.generate_id().map_err(|e| e.to_string())?;
    canary
        .conn
        .create_window(
            0,
            wid,
            canary.root,
            CLIENT_AT.0,
            CLIENT_AT.1,
            CLIENT_SIZE.0,
            CLIENT_SIZE.1,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new().background_pixel(CLIENT_BG),
        )
        .map_err(|e| e.to_string())?
        .check()
        .map_err(|e| e.to_string())?;
    canary
        .conn
        .change_property(
            PropMode::REPLACE,
            wid,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            8,
            CLIENT_NAME.len() as u32,
            CLIENT_NAME.as_bytes(),
        )
        .map_err(|e| e.to_string())?
        .check()
        .map_err(|e| e.to_string())?;
    canary
        .conn
        .map_window(wid)
        .map_err(|e| e.to_string())?
        .check()
        .map_err(|e| e.to_string())?;
    canary.conn.flush().map_err(|e| e.to_string())?;
    Ok(wid)
}

/// Visible span of `[start, start+len)` clipped to `[0, limit)`.
fn visible(start: i32, len: u32, limit: i32) -> i32 {
    (start + len as i32).min(limit) - start.max(0)
}

struct Case {
    name: &'static str,
    frame: Rect,
    outside: bool,
    quarter_ok: bool,
    shot: String,
}

/// B16 explicit-window-overflow (A14): overflow limits, taskbar stacking, snap and maximize.
pub fn run_overflow(canary: &crate::Canary, args: &[String]) -> (bool, String) {
    let dir = match shot_dir(args) {
        Ok(dir) => dir,
        Err(result) => return result,
    };
    let screen = canary.root_geometry();
    let (sw, sh) = (i32::from(screen.0), i32::from(screen.1));
    let Some(panel) = canary.find(PANEL).into_iter().find(|w| w.map_state == 2) else {
        return fail("FlameWM panel not viewable");
    };
    // The WM publishes _NET_WORKAREA after the panel strut lands; wait for the reserve.
    let deadline = Instant::now() + Duration::from_millis(5_000);
    let area = loop {
        let area = root_work_area(canary);
        let reserved = area.is_some_and(|(_, y, _, h)| y + h as i32 <= i32::from(panel.y));
        if reserved || Instant::now() >= deadline {
            break area;
        }
        sleep_ms(50);
    };
    let Some(area) = area else {
        return fail("work area unavailable");
    };
    let client = match create_client(canary) {
        Ok(w) => w,
        Err(e) => return fail(e),
    };
    let Some(start) = wait_frame(canary, client) else {
        return fail("client not managed");
    };
    let (cx, cy) = (i32::from(CLIENT_AT.0), i32::from(CLIENT_AT.1));
    let mut cases: Vec<Case> = Vec::new();
    let mut pass = true;
    let mut parts = Vec::new();

    // Each direction: drag as far as the WM allows, then recentre.
    let directions: [(&str, (i32, i32)); 4] = [
        ("left", (-600, cy)),
        ("right", (sw + 600, cy)),
        ("up", (cx, -600)),
        ("down", (cx, sh + 600)),
    ];
    for (name, target) in directions {
        let Some(f) = move_frame(canary, client, target) else {
            pass = false;
            parts.push(format!("{name}[pass=false frame-missing]"));
            break;
        };
        let (fw, fh) = (f.2 as i32, f.3 as i32);
        let ((lo_x, hi_x), (lo_y, hi_y)) = keep_visible_bounds(f, (sw, sh));
        let outside = match name {
            "left" => f.0 < 0,
            "right" => f.0 + fw > sw,
            "up" => f.1 < 0,
            _ => f.1 + fh > sh,
        };
        let vis_w = visible(f.0, f.2, sw);
        let vis_h = visible(f.1, f.3, sh);
        let quarter_ok = vis_w >= min_visible(fw) && vis_h >= min_visible(fh);
        let at_bound = match name {
            "left" => f.0 == lo_x,
            "right" => f.0 == hi_x,
            "up" => f.1 == lo_y,
            _ => f.1 == hi_y,
        };
        let shot = screenshot(&dir, &format!("overflow-{name}.png")).unwrap_or_default();
        // at_bound is reported, not gated: a titlebar grab reaches at most 40 - (602 - 150) = -412
        // on the left (pointer margin + controls reserve), short of the -451 bound; the exact
        // bound is asserted by explicit-window-clamp via ConfigureRequest.
        let ok = outside && quarter_ok;
        // Up is NOT-RUN: a titlebar grab cannot lift the frame above y=0 by more than a few px
        // without the pointer entering the top-edge zone (a drop there maximizes the window).
        let status = if name == "up" {
            "NOT-RUN"
        } else {
            pass &= ok;
            if ok { "PASS" } else { "FAIL" }
        };
        parts.push(format!(
            "{name}[{status} frame={} outside={outside} at_bound={at_bound} visible_w={vis_w}/{fw} visible_h={vis_h}/{fh} min_visible_w={} min_visible_h={}]",
            rect(f),
            min_visible(fw),
            min_visible(fh)
        ));
        cases.push(Case {
            name,
            frame: f,
            outside,
            quarter_ok,
            shot,
        });
        let _ = move_frame(canary, client, (cx, cy));
    }

    // Recentred: snap to the top-left corner must land on the work-area quarter.
    let mut snap_ok = false;
    let mut snap_detail = String::from("snap=skipped");
    if let Some(f) = frame_of(canary, client) {
        let dragged = xtest_drag_path(canary, title_point(f), (0, 0), 1, DRAG_STEPS);
        let (ax, ay, aw, ah) = area;
        let expected = (ax, ay, aw / 2, ah / 2);
        let observed = settle_frame(canary, client, expected);
        snap_ok = dragged && observed == Some(expected);
        snap_detail = format!(
            "snap[pass={snap_ok} observed={} expected={} dragged={dragged}]",
            observed.map(rect).unwrap_or_default(),
            rect(expected)
        );
        let _ = screenshot(&dir, "overflow-snap-tl.png");
    }
    pass &= snap_ok;
    parts.push(snap_detail);

    // Maximize: frame must cover the work area exactly.
    let sent = send_root_message(canary, client, "_NET_WM_STATE", maximize_msg(canary, 1));
    let max_observed = settle_frame(canary, client, area);
    let max_ok = sent && max_observed == Some(area);
    pass &= max_ok;
    parts.push(format!(
        "maximize[pass={max_ok} observed={} expected={}]",
        max_observed.map(rect).unwrap_or_default(),
        rect(area)
    ));
    let _ = screenshot(&dir, "overflow-maximized.png");
    let _ = send_root_message(canary, client, "_NET_WM_STATE", maximize_msg(canary, 0));
    let _ = canary.conn.destroy_window(client);
    let _ = canary.conn.flush();

    // Machine-readable cases for the Python taskbar pixel probe.
    let mut json = format!(
        "{{\"screen\":[{sw},{sh}],\"panel\":[{},{},{},{}],\"cases\":[",
        panel.x, panel.y, panel.width, panel.height
    );
    let items: Vec<String> = cases
        .iter()
        .map(|c| {
            format!(
                "{{\"name\":\"{}\",\"frame\":[{},{},{},{}],\"outside\":{},\"quarter_ok\":{},\"shot\":{:?}}}",
                c.name, c.frame.0, c.frame.1, c.frame.2, c.frame.3, c.outside, c.quarter_ok, c.shot
            )
        })
        .collect();
    json.push_str(&items.join(","));
    json.push_str("]}\n");
    let _ = fs::write(dir.join("overflow-cases.json"), json);

    (
        pass,
        format!("start={} {} {PATH}", rect(start), parts.join(" ")),
    )
}

/// Keep-visible bounds of one axis on OUTER extent `len`: start in `[lo - (len - q), hi - q]`, `q = min_visible(len)`.
fn clamp_bounds(len: u32, lo: i32, hi: i32) -> (i32, i32) {
    let len = len as i32;
    let q = min_visible(len);
    (lo - (len - q), hi - q)
}

/// Keep-visible `((lo_x, hi_x), (lo_y, hi_y))` for an OUTER frame rect on a `screen` (w, h).
fn keep_visible_bounds(outer: Rect, screen: (i32, i32)) -> ((i32, i32), (i32, i32)) {
    (
        clamp_bounds(outer.2, 0, screen.0),
        clamp_bounds(outer.3, 0, screen.1),
    )
}

/// G2 explicit-window-clamp: the titlebar route cannot lift the frame above y=0, so the 3/4
/// overflow limit is driven by client ConfigureRequest (XMoveWindow to +-10000) in all four
/// directions. Each landing must equal the keep-visible bound exactly. The titlebar route
/// is measured (not asserted) with the pointer at the top margin and at the y=0 edge.
pub fn run_clamp(canary: &crate::Canary, args: &[String]) -> (bool, String) {
    let dir = match shot_dir(args) {
        Ok(dir) => dir,
        Err(result) => return result,
    };
    let screen = canary.root_geometry();
    let (sw, sh) = (i32::from(screen.0), i32::from(screen.1));
    let client = match create_client(canary) {
        Ok(w) => w,
        Err(e) => return fail(e),
    };
    let Some(start) = wait_frame(canary, client) else {
        return fail("client not managed");
    };
    let (cx, cy) = (i32::from(CLIENT_AT.0), i32::from(CLIENT_AT.1));
    let (fw, fh) = (start.2, start.3);
    let ((lo_x, hi_x), (lo_y, hi_y)) = keep_visible_bounds(start, (sw, sh));
    let far = 10_000;
    let cases = [
        ("left", (-far, cy), (lo_x, start.1)),
        ("right", (far, cy), (hi_x, start.1)),
        ("up", (cx, -far), (start.0, lo_y)),
        ("down", (cx, far), (start.0, hi_y)),
    ];
    let mut pass = true;
    let mut parts = vec![format!("start={}", rect(start))];
    let reset = |canary: &crate::Canary| {
        let aux = ConfigureWindowAux::new().x(cx).y(cy);
        let _ = canary.conn.configure_window(client, &aux);
        let _ = canary.conn.flush();
        settle_frame(canary, client, start)
    };
    for (name, (x, y), (ex, ey)) in cases {
        let expected = (ex, ey, fw, fh);
        let aux = ConfigureWindowAux::new().x(x).y(y);
        let sent = canary
            .conn
            .configure_window(client, &aux)
            .is_ok_and(|c| c.check().is_ok())
            && canary.conn.flush().is_ok();
        let observed = settle_frame(canary, client, expected);
        let f = observed.unwrap_or(start);
        let visible_ok = visible(f.0, f.2, sw) >= min_visible(fw as i32)
            && visible(f.1, f.3, sh) >= min_visible(fh as i32);
        let ok = sent && observed == Some(expected) && visible_ok;
        pass &= ok;
        let _ = screenshot(&dir, &format!("clamp-{name}.png"));
        parts.push(format!(
            "{name}[{} req=({x},{y}) frame={} expected={} visible_w={}/{fw} visible_h={}/{fh}]",
            if ok { "PASS" } else { "FAIL" },
            rect(f),
            rect(expected),
            visible(f.0, f.2, sw),
            visible(f.1, f.3, sh)
        ));
        let _ = reset(canary);
    }

    // Titlebar route, pointer held at the top margin: measured only.
    let up = move_frame(canary, client, (cx, -600)).unwrap_or(start);
    let _ = screenshot(&dir, "clamp-titlebar-up.png");
    parts.push(format!(
        "titlebar-up[frame={} top_outside={} visible_h={}/{fh}]",
        rect(up),
        up.1 < 0,
        visible(up.1, up.3, sh)
    ));
    let _ = reset(canary);

    // Titlebar route, pointer released on the y=0 edge: measured only (top-edge zone).
    if let Some(f) = frame_of(canary, client) {
        let grab = (title_point(f).0, (f.1 + GRAB_BOTTOM) as i16);
        let dropped = xtest_drag_path(canary, grab, (grab.0, 0), 1, DRAG_STEPS);
        sleep_ms(400);
        let after = frame_of(canary, client).unwrap_or(f);
        let _ = screenshot(&dir, "clamp-titlebar-edge-y0.png");
        parts.push(format!(
            "titlebar-edge-y0[dragged={dragged} frame={} resized={} top_outside={}]",
            rect(after),
            (after.2, after.3) != (fw, fh),
            after.1 < 0,
            dragged = dropped
        ));
    }

    let _ = canary.conn.destroy_window(client);
    let _ = canary.conn.flush();
    (pass, format!("{} {PATH}", parts.join(" ")))
}
