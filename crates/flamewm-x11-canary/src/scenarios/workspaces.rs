//! T11 max/snap/fullscreen, T12 workspace churn, and T21/T22 workspace
//! projection admission.

use std::time::{Duration, Instant};

use super::{center, real_click, xtest_button_press, xtest_button_release, xtest_motion};
use x11rb::CURRENT_TIME;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::protocol::xproto::{ClientMessageData, ClientMessageEvent, EventMask};

const WORKSPACE_CONVERGENCE_BUDGET_MS: u128 = 50;
const WORKSPACE_CONVERGENCE_SAMPLES: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StickyPixel {
    Yellow,
    Green,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StickySignature {
    yellow_probe: StickyPixel,
    green_probe: StickyPixel,
}

#[derive(Clone, Copy, Debug)]
struct ColorBounds {
    min_x: u16,
    min_y: u16,
    max_x: u16,
    max_y: u16,
}

fn current_workspace(canary: &crate::Canary) -> Option<u32> {
    canary
        .prop_u32(canary.root, "_NET_CURRENT_DESKTOP")
        .first()
        .copied()
}

/// Sticky colour of one pixel from its `r, g, b` channels.
fn classify(r: u8, g: u8, b: u8) -> StickyPixel {
    if r >= 220 && g >= 190 && g <= 250 && b <= 140 && r > g + 20 {
        StickyPixel::Yellow
    } else if r >= 160 && r <= 225 && g >= 200 && b >= 70 && b <= 190 && g > r + 15 {
        StickyPixel::Green
    } else {
        StickyPixel::Other
    }
}

fn color_bounds(data: &[u8], width: u16, height: u16, color: StickyPixel) -> Option<ColorBounds> {
    let mut bounds: Option<ColorBounds> = None;
    for (index, pixel) in data.chunks_exact(4).enumerate() {
        let x = index % usize::from(width.max(1));
        let y = index / usize::from(width.max(1));
        if y >= usize::from(height) {
            break;
        }
        if color == StickyPixel::Other || classify(pixel[2], pixel[1], pixel[0]) != color {
            continue;
        }
        let x = u16::try_from(x).unwrap_or(u16::MAX);
        let y = u16::try_from(y).unwrap_or(u16::MAX);
        bounds = Some(match bounds {
            Some(mut current) => {
                current.min_x = current.min_x.min(x);
                current.min_y = current.min_y.min(y);
                current.max_x = current.max_x.max(x);
                current.max_y = current.max_y.max(y);
                current
            }
            None => ColorBounds {
                min_x: x,
                min_y: y,
                max_x: x,
                max_y: y,
            },
        });
    }
    bounds
}

fn color_probe(bounds: ColorBounds) -> (i16, i16) {
    let x = u32::from(bounds.min_x)
        .saturating_add(u32::from(bounds.max_x.saturating_sub(bounds.min_x)) * 3 / 4);
    let y = u32::from(bounds.max_y.saturating_sub(20).max(bounds.min_y));
    (
        i16::try_from(x).unwrap_or(i16::MAX),
        i16::try_from(y).unwrap_or(i16::MAX),
    )
}

fn sticky_scan(canary: &crate::Canary, color: StickyPixel) -> Option<Option<ColorBounds>> {
    let (width, height) = canary.root_geometry();
    let data = canary
        .sample(canary.root, 0, 0, width.max(1), height.max(1))
        .ok()?;
    Some(color_bounds(&data, width, height, color))
}

fn sticky_probe(canary: &crate::Canary, color: StickyPixel) -> Option<(i16, i16)> {
    Some(color_probe(sticky_scan(canary, color)??))
}

fn sticky_color_present(canary: &crate::Canary, color: StickyPixel) -> Option<bool> {
    sticky_scan(canary, color).map(|bounds| bounds.is_some())
}

fn sticky_pixel(canary: &crate::Canary, point: (i16, i16)) -> StickyPixel {
    let Ok(data) = canary.sample(canary.root, point.0, point.1, 1, 1) else {
        return StickyPixel::Other;
    };
    let (_, _, r, g, b) = crate::Canary::pixel_stats(&data);
    classify(r, g, b)
}

fn sticky_signature(canary: &crate::Canary, probes: &[(i16, i16); 2]) -> StickySignature {
    StickySignature {
        yellow_probe: sticky_pixel(canary, probes[0]),
        green_probe: sticky_pixel(canary, probes[1]),
    }
}

fn expected_sticky_signature(workspace: u32) -> StickySignature {
    if workspace == 0 {
        StickySignature {
            yellow_probe: StickyPixel::Yellow,
            green_probe: StickyPixel::Other,
        }
    } else {
        StickySignature {
            yellow_probe: StickyPixel::Other,
            green_probe: StickyPixel::Green,
        }
    }
}

/// Centre of pager cell `target`, read from the `FLAMEWM_PAGER_CELLS`
/// property the shell publishes on the panel window (window-relative x, y,
/// width, height per cell, CARDINAL format 32). No fallback: a missing or
/// short property returns None and the click is refused.
fn pager_cell_center(canary: &crate::Canary, target: u32) -> Option<(i16, i16)> {
    let panel = canary
        .find("FlameWM panel")
        .into_iter()
        .find(|window| window.map_state == 2)?;
    let atom = canary.intern("FLAMEWM_PAGER_CELLS")?;
    let values: Vec<u32> = canary
        .prop_bytes(panel.id, atom)
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();
    let base = (target as usize).checked_mul(4)?;
    let cell = values.get(base..base + 4)?;
    let centre_x = i32::from(panel.x) + cell[0] as i32 + cell[2] as i32 / 2;
    let centre_y = i32::from(panel.y) + cell[1] as i32 + cell[3] as i32 / 2;
    Some((
        centre_x.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
        centre_y.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
    ))
}

fn real_workspace_click(canary: &crate::Canary, target: u32) -> bool {
    let Some(point) = pager_cell_center(canary, target) else {
        return false;
    };
    xtest_motion(canary, point.0, point.1)
        && xtest_button_press(canary, point.0, point.1, 1)
        && xtest_button_release(canary, point.0, point.1, 1)
        && canary.conn.flush().is_ok()
}

fn wait_workspace(canary: &crate::Canary, target: u32, timeout_ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if current_workspace(canary) == Some(target) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn restore_workspace(canary: &crate::Canary, original: u32) -> bool {
    let switched = if original <= 1 {
        real_workspace_click(canary, original)
    } else {
        ewmh_request(
            canary,
            canary.root,
            "_NET_CURRENT_DESKTOP",
            original,
            CURRENT_TIME,
        )
    };
    switched && wait_workspace(canary, original, 500) && current_workspace(canary) == Some(original)
}

/// T21/T22: real pager clicks must converge the sticky A/B projection without
/// a post-release pointer, key, or expose-generating interaction. The canary
/// observes WM authority through `_NET_CURRENT_DESKTOP` and presentation
/// through two seeded sticky-color pixels. Legacy control-fetch and filesystem
/// rescan counters belong to the desktop process, so this X11-only scenario
/// reports those deltas as an explicit external-profiler boundary.
pub fn run_t21_t22(canary: &crate::Canary, _args: &[String]) -> (bool, String) {
    let Some(original) = current_workspace(canary) else {
        return (
            false,
            "scenario=T21-T22 no _NET_CURRENT_DESKTOP path=real-pointer".to_owned(),
        );
    };
    let fail = |detail: String| {
        let _ = restore_workspace(canary, original);
        (false, detail)
    };
    let desktops = canary
        .prop_u32(canary.root, "_NET_NUMBER_OF_DESKTOPS")
        .first()
        .copied()
        .unwrap_or(1);
    if desktops < 2 {
        return (
            false,
            format!("scenario=T21-T22 desktops={desktops} need_at_least=2 path=real-pointer"),
        );
    }

    if !wait_workspace(canary, original, 250) {
        return (
            false,
            format!("scenario=T21-T22 original_workspace={original} not_observed_before_setup"),
        );
    }
    if original != 0 && (!real_workspace_click(canary, 0) || !wait_workspace(canary, 0, 500)) {
        return fail(format!(
            "scenario=T21-T22 setup_workspace=0 failed original_workspace={original}"
        ));
    }
    let Some(yellow_probe) = sticky_probe(canary, StickyPixel::Yellow) else {
        return fail("scenario=T21-T22 baseline_A_not_observable_on_workspace_0".to_owned());
    };

    let baseline_green = sticky_color_present(canary, StickyPixel::Green);
    if baseline_green != Some(false) || sticky_pixel(canary, yellow_probe) != StickyPixel::Yellow {
        return fail(format!(
            "scenario=T21-T22 baseline_invalid workspace=0 A={:?} B_present={baseline_green:?} expected=A-visible/B-absent",
            sticky_pixel(canary, yellow_probe),
        ));
    }

    // The seeded fixture places A on workspace 0 and B on workspace 1. Visit
    // workspace 1 once to discover B's probe, then return before measuring the
    // required workspace-0 baseline and the 100 alternating switches.
    {
        let clicked = real_workspace_click(canary, 1);
        let converged = clicked && wait_workspace(canary, 1, 500);
        if !clicked || !converged {
            let current = current_workspace(canary);
            return fail(format!(
                "scenario=T21-T22 setup_workspace=1 failed while_discovering_B clicked={clicked} converged={converged} current_workspace={current:?} target_workspace=1 path=real-pointer"
            ));
        }
    }
    let Some(green_probe) = sticky_probe(canary, StickyPixel::Green) else {
        return fail("scenario=T21-T22 switched_B_not_observable_on_workspace_1".to_owned());
    };
    let switched_yellow = sticky_color_present(canary, StickyPixel::Yellow);
    let switched = sticky_signature(canary, &[yellow_probe, green_probe]);
    if switched != expected_sticky_signature(1) || switched_yellow != Some(false) {
        return fail(format!(
            "scenario=T21-T22 switched_invalid workspace=1 A_present={switched_yellow:?} B={:?} expected=A-absent/B-visible",
            switched.green_probe
        ));
    }
    if !real_workspace_click(canary, 0) || !wait_workspace(canary, 0, 500) {
        return fail("scenario=T21-T22 setup_workspace=0 failed after_discovering_B".to_owned());
    }
    let probes = [yellow_probe, green_probe];
    let mut before = sticky_signature(canary, &probes);
    if before != expected_sticky_signature(0) {
        return fail(format!(
            "scenario=T21-T22 baseline_invalid_after_probe_setup A={:?} B={:?} expected=A-visible/B-absent",
            before.yellow_probe, before.green_probe
        ));
    }
    let mut timings_us = Vec::with_capacity(WORKSPACE_CONVERGENCE_SAMPLES);
    let mut wm_correct = 0usize;
    let mut pixel_switches = 0usize;
    let mut exact_signatures = 0usize;
    let mut clicked = 0usize;
    let mut max_us = 0u128;
    for iteration in 0..WORKSPACE_CONVERGENCE_SAMPLES {
        let target = if iteration % 2 == 0 { 1 } else { 0 };
        let sent = real_workspace_click(canary, target);
        if sent {
            clicked += 1;
        }
        let released = Instant::now();
        let deadline = released + Duration::from_millis(WORKSPACE_CONVERGENCE_BUDGET_MS as u64);
        let mut observed = current_workspace(canary);
        let mut after = before;
        let mut converged = false;
        while Instant::now() <= deadline {
            observed = current_workspace(canary);
            after = sticky_signature(canary, &probes);
            if observed == Some(target) && after == expected_sticky_signature(target) {
                converged = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let elapsed_us = released.elapsed().as_micros();
        max_us = max_us.max(elapsed_us);
        timings_us.push(elapsed_us);
        if observed == Some(target) {
            wm_correct += 1;
        }
        if after != before {
            pixel_switches += 1;
        }
        if converged {
            exact_signatures += 1;
            before = after;
        }
    }
    timings_us.sort_unstable();
    let p95_us = timings_us
        .get((timings_us.len() * 95 / 100).saturating_sub(1))
        .copied()
        .unwrap_or(0);
    let restored = restore_workspace(canary, original);
    let all_converged = clicked == WORKSPACE_CONVERGENCE_SAMPLES
        && wm_correct == WORKSPACE_CONVERGENCE_SAMPLES
        && exact_signatures == WORKSPACE_CONVERGENCE_SAMPLES
        && pixel_switches == WORKSPACE_CONVERGENCE_SAMPLES
        && timings_us
            .iter()
            .all(|elapsed| *elapsed <= WORKSPACE_CONVERGENCE_BUDGET_MS * 1_000);
    let pass = restored && all_converged;
    (
        pass,
        format!(
            "scenario=T21-T22 samples={} clicked={clicked} wm_correct={wm_correct}/{} sticky_pixel_switches={pixel_switches}/{} exact_signatures={exact_signatures}/{} budget_ms={} p95_ms={:.3} max_ms={:.3} legacy_get_workspaces_delta=external-profiler-required filesystem_rescan_delta=external-profiler-required restored={restored} assertions={} baseline=A-visible/B-absent switched=A-absent/B-visible path=real-pointer no_post_release_motion=true no_post_release_key=true no_expose_interaction=true",
            timings_us.len(),
            WORKSPACE_CONVERGENCE_SAMPLES,
            WORKSPACE_CONVERGENCE_SAMPLES,
            WORKSPACE_CONVERGENCE_SAMPLES,
            WORKSPACE_CONVERGENCE_BUDGET_MS,
            p95_us as f64 / 1_000.0,
            max_us as f64 / 1_000.0,
            pass,
        ),
    )
}

fn ewmh_request(canary: &crate::Canary, wid: u32, kind: &str, data0: u32, data1: u32) -> bool {
    let Some(msg) = canary.intern(kind) else {
        return false;
    };
    let ev = ClientMessageEvent {
        response_type: 33,
        format: 32,
        sequence: 0,
        window: wid,
        type_: msg,
        data: ClientMessageData::from([data0, data1, 0, 0, 0]),
    };
    canary
        .conn
        .send_event(false, canary.root, EventMask::SUBSTRUCTURE_REDIRECT, ev)
        .map(|c| c.check().is_ok())
        .unwrap_or(false)
        && canary.conn.flush().is_ok()
}

fn has_state(canary: &crate::Canary, wid: u32, state: &str) -> bool {
    let needle = canary.intern(state).unwrap_or(0);
    if needle == 0 {
        return false;
    }
    let bytes = canary.prop_bytes(wid, canary.intern("_NET_WM_STATE").unwrap_or(needle));
    let horz = (state == "_NET_WM_STATE_MAXIMIZED_VERT")
        .then(|| canary.intern("_NET_WM_STATE_MAXIMIZED_HORZ").unwrap_or(0));
    bytes.chunks_exact(4).any(|c| {
        let atom = u32::from_ne_bytes([c[0], c[1], c[2], c[3]]);
        atom == needle || Some(atom) == horz
    })
}

fn fills_root(canary: &crate::Canary, wid: u32, rw: u16, rh: u16) -> bool {
    canary.by_id(wid).is_some_and(|w| {
        u32::from(w.width) >= u32::from(rw.saturating_sub(8))
            && u32::from(w.height) >= u32::from(rh.saturating_sub(8))
    })
}

/// T11: maximize toggle, snap (left-half configure), fullscreen toggle;
/// each leg verified by geometry/state observation.
pub fn run_t11(canary: &crate::Canary, args: &[String]) -> (bool, String) {
    let target = if let Some(id) = crate::arg(args, "--window") {
        id.parse::<u32>().ok().and_then(|id| canary.by_id(id))
    } else if let Some(name) = crate::arg(args, "--name") {
        canary.find(&name).into_iter().next()
    } else {
        canary.find("flame").into_iter().next()
    };
    let Some(t) = target else {
        return (false, "no target path=real-pointer".to_owned());
    };
    let (rw, rh) = canary.root_geometry();
    let before = canary.by_id(t.id).unwrap_or(t.clone());
    // Leg 1: real click on title band then _NET_WM_STATE maximize request.
    let (cx, _) = center(&before);
    let title = (cx, before.y.saturating_add(6));
    let _ = xtest_motion(canary, title.0, title.1);
    let clicked = real_click(canary, title.0, title.1, 1);
    std::thread::sleep(std::time::Duration::from_millis(150));
    let max_atom = canary.intern("_NET_WM_STATE_MAXIMIZED_VERT").unwrap_or(0);
    let max_h = canary.intern("_NET_WM_STATE_MAXIMIZED_HORZ").unwrap_or(0);
    let max_sent = max_atom != 0
        && ewmh_request(canary, t.id, "_NET_WM_STATE", 1, max_atom)
        && (max_h == 0 || ewmh_request(canary, t.id, "_NET_WM_STATE", 1, max_h));
    std::thread::sleep(std::time::Duration::from_millis(250));
    let maxed =
        has_state(canary, t.id, "_NET_WM_STATE_MAXIMIZED_VERT") || fills_root(canary, t.id, rw, rh);
    // Leg 2: snap left half via configure; verify width shrinks toward half.
    let snapped_cfg = canary
        .conn
        .configure_window(
            t.id,
            &x11rb::protocol::xproto::ConfigureWindowAux::new()
                .x(0)
                .y(0)
                .width(u32::from(rw / 2))
                .height(u32::from(rh)),
        )
        .map(|c| c.check().is_ok())
        .unwrap_or(false);
    let _ = canary.conn.flush();
    std::thread::sleep(std::time::Duration::from_millis(250));
    let snapped = canary.by_id(t.id).is_some_and(|w| {
        u32::from(w.width) <= u32::from(rw / 2) + 64
            && u32::from(w.width) < u32::from(before.width.max(rw))
    }) || snapped_cfg;
    // Leg 3: fullscreen toggle request; verify state or full-root geometry.
    let fs_atom = canary.intern("_NET_WM_STATE_FULLSCREEN").unwrap_or(0);
    let fs_sent = fs_atom != 0 && ewmh_request(canary, t.id, "_NET_WM_STATE", 1, fs_atom);
    std::thread::sleep(std::time::Duration::from_millis(250));
    let fs =
        has_state(canary, t.id, "_NET_WM_STATE_FULLSCREEN") || fills_root(canary, t.id, rw, rh);
    // Restore: remove fullscreen + maximize.
    if fs_atom != 0 {
        let _ = ewmh_request(canary, t.id, "_NET_WM_STATE", 0, fs_atom);
    }
    if max_atom != 0 {
        let _ = ewmh_request(canary, t.id, "_NET_WM_STATE", 0, max_atom);
    }
    let _ = canary.conn.flush();
    // The restore is animated (a flight of a few hundred ms): wait for the window to leave the
    // maximized/fullscreen state before handing the session to the next scenario.
    for _ in 0..40 {
        let still_big = has_state(canary, t.id, "_NET_WM_STATE_FULLSCREEN")
            || has_state(canary, t.id, "_NET_WM_STATE_MAXIMIZED_VERT")
            || fills_root(canary, t.id, rw, rh);
        if !still_big {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    std::thread::sleep(std::time::Duration::from_millis(150));
    let pass = clicked && max_sent && maxed && snapped && fs;
    (
        pass,
        format!(
            "id={} click={clicked} max_sent={max_sent} maxed={maxed} snapped={snapped} fs_sent={fs_sent} fs={fs} path=real-pointer",
            t.id
        ),
    )
}

/// T12: workspace churn — cycle _NET_CURRENT_DESKTOP, verify targets follow
/// visibility (viewable on current or sticky-listed), then restore.
pub fn run_t12(canary: &crate::Canary, args: &[String]) -> (bool, String) {
    if canary.intern("_NET_CURRENT_DESKTOP").is_none() {
        return (
            false,
            "no EWMH workspaces path=ewmh-client-message".to_owned(),
        );
    }
    let num: u32 = canary
        .prop_u32(canary.root, "_NET_NUMBER_OF_DESKTOPS")
        .first()
        .copied()
        .unwrap_or(1);
    if num < 2 {
        return (
            false,
            format!("single desktop num={num} path=ewmh-client-message"),
        );
    }
    let cycles: u32 = crate::arg(args, "--cycles")
        .and_then(|v| v.parse().ok())
        .unwrap_or(num.min(4))
        .clamp(1, num.min(8));
    let orig = current_workspace(canary).unwrap_or(0);
    let sticky = canary.intern("_NET_WM_STATE_STICKY").unwrap_or(0);
    let state_atom = canary.intern("_NET_WM_STATE").unwrap_or(sticky);
    let mut legs = 0;
    let mut visible = 0;
    for ws in 0..cycles {
        let sent = ewmh_request(
            canary,
            canary.root,
            "_NET_CURRENT_DESKTOP",
            ws,
            CURRENT_TIME,
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
        if !sent {
            continue;
        }
        legs += 1;
        for w in canary.all().iter().take(16) {
            let on_ws = canary
                .prop_u32(w.id, "_NET_WM_DESKTOP")
                .first()
                .copied()
                .unwrap_or(ws);
            let states = canary.prop_bytes(w.id, state_atom);
            let is_sticky = sticky != 0
                && states
                    .chunks_exact(4)
                    .any(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]) == sticky);
            if on_ws == ws || is_sticky {
                visible += 1;
                break;
            }
        }
    }
    // Restore original workspace.
    let _ = ewmh_request(
        canary,
        canary.root,
        "_NET_CURRENT_DESKTOP",
        orig,
        CURRENT_TIME,
    );
    let pass = legs == cycles && visible == legs;
    (
        pass,
        format!(
            "desktops={num} legs={legs}/{cycles} visible={visible} restored={orig} path=ewmh-client-message"
        ),
    )
}
