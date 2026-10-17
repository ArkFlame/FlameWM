//! S1-S4 snap/border/extents probes on a synthetic client: physical output
//! corner quarters, snapped border width, per-mode `_NET_FRAME_EXTENTS`
//! relations, and fixed-hints (`min == max`) client stability. Pointer input
//! is XTEST only; the client is a canary-owned top-level with WM_NORMAL_HINTS.

use std::time::{Duration, Instant};

use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageData, ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _,
    CreateWindowAux, EventMask, PropMode, WindowClass,
};

use super::interaction_regressions::root_work_area;
use super::window_interaction::input_surface;
use super::{xtest_button_press, xtest_button_release, xtest_motion};

const PATH: &str = "path=real-pointer";
const CLIENT_NAME: &str = "snap-border-canary";
/// Canonical frame layout (mirrors window_interaction): 6px resize strip,
/// 31px titlebar, 114px controls.
const RESIZE_T: i32 = 6;
const TITLEBAR_H: i32 = 31;
const CONTROLS_W: i32 = 114;
const DRAG_STEPS: i32 = 8;
const FIXED: (u32, u32) = (275, 116);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Frame {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    bw: u32,
}

impl Frame {
    /// Outer rectangle as the X server lays it out: interior plus both borders.
    fn outer(self) -> (i32, i32, u32, u32) {
        (self.x, self.y, self.w + 2 * self.bw, self.h + 2 * self.bw)
    }
}

#[derive(Clone, Copy)]
enum Corner {
    Tl,
    Tr,
    Bl,
    Br,
}

/// Quarter of a work area: left/top take the floor half, right/bottom the remainder.
fn quarter(area: (i32, i32, u32, u32), corner: Corner) -> (i32, i32, u32, u32) {
    let (ax, ay, aw, ah) = area;
    let lw = aw / 2;
    let rw = aw - lw;
    let th = ah / 2;
    let bh = ah - th;
    match corner {
        Corner::Tl => (ax, ay, lw, th),
        Corner::Tr => (ax + lw as i32, ay, rw, th),
        Corner::Bl => (ax, ay + th as i32, lw, bh),
        Corner::Br => (ax + lw as i32, ay + th as i32, rw, bh),
    }
}

fn rect(r: (i32, i32, u32, u32)) -> String {
    format!("{}x{}+{}+{}", r.2, r.3, r.0, r.1)
}

fn frame_of(canary: &crate::Canary, client: u32) -> Option<Frame> {
    let info = canary.by_id(client)?;
    let frame = input_surface(canary, &info)?;
    let geom = canary.conn.get_geometry(frame.id).ok()?.reply().ok()?;
    Some(Frame {
        x: i32::from(geom.x),
        y: i32::from(geom.y),
        w: u32::from(geom.width),
        h: u32::from(geom.height),
        bw: u32::from(geom.border_width),
    })
}

fn client_size(canary: &crate::Canary, client: u32) -> Option<(u32, u32)> {
    let geom = canary.conn.get_geometry(client).ok()?.reply().ok()?;
    Some((u32::from(geom.width), u32::from(geom.height)))
}

fn frame_extents(canary: &crate::Canary, client: u32) -> Option<[u32; 4]> {
    let values = canary.prop_u32(client, "_NET_FRAME_EXTENTS");
    <[u32; 4]>::try_from(values.get(..4)?).ok()
}

/// Poll `read` until `ready` holds or the deadline passes; returns the last read.
fn settle<T>(
    timeout_ms: u64,
    mut read: impl FnMut() -> Option<T>,
    ready: impl Fn(&T) -> bool,
) -> Option<T> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let value = read();
        if value.as_ref().is_some_and(|v| ready(v)) || Instant::now() >= deadline {
            return value;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// Frame titlebar point (canonical layout) in root coordinates.
fn title_point(frame: Frame) -> (i16, i16) {
    let title_w = frame.w as i32 - 2 * RESIZE_T - CONTROLS_W;
    let x = frame.x + RESIZE_T + title_w / 2;
    let y = frame.y + RESIZE_T + (TITLEBAR_H - RESIZE_T) / 2;
    (x as i16, y as i16)
}

/// Real XTEST drag: move to `from`, press, hold `pre_hold_ms`, move in steps to
/// `to`, hold `dwell_ms` at `to`, then release.
fn drag(
    canary: &crate::Canary,
    from: (i16, i16),
    to: (i16, i16),
    pre_hold_ms: u64,
    dwell_ms: u64,
) -> bool {
    if !xtest_motion(canary, from.0, from.1) || !xtest_button_press(canary, from.0, from.1, 1) {
        return false;
    }
    sleep_ms(pre_hold_ms);
    for i in 1..=DRAG_STEPS {
        let lerp =
            |a: i16, b: i16| (i32::from(a) + (i32::from(b) - i32::from(a)) * i / DRAG_STEPS) as i16;
        if !xtest_motion(canary, lerp(from.0, to.0), lerp(from.1, to.1)) {
            return false;
        }
        sleep_ms(15);
    }
    sleep_ms(dwell_ms);
    xtest_button_release(canary, to.0, to.1, 1)
}

pub(super) fn send_root_message(
    canary: &crate::Canary,
    window: u32,
    type_name: &str,
    data: [u32; 5],
) -> bool {
    let Some(type_) = canary.intern(type_name) else {
        return false;
    };
    let ev = ClientMessageEvent {
        response_type: 33,
        format: 32,
        sequence: 0,
        window,
        type_,
        data: ClientMessageData::from(data),
    };
    canary
        .conn
        .send_event(
            false,
            canary.root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            ev,
        )
        .is_ok_and(|cookie| cookie.check().is_ok())
        && canary.conn.flush().is_ok()
}

/// Create and map a top-level client, optionally with fixed WM_NORMAL_HINTS
/// (`PMinSize|PMaxSize` with min == max). Returns the client window id.
fn create_client(
    canary: &crate::Canary,
    at: (i16, i16),
    size: (u16, u16),
    fixed: Option<(u32, u32)>,
) -> Result<u32, String> {
    let wid = canary.conn.generate_id().map_err(|e| e.to_string())?;
    canary
        .conn
        .create_window(
            0,
            wid,
            canary.root,
            at.0,
            at.1,
            size.0,
            size.1,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new(),
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
    if let Some((w, h)) = fixed {
        // WM_SIZE_HINTS: flags, x, y, w, h, min_w, min_h, max_w, max_h, ... (18 words).
        const PMIN_SIZE: u32 = 1 << 4;
        const PMAX_SIZE: u32 = 1 << 5;
        let mut words = [0u32; 18];
        words[0] = PMIN_SIZE | PMAX_SIZE;
        words[5] = w;
        words[6] = h;
        words[7] = w;
        words[8] = h;
        let bytes: Vec<u8> = words.iter().flat_map(|v| v.to_ne_bytes()).collect();
        canary
            .conn
            .change_property(
                PropMode::REPLACE,
                wid,
                AtomEnum::WM_NORMAL_HINTS,
                AtomEnum::WM_SIZE_HINTS,
                32,
                words.len() as u32,
                &bytes,
            )
            .map_err(|e| e.to_string())?
            .check()
            .map_err(|e| e.to_string())?;
    }
    canary
        .conn
        .map_window(wid)
        .map_err(|e| e.to_string())?
        .check()
        .map_err(|e| e.to_string())?;
    canary.conn.flush().map_err(|e| e.to_string())?;
    Ok(wid)
}

/// Wait until the WM has reparented the client into a viewable frame.
fn wait_managed(canary: &crate::Canary, client: u32) -> Option<Frame> {
    settle(3_000, || frame_of(canary, client), |_| true)
}

/// Wait for the frame to reach the exact `expected` outer rectangle (2 s budget).
fn settle_outer(canary: &crate::Canary, client: u32, expected: (i32, i32, u32, u32)) -> Option<Frame> {
    settle(2_000, || frame_of(canary, client), |f| f.outer() == expected)
}

/// `_NET_FRAME_EXTENTS` with no left, right or top border (snapped and maximized).
fn no_edge_extents(e: &[u32; 4]) -> bool {
    e[0] == 0 && e[1] == 0 && e[3] == 0
}

/// `_NET_WM_STATE` client message data toggling maximize (`action` 1 add, 0 remove).
pub(super) fn maximize_msg(canary: &crate::Canary, action: u32) -> [u32; 5] {
    [
        action,
        canary.intern("_NET_WM_STATE_MAXIMIZED_HORZ").unwrap_or(0),
        canary.intern("_NET_WM_STATE_MAXIMIZED_VERT").unwrap_or(0),
        1,
        0,
    ]
}

fn destroy_client(canary: &crate::Canary, client: u32) {
    let _ = canary.conn.destroy_window(client);
    let _ = canary.conn.flush();
}

fn new_client(canary: &crate::Canary) -> Result<(u32, Frame), String> {
    let client = create_client(canary, (100, 100), (320, 200), None)?;
    match wait_managed(canary, client) {
        Some(frame) => Ok((client, frame)),
        None => {
            destroy_client(canary, client);
            Err(format!("client {client} not managed"))
        }
    }
}

pub(super) fn fail(message: impl Into<String>) -> (bool, String) {
    (false, format!("observed=false {} {PATH}", message.into()))
}

/// Snap a fresh client to a physical corner and report the observed quarter.
fn snap_case(
    canary: &crate::Canary,
    area: (i32, i32, u32, u32),
    point: (i32, i32),
    corner: Corner,
) -> (bool, String) {
    let (client, frame) = match new_client(canary) {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    let to = (point.0 as i16, point.1 as i16);
    let dragged = drag(canary, title_point(frame), to, 0, 400);
    let expected = quarter(area, corner);
    let observed = settle_outer(canary, client, expected);
    destroy_client(canary, client);
    let Some(observed) = observed else {
        return fail("frame vanished");
    };
    let pass = dragged && observed.outer() == expected;
    (
        pass,
        format!(
            "point={},{} observed={} expected={} dragged={dragged} {PATH}",
            point.0,
            point.1,
            rect(observed.outer()),
            rect(expected)
        ),
    )
}

/// S1: physical output corners snap to the matching work-area quarter.
pub fn run_s1(canary: &crate::Canary, _args: &[String]) -> (bool, String) {
    let Some(area) = root_work_area(canary) else {
        return fail("work area unavailable");
    };
    let (sw, sh) = canary.root_geometry();
    let (sw, sh) = (i32::from(sw) - 1, i32::from(sh) - 1);
    let cases = [
        ("tl", (0, 0), Corner::Tl),
        ("tr", (sw, 0), Corner::Tr),
        ("bl", (0, sh), Corner::Bl),
        ("br", (sw, sh), Corner::Br),
    ];
    let mut pass = true;
    let mut parts = Vec::new();
    for (name, point, corner) in cases {
        let (ok, detail) = snap_case(canary, area, point, corner);
        pass &= ok;
        parts.push(format!("{name}[pass={ok} {detail}]"));
    }
    (
        pass,
        format!(
            "S1 physical_corner_quarters work_area={} {}",
            rect(area),
            parts.join(" ")
        ),
    )
}

/// S2: a TL snap clears the frame border; dragging out restores the configured border (1).
pub fn run_s2(canary: &crate::Canary, _args: &[String]) -> (bool, String) {
    let Some(area) = root_work_area(canary) else {
        return fail("work area unavailable");
    };
    let (client, floating) = match new_client(canary) {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    let expected = quarter(area, Corner::Tl);
    let dragged_in = drag(canary, title_point(floating), (0, 0), 0, 400);
    let snapped = settle_outer(canary, client, expected);
    let snapped_bw = snapped.map(|f| f.bw);
    let (cx, cy) = (
        area.0 + area.2 as i32 / 2 + 40,
        area.1 + area.3 as i32 / 2 + 40,
    );
    let dragged_out =
        snapped.is_some_and(|s| drag(canary, title_point(s), (cx as i16, cy as i16), 0, 400));
    let restored = settle(2_000, || frame_of(canary, client), |f| f.bw == 1);
    let restored_bw = restored.map(|f| f.bw);
    destroy_client(canary, client);
    let pass = dragged_in && dragged_out && snapped_bw == Some(0) && restored_bw == Some(1);
    (
        pass,
        format!(
            "S2 snapped_bw={snapped_bw:?} expected_snapped_bw=0 restored_bw={restored_bw:?} expected_restored_bw=1 snapped_outer={} dragged_in={dragged_in} dragged_out={dragged_out} {PATH}",
            snapped.map_or_else(|| "none".to_owned(), |f| rect(f.outer()))
        ),
    )
}

/// S3: `_NET_FRAME_EXTENTS` per placement mode. Floating left/right/bottom equal
/// the configured border and top is the titlebar; snapped and maximized have
/// left/right/bottom == 0; floating values return after unsnap/unmaximize.
pub fn run_s3(canary: &crate::Canary, _args: &[String]) -> (bool, String) {
    let Some(area) = root_work_area(canary) else {
        return fail("work area unavailable");
    };
    let (client, floating_frame) = match new_client(canary) {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    let ext = |c: u32| settle(2_000, || frame_extents(canary, c), |e| e[2] > 0);
    let Some(floating) = ext(client) else {
        destroy_client(canary, client);
        return fail("_NET_FRAME_EXTENTS missing at start");
    };
    let floating_ok = floating[0] == floating[1]
        && floating[1] == floating[3]
        && floating[0] > 0
        && floating[2] > floating[3];

    let expected = quarter(area, Corner::Tl);
    let dragged_in = drag(canary, title_point(floating_frame), (0, 0), 0, 400);
    let _ = settle_outer(canary, client, expected);
    let snapped = settle(2_000, || frame_extents(canary, client), no_edge_extents);
    let snapped_ok = snapped.as_ref().is_some_and(no_edge_extents);

    let snapped_frame = frame_of(canary, client);
    let cx = (area.0 + area.2 as i32 / 2 + 40) as i16;
    let cy = (area.1 + area.3 as i32 / 2 + 40) as i16;
    let dragged_out = snapped_frame.is_some_and(|f| drag(canary, title_point(f), (cx, cy), 0, 400));
    let restored_snap = settle(2_000, || frame_extents(canary, client), |e| *e == floating);
    let snap_restored = restored_snap == Some(floating);

    let maximized_sent = send_root_message(
        canary,
        client,
        "_NET_WM_STATE",
        maximize_msg(canary, 1),
    );
    let maximized = settle(2_000, || frame_extents(canary, client), no_edge_extents);
    let maximized_ok = maximized_sent && maximized.as_ref().is_some_and(no_edge_extents);
    let unmaximized_sent = send_root_message(
        canary,
        client,
        "_NET_WM_STATE",
        maximize_msg(canary, 0),
    );
    let restored_max = settle(2_000, || frame_extents(canary, client), |e| *e == floating);
    let max_restored = unmaximized_sent && restored_max == Some(floating);
    destroy_client(canary, client);

    let pass = floating_ok
        && dragged_in
        && snapped_ok
        && dragged_out
        && snap_restored
        && maximized_ok
        && max_restored;
    (
        pass,
        format!(
            "S3 floating={floating:?} floating_ok={floating_ok} snapped={snapped:?} snapped_ok={snapped_ok} restored_after_unsnap={snap_restored} maximized={maximized:?} maximized_ok={maximized_ok} restored_after_unmax={max_restored} {PATH}"
        ),
    )
}

/// S4: fixed min==max hints keep the client at 275x116 through ConfigureWindow,
/// a titlebar drag, and a `_NET_WM_MOVERESIZE` move sampled for 1s.
pub fn run_s4(canary: &crate::Canary, _args: &[String]) -> (bool, String) {
    let client = match create_client(
        canary,
        (100, 100),
        (FIXED.0 as u16, FIXED.1 as u16),
        Some(FIXED),
    ) {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    let Some(frame0) = wait_managed(canary, client) else {
        destroy_client(canary, client);
        return fail("fixed client not managed");
    };
    let _ = canary
        .conn
        .configure_window(client, &ConfigureWindowAux::new().width(100).height(50));
    let _ = canary.conn.flush();
    let size_configure = settle(1_000, || client_size(canary, client), |s| *s == FIXED);
    let size_configure = size_configure.unwrap_or((0, 0));

    let frame0 = frame_of(canary, client).unwrap_or(frame0);
    let from = title_point(frame0);
    let to = (from.0 + 40, from.1 + 40);
    let dragged = drag(canary, from, to, 600, 0);
    let moved = settle(
        2_000,
        || frame_of(canary, client),
        |f| f.x == frame0.x + 40 && f.y == frame0.y + 40,
    );
    let size_drag = client_size(canary, client).unwrap_or((0, 0));
    let origin_delta = moved.map(|f| (f.x - frame0.x, f.y - frame0.y));

    let frame1 = frame_of(canary, client).unwrap_or(frame0);
    let move_from = title_point(frame1);
    let press = xtest_motion(canary, move_from.0, move_from.1)
        && xtest_button_press(canary, move_from.0, move_from.1, 1);
    let move_sent = send_root_message(
        canary,
        client,
        "_NET_WM_MOVERESIZE",
        [
            u32::from(move_from.0 as u16),
            u32::from(move_from.1 as u16),
            8,
            1,
            1,
        ],
    );
    let mut samples = Vec::with_capacity(20);
    for _ in 0..20 {
        sleep_ms(50);
        samples.push(client_size(canary, client).unwrap_or((0, 0)));
    }
    let released = xtest_button_release(canary, move_from.0, move_from.1, 1);
    destroy_client(canary, client);

    let samples_stable = samples.iter().all(|s| *s == FIXED);
    let pass = size_configure == FIXED
        && dragged
        && size_drag == FIXED
        && origin_delta == Some((40, 40))
        && press
        && move_sent
        && released
        && samples_stable;
    let min = samples.iter().min().copied().unwrap_or((0, 0));
    let max = samples.iter().max().copied().unwrap_or((0, 0));
    (
        pass,
        format!(
            "S4 expected={}x{} size_after_configure={}x{} size_after_drag={}x{} origin_delta={origin_delta:?} expected_delta=(40, 40) moveresize_sent={move_sent} samples={} sample_min={}x{} sample_max={}x{} samples_stable={samples_stable} {PATH}",
            FIXED.0,
            FIXED.1,
            size_configure.0,
            size_configure.1,
            size_drag.0,
            size_drag.1,
            samples.len(),
            min.0,
            min.1,
            max.0,
            max.1
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::{Corner, quarter};

    #[test]
    fn quarter_odd_area_gives_floor_half_to_left_and_top() {
        let area = (0, 0, 1279, 799);
        assert_eq!(quarter(area, Corner::Tl), (0, 0, 639, 399));
        assert_eq!(quarter(area, Corner::Tr), (639, 0, 640, 399));
        assert_eq!(quarter(area, Corner::Bl), (0, 399, 639, 400));
        assert_eq!(quarter(area, Corner::Br), (639, 399, 640, 400));
    }

    #[test]
    fn quarter_offset_area_keeps_origin() {
        let area = (10, 20, 1280, 600);
        assert_eq!(quarter(area, Corner::Br), (650, 320, 640, 300));
    }
}
