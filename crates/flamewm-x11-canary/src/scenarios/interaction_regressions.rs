//! Bounded X01-X09 interaction regressions.
//!
//! These probes deliberately use the canonical XTEST helpers.  The canary can
//! observe native geometry and EWMH state, but it cannot read the WM's private
//! effect/counter stream or query an X pointer grab.  Those assertions remain
//! explicit blockers rather than being replaced with synthetic evidence.

use super::{InputPath, xtest_button_press, xtest_button_release, xtest_drag_path, xtest_motion};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{
    ChangeWindowAttributesAux, ConnectionExt as _, EventMask, WindowClass,
};

#[derive(Clone, Copy)]
struct Case {
    id: &'static str,
    name: &'static str,
    region: &'static str,
    start: fn(&crate::WinInfo) -> (i16, i16),
    delta: (i16, i16),
}

fn title(w: &crate::WinInfo) -> (i16, i16) {
    (
        w.x.saturating_add(w.width as i16 / 2),
        w.y.saturating_add(8),
    )
}

fn left(w: &crate::WinInfo) -> (i16, i16) {
    (
        w.x.saturating_add(2),
        w.y.saturating_add(w.height as i16 / 2),
    )
}

fn top_left(w: &crate::WinInfo) -> (i16, i16) {
    (w.x.saturating_add(2), w.y.saturating_add(2))
}

fn bottom_left(w: &crate::WinInfo) -> (i16, i16) {
    (
        w.x.saturating_add(2),
        w.y.saturating_add(w.height as i16 - 3),
    )
}

fn bottom_right(w: &crate::WinInfo) -> (i16, i16) {
    (
        w.x.saturating_add(w.width as i16 - 3),
        w.y.saturating_add(w.height as i16 - 3),
    )
}

fn source_at(canary: &crate::Canary, x: i16, y: i16) -> Option<crate::WinInfo> {
    canary
        .all()
        .into_iter()
        .filter(|w| {
            x >= w.x
                && y >= w.y
                && x < w.x.saturating_add(w.width as i16)
                && y < w.y.saturating_add(w.height as i16)
        })
        .max_by_key(|w| w.width as u32 * u32::from(w.height))
}

fn select_structure_notify(canary: &crate::Canary, target: &crate::WinInfo) -> bool {
    let mask = ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY);
    [target.id, target.parent]
        .into_iter()
        .filter(|window| *window != canary.root)
        .all(|window| {
            canary
                .conn
                .change_window_attributes(window, &mask)
                .is_ok_and(|cookie| cookie.check().is_ok())
        })
        && canary.conn.flush().is_ok()
}

pub(crate) fn native_configures(
    canary: &crate::Canary,
    target: &crate::WinInfo,
    timeout_ms: u64,
) -> usize {
    let mut count = 0;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        match canary.conn.poll_for_event() {
            Ok(Some(x11rb::protocol::Event::ConfigureNotify(event)))
                if event.event == target.id
                    || event.window == target.id
                    || event.event == target.parent
                    || event.window == target.parent =>
            {
                count += 1;
            }
            Ok(Some(_)) => {}
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    return count;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(_) => return count,
        }
    }
}

fn input_surface(canary: &crate::Canary, target: &crate::WinInfo) -> Option<crate::WinInfo> {
    super::window_interaction::input_surface(canary, target)
}

pub(crate) fn wait_until<F>(timeout_ms: u64, mut ready: F) -> bool
where
    F: FnMut() -> bool,
{
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        if ready() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

#[derive(Clone, Copy)]
struct PinnedX03 {
    client: u32,
    frame: u32,
}

#[derive(Debug)]
enum X03ProbeError {
    IdentityChange(String),
    RouteMismatch(String),
    XtestFailed(String),
}

fn x03_identity(
    canary: &crate::Canary,
    pinned: PinnedX03,
) -> Result<(crate::WinInfo, crate::WinInfo), X03ProbeError> {
    let client = canary
        .by_id(pinned.client)
        .filter(|window| window.parent == pinned.frame)
        .ok_or_else(|| {
            X03ProbeError::IdentityChange(format!(
                "client={} frame={} client_missing_or_reparented",
                pinned.client, pinned.frame
            ))
        })?;
    let frame = canary
        .by_id(pinned.frame)
        .filter(|window| window.parent == canary.root)
        .ok_or_else(|| {
            X03ProbeError::IdentityChange(format!(
                "client={} frame={} frame_missing_or_reparented",
                pinned.client, pinned.frame
            ))
        })?;
    Ok((client, frame))
}

fn x03_rect(window: &crate::WinInfo) -> String {
    format!(
        "{}x{}+{}+{}",
        window.width, window.height, window.x, window.y
    )
}

fn x03_native_route(canary: &crate::Canary) -> Option<u32> {
    let child = canary
        .conn
        .query_pointer(canary.root)
        .ok()?
        .reply()
        .ok()?
        .child;
    (child != x11rb::NONE).then_some(child)
}

fn x03_interaction_capture_route(
    canary: &crate::Canary,
    window: u32,
    root_geometry: (u16, u16),
) -> bool {
    let Some(info) = canary.by_id(window) else {
        return false;
    };
    if info.parent != canary.root
        || info.x != 0
        || info.y != 0
        || info.map_state != 2
        || info.width != root_geometry.0
        || info.height != root_geometry.1
    {
        return false;
    }
    canary
        .conn
        .get_window_attributes(window)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .is_some_and(|attributes| attributes.class == WindowClass::INPUT_ONLY)
}

fn x03_root_ready(
    client: &crate::WinInfo,
    frame: &crate::WinInfo,
    root: (i16, i16),
    root_geometry: (u16, u16),
    capture_route: bool,
) -> bool {
    let x = i32::from(root.0);
    let y = i32::from(root.1);
    let frame_x = i32::from(frame.x);
    let frame_y = i32::from(frame.y);
    let frame_right = frame_x + i32::from(frame.width);
    let frame_bottom = frame_y + i32::from(frame.height);
    client.map_state == 2
        && client.width > 0
        && client.height > 0
        && frame.map_state == 2
        && frame.width > 0
        && frame.height > 0
        && x >= 0
        && y >= 0
        && x < i32::from(root_geometry.0)
        && y < i32::from(root_geometry.1)
        && (capture_route || (x >= frame_x && y >= frame_y && x < frame_right && y < frame_bottom))
}

fn x03_observe(
    canary: &crate::Canary,
    pinned: PinnedX03,
    event: &str,
    root: (i16, i16),
    logs: &mut Vec<String>,
) -> Result<(), X03ProbeError> {
    let (client, frame) = x03_identity(canary, pinned)?;
    let active = canary.active_window();
    let (root_width, root_height) = canary.root_geometry();
    let native_route = x03_native_route(canary);
    let capture_route = native_route.is_some_and(|window| {
        x03_interaction_capture_route(canary, window, (root_width, root_height))
    });
    let root_ready = x03_root_ready(
        &client,
        &frame,
        root,
        (root_width, root_height),
        capture_route,
    );
    let native_route_ok = native_route.map_or(true, |window| {
        window == pinned.client || window == pinned.frame || capture_route
    });
    let active_ok = active.map_or(true, |window| {
        window == pinned.client || window == pinned.frame
    });
    let pre_event = event.starts_with("before-");
    logs.push(format!(
        "phase=X03 event={} root=({}, {}) root_geometry={}x{} client={} frame={} rect=client:{} frame:{} active={} native_route={} capture_route={} root_ready={} active_ok={} native_route_ok={} route_validation={}",
        event,
        root.0,
        root.1,
        root_width,
        root_height,
        client.id,
        frame.id,
        x03_rect(&client),
        x03_rect(&frame),
        active.map_or_else(|| "none".to_owned(), |window| window.to_string()),
        native_route.map_or_else(|| "none".to_owned(), |window| window.to_string()),
        capture_route,
        root_ready,
        active_ok,
        native_route_ok,
        if pre_event { "deferred" } else { "post-event" },
    ));
    if pre_event && !root_ready {
        return Err(X03ProbeError::IdentityChange(format!(
            "event={} pinned_frame_root_not_ready root=({}, {}) client={} frame={}",
            event, root.0, root.1, pinned.client, pinned.frame
        )));
    }
    if !active_ok {
        return Err(X03ProbeError::RouteMismatch(format!(
            "event={} active={:?} expected_client={} frame={}",
            event, active, pinned.client, pinned.frame
        )));
    }
    if !pre_event && !native_route_ok {
        return Err(X03ProbeError::RouteMismatch(format!(
            "event={} native_route={:?} capture_route={} expected_client={} frame={}",
            event, native_route, capture_route, pinned.client, pinned.frame
        )));
    }
    Ok(())
}

fn x03_event<F>(
    canary: &crate::Canary,
    pinned: PinnedX03,
    event: &str,
    root: (i16, i16),
    logs: &mut Vec<String>,
    send: F,
) -> Result<(), X03ProbeError>
where
    F: FnOnce(&crate::Canary) -> bool,
{
    x03_observe(canary, pinned, &format!("before-{}", event), root, logs)?;
    if !send(canary) {
        return Err(X03ProbeError::XtestFailed(format!(
            "event={} root=({}, {})",
            event, root.0, root.1
        )));
    }
    x03_observe(canary, pinned, &format!("after-{}", event), root, logs)
}

fn x03_drag(
    canary: &crate::Canary,
    pinned: PinnedX03,
    from: (i16, i16),
    to: (i16, i16),
    button: u8,
    steps: u32,
    logs: &mut Vec<String>,
) -> Result<(), X03ProbeError> {
    x03_event(canary, pinned, "motion", from, logs, |canary| {
        xtest_motion(canary, from.0, from.1)
    })?;
    x03_event(canary, pinned, "press", from, logs, |canary| {
        xtest_button_press(canary, from.0, from.1, button)
    })?;
    let steps = steps.max(1);
    for i in 1..=steps {
        let i = i64::from(i);
        let steps = i64::from(steps);
        let x = i64::from(from.0) + (i64::from(to.0) - i64::from(from.0)) * i / steps;
        let y = i64::from(from.1) + (i64::from(to.1) - i64::from(from.1)) * i / steps;
        let point = (x as i16, y as i16);
        x03_event(canary, pinned, "motion", point, logs, |canary| {
            xtest_motion(canary, point.0, point.1)
        })?;
    }
    x03_event(canary, pinned, "release", to, logs, |canary| {
        xtest_button_release(canary, to.0, to.1, button)
    })
}

fn x03_maximize(
    canary: &crate::Canary,
    pinned: PinnedX03,
    logs: &mut Vec<String>,
) -> Result<(), X03ProbeError> {
    let (_, frame) = x03_identity(canary, pinned)?;
    let start = title(&frame);
    let (screen_w, _) = canary.root_geometry();
    let top = ((screen_w as i16 / 2).max(1), 2);
    x03_event(canary, pinned, "maximize-motion", start, logs, |canary| {
        xtest_motion(canary, start.0, start.1)
    })?;
    x03_event(canary, pinned, "maximize-press", start, logs, |canary| {
        xtest_button_press(canary, start.0, start.1, 1)
    })?;
    x03_event(canary, pinned, "maximize-motion", top, logs, |canary| {
        xtest_motion(canary, top.0, top.1)
    })?;
    x03_event(canary, pinned, "maximize-release", top, logs, |canary| {
        xtest_button_release(canary, top.0, top.1, 1)
    })
}

fn x03_failure(pinned: PinnedX03, error: impl std::fmt::Debug, logs: &[String]) -> (bool, String) {
    (
        false,
        format!(
            "scenario=X03 client={} frame={} assertions=false probe_failure={error:?} diagnostics=[{}]",
            pinned.client,
            pinned.frame,
            logs.join(" | ")
        ),
    )
}

fn prepare_native_measurement(canary: &crate::Canary, target: &crate::WinInfo) -> bool {
    if !select_structure_notify(canary, target) {
        return false;
    }
    let _ = native_configures(canary, target, 0);
    true
}

fn establish_floating(canary: &crate::Canary, target: &crate::WinInfo) -> bool {
    if !maximized(canary, target.id) {
        return true;
    }
    let Some(surface) = input_surface(canary, target) else {
        return false;
    };
    let start = title(&surface);
    let end = (start.0.saturating_add(64), start.1.saturating_add(48));
    if !xtest_drag_path(canary, start, end, 1, 8) {
        return false;
    }
    wait_until(500, || !maximized(canary, target.id))
}

fn maximize_with_xtest(canary: &crate::Canary, target: &crate::WinInfo) -> bool {
    let Some(surface) = input_surface(canary, target) else {
        return false;
    };
    let start = title(&surface);
    let (screen_w, _) = canary.root_geometry();
    let top = ((screen_w as i16 / 2).max(1), 2);
    xtest_motion(canary, start.0, start.1)
        && xtest_button_press(canary, start.0, start.1, 1)
        && xtest_motion(canary, top.0, top.1)
        && xtest_button_release(canary, top.0, top.1, 1)
        && wait_until(500, || maximized(canary, target.id))
}

fn expected(case: &Case, before: &crate::WinInfo, after: &crate::WinInfo) -> bool {
    match case.name {
        "titlebar-drag" => after.x != before.x || after.y != before.y,
        "left/L" => after.x < before.x && after.width > before.width,
        "TL" => {
            after.x < before.x
                && after.y < before.y
                && after.width > before.width
                && after.height > before.height
        }
        "BL" => after.x < before.x && after.height > before.height,
        "BR" => after.width > before.width && after.height > before.height,
        _ => false,
    }
}

fn wm_state(canary: &crate::Canary, window: u32) -> Vec<u32> {
    canary.prop_u32(window, "_NET_WM_STATE")
}

fn maximized(canary: &crate::Canary, window: u32) -> bool {
    let state = wm_state(canary, window);
    let horz = canary
        .intern("_NET_WM_STATE_MAXIMIZED_HORZ")
        .is_some_and(|atom| state.contains(&atom));
    let vert = canary
        .intern("_NET_WM_STATE_MAXIMIZED_VERT")
        .is_some_and(|atom| state.contains(&atom));
    horz || vert
}

fn ewmh_available(canary: &crate::Canary) -> bool {
    canary.intern("_NET_WM_STATE").is_some() && canary.intern("_NET_ACTIVE_WINDOW").is_some()
}

fn active_target(canary: &crate::Canary, target: u32) -> bool {
    canary.active_window() == Some(target)
}

fn preview_visible(canary: &crate::Canary) -> Option<crate::WinInfo> {
    canary
        .find("FlameWM snap preview")
        .into_iter()
        .find(|window| window.map_state == 2 && window.width > 0 && window.height > 0)
}

/// `_NET_WM_PID` is the only process-health evidence available to this
/// protocol-only canary.  `None` is an unsupported assertion, not a pass.
fn process_health(canary: &crate::Canary, window: u32) -> Option<bool> {
    let pid = canary.prop_u32(window, "_NET_WM_PID").first().copied()?;
    Some(std::path::Path::new(&format!("/proc/{pid}")).is_dir())
}

fn unsupported_assertions() -> &'static str {
    "wm-counters,x-pointer-grab"
}

fn process_health_detail(health: Option<bool>) -> &'static str {
    match health {
        Some(true) => "ok",
        Some(false) => "failed",
        None => "unsupported-no-_NET_WM_PID",
    }
}

/// X01-X03 are deliberately independent real-XTEST phases.  Each phase
/// resolves a ready client/frame pair, establishes its starting state, selects
/// StructureNotify, and reports only observations available to this canary.
fn run_drag_max_probe(canary: &crate::Canary, args: &[String]) -> (bool, String) {
    let phases = [run_x01, run_x02, run_x03]
        .into_iter()
        .map(|phase| phase(canary, args))
        .collect::<Vec<_>>();
    let x01 = phases[0].0;
    let x02 = phases[1].0;
    let x03 = phases[2].0;
    let target = super::window_interaction::resolve_target(canary, args);
    let (native, ewmh, active, health) = target.map_or((0, false, false, None), |target| {
        let native = native_configures(canary, &target, 250);
        (
            native,
            ewmh_available(canary),
            active_target(canary, target.id),
            process_health(canary, target.id),
        )
    });
    let x04 =
        native > 0 && ewmh && active && health == Some(true) && unsupported_assertions().is_empty();
    let assertions = x01 && x02 && x03 && x04;
    (
        assertions,
        format!(
            "scenario=X01-X04 x01={} x02={} x03={} x04={} native_configures={} ewmh={} active={} process_health={} assertions={} unsupported={} path={} phases=[{}]",
            x01,
            x02,
            x03,
            x04,
            native,
            ewmh,
            active,
            process_health_detail(health),
            assertions,
            unsupported_assertions(),
            InputPath::RealPointer.as_str(),
            phases
                .iter()
                .map(|(_, detail)| detail.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        ),
    )
}

fn run_x01(canary: &crate::Canary, args: &[String]) -> (bool, String) {
    let Some(target) = super::window_interaction::resolve_target(canary, args) else {
        return (
            false,
            "scenario=X01 target_not_ready path=real-pointer".to_owned(),
        );
    };
    let reset = establish_floating(canary, &target);
    let Some(before) = canary.by_id(target.id) else {
        return (false, format!("scenario=X01 id={} target_gone", target.id));
    };
    let Some(surface) = input_surface(canary, &before) else {
        return (
            false,
            format!("scenario=X01 id={} frame_not_ready", target.id),
        );
    };
    let selected = prepare_native_measurement(canary, &before);
    let start = title(&surface);
    let end = (start.0.saturating_add(48), start.1.saturating_add(32));
    let dragged = reset && xtest_drag_path(canary, start, end, 1, 8);
    let moved = wait_until(500, || {
        canary
            .by_id(before.id)
            .and_then(|after| input_surface(canary, &after))
            .is_some_and(|after| after.x != surface.x || after.y != surface.y)
    });
    let after_max = maximized(canary, before.id);
    let native = native_configures(canary, &before, 250);
    let pass = selected && dragged && moved && !after_max && native > 0;
    (
        pass,
        format!(
            "scenario=X01 id={} reset_floating={} selected_structure_notify={} dragged={} moved={} maximized={} native_configures={} assertions={} path={}",
            before.id,
            reset,
            selected,
            dragged,
            moved,
            after_max,
            native,
            pass,
            InputPath::RealPointer.as_str()
        ),
    )
}

fn run_x02(canary: &crate::Canary, args: &[String]) -> (bool, String) {
    let Some(target) = super::window_interaction::resolve_target(canary, args) else {
        return (
            false,
            "scenario=X02 target_not_ready path=real-pointer".to_owned(),
        );
    };
    let reset = establish_floating(canary, &target);
    let Some(before) = canary.by_id(target.id) else {
        return (false, format!("scenario=X02 id={} target_gone", target.id));
    };
    let Some(_surface) = input_surface(canary, &before) else {
        return (
            false,
            format!("scenario=X02 id={} frame_not_ready", target.id),
        );
    };
    let selected = prepare_native_measurement(canary, &before);
    let maximized_once = reset && maximize_with_xtest(canary, &before);
    let Some(current) = super::window_interaction::resolve_target(canary, args) else {
        return (
            false,
            format!("scenario=X02 id={} target_lost_after_maximize", target.id),
        );
    };
    let Some(max_surface) = input_surface(canary, &current) else {
        return (
            false,
            format!("scenario=X02 id={} max_frame_not_ready", target.id),
        );
    };
    let jitter_start = title(&max_surface);
    let jitter_end = (
        jitter_start.0.saturating_add(8),
        jitter_start.1.saturating_add(1),
    );
    let jittered = xtest_drag_path(canary, jitter_start, jitter_end, 1, 4);
    let remained_maximized = wait_until(500, || maximized(canary, current.id));
    let native = native_configures(canary, &current, 250);
    let pass = selected && maximized_once && jittered && remained_maximized && native > 0;
    (
        pass,
        format!(
            "scenario=X02 id={} reset_floating={} selected_structure_notify={} maximized_once={} jittered={} remained_maximized={} native_configures={} assertions={} path={}",
            current.id,
            reset,
            selected,
            maximized_once,
            jittered,
            remained_maximized,
            native,
            pass,
            InputPath::RealPointer.as_str()
        ),
    )
}

fn run_x03(canary: &crate::Canary, args: &[String]) -> (bool, String) {
    let Some(target) = super::window_interaction::resolve_target(canary, args) else {
        return (
            false,
            "scenario=X03 target_not_ready path=real-pointer".to_owned(),
        );
    };
    let pinned = PinnedX03 {
        client: target.id,
        frame: target.parent,
    };
    let mut logs = Vec::with_capacity(48);
    let Ok((initial_floating, initial_surface)) = x03_identity(canary, pinned) else {
        return x03_failure(
            pinned,
            X03ProbeError::IdentityChange("target_not_ready_after_pin".to_owned()),
            &logs,
        );
    };
    let floating_title = title(&initial_surface);
    if let Err(error) = x03_observe(
        canary,
        pinned,
        "before-establish-floating",
        floating_title,
        &mut logs,
    ) {
        return x03_failure(pinned, error, &logs);
    }
    let reset = if maximized(canary, pinned.client) {
        let end = (
            floating_title.0.saturating_add(64),
            floating_title.1.saturating_add(48),
        );
        match x03_drag(canary, pinned, floating_title, end, 1, 8, &mut logs) {
            Ok(()) => wait_until(500, || !maximized(canary, pinned.client)),
            Err(error) => return x03_failure(pinned, error, &logs),
        }
    } else {
        true
    };
    let Some(floating) = canary
        .by_id(pinned.client)
        .filter(|window| window.parent == pinned.frame)
    else {
        return x03_failure(
            pinned,
            X03ProbeError::IdentityChange("target_gone_after_establish".to_owned()),
            &logs,
        );
    };
    let Some(floating_surface) = canary
        .by_id(pinned.frame)
        .filter(|window| window.parent == canary.root)
    else {
        return x03_failure(
            pinned,
            X03ProbeError::IdentityChange("frame_gone_after_establish".to_owned()),
            &logs,
        );
    };
    let selected_before = prepare_native_measurement(canary, &floating);
    let maximized_once = if !reset {
        false
    } else {
        match x03_maximize(canary, pinned, &mut logs) {
            Ok(()) => wait_until(500, || maximized(canary, pinned.client)),
            Err(error) => return x03_failure(pinned, error, &logs),
        }
    };
    let Some(max_surface) = canary
        .by_id(pinned.frame)
        .filter(|window| window.parent == canary.root)
    else {
        return x03_failure(
            pinned,
            X03ProbeError::IdentityChange("frame_gone_after_maximize".to_owned()),
            &logs,
        );
    };
    let max_title = title(&max_surface);
    if let Err(error) = x03_observe(canary, pinned, "after-maximize", max_title, &mut logs) {
        return x03_failure(pinned, error, &logs);
    }
    let selected_after = canary
        .by_id(pinned.client)
        .filter(|window| window.parent == pinned.frame)
        .is_some_and(|window| prepare_native_measurement(canary, &window));
    let end = (
        max_title.0.saturating_add(64),
        max_title.1.saturating_add(48),
    );
    let dragged_away = match x03_drag(canary, pinned, max_title, end, 1, 8, &mut logs) {
        Ok(()) => true,
        Err(error) => return x03_failure(pinned, error, &logs),
    };
    let restored = wait_until(500, || !maximized(canary, pinned.client));
    let Some(after) = canary
        .by_id(pinned.client)
        .filter(|window| window.parent == pinned.frame)
    else {
        return x03_failure(
            pinned,
            X03ProbeError::IdentityChange("target_gone_after_restore".to_owned()),
            &logs,
        );
    };
    let Some(after_surface) = canary
        .by_id(pinned.frame)
        .filter(|window| window.parent == canary.root)
    else {
        return x03_failure(
            pinned,
            X03ProbeError::IdentityChange("frame_gone_after_restore".to_owned()),
            &logs,
        );
    };
    if let Err(error) = x03_observe(
        canary,
        pinned,
        "after-restore",
        title(&after_surface),
        &mut logs,
    ) {
        return x03_failure(pinned, error, &logs);
    }
    let position_changed =
        after_surface.x != initial_surface.x || after_surface.y != initial_surface.y;
    let size_restored =
        after.width == initial_floating.width && after.height == initial_floating.height;
    let native = native_configures(canary, &after, 250);
    let pass = selected_before
        && selected_after
        && maximized_once
        && dragged_away
        && restored
        && position_changed
        && size_restored
        && native > 0;
    (
        pass,
        format!(
            "scenario=X03 client={} frame={} reset_floating={} selected_before={} selected_after={} maximized_once={} dragged_away={} restored_floating={} position_changed={} size_restored={} native_configures={} assertions={} path={} diagnostics=[{}]",
            pinned.client,
            pinned.frame,
            reset,
            selected_before,
            selected_after,
            maximized_once,
            dragged_away,
            restored,
            position_changed,
            size_restored,
            native,
            pass,
            InputPath::RealPointer.as_str(),
            logs.join(" | ")
        ),
    )
}

const TOP_HOLD_MOTIONS: usize = 200;

struct TopHoldSetup {
    client: crate::WinInfo,
    frame: crate::WinInfo,
    start: (i16, i16),
    top: (i16, i16),
    initial_state: Vec<u32>,
    initial_preview: bool,
    activated_floating: bool,
    selected_structure_notify: bool,
    ewmh: bool,
    work_area: Option<(i32, i32, u32, u32)>,
}

struct TopHoldObservation {
    motion_count: usize,
    identity_stable: bool,
    dimensions_stable: bool,
    state_stable: bool,
    maximized_seen: bool,
    final_state: Vec<u32>,
    final_client: Option<crate::WinInfo>,
    final_frame: Option<crate::WinInfo>,
    native_route: Option<u32>,
    capture_route: bool,
    active: bool,
    preview_visible: bool,
    native_configures: usize,
    process_health: Option<bool>,
}

pub(crate) fn root_work_area(canary: &crate::Canary) -> Option<(i32, i32, u32, u32)> {
    let desktop = canary
        .prop_u32(canary.root, "_NET_CURRENT_DESKTOP")
        .first()
        .copied()
        .unwrap_or(0) as usize;
    let values = canary.prop_u32(canary.root, "_NET_WORKAREA");
    let start = desktop.checked_mul(4)?;
    if values.len() < start.saturating_add(4) {
        return None;
    }
    let x = i32::from_ne_bytes(values[start].to_ne_bytes());
    let y = i32::from_ne_bytes(values[start + 1].to_ne_bytes());
    let width = values[start + 2];
    let height = values[start + 3];
    (width > 0 && height > 0).then_some((x, y, width, height))
}

fn work_area_detail(work_area: Option<(i32, i32, u32, u32)>) -> String {
    work_area.map_or_else(
        || "unavailable".to_owned(),
        |(x, y, width, height)| format!("{width}x{height}+{x}+{y}"),
    )
}

pub(crate) fn maximized_both(canary: &crate::Canary, window: u32) -> bool {
    let state = wm_state(canary, window);
    let Some(horz) = canary.intern("_NET_WM_STATE_MAXIMIZED_HORZ") else {
        return false;
    };
    let Some(vert) = canary.intern("_NET_WM_STATE_MAXIMIZED_VERT") else {
        return false;
    };
    state.contains(&horz) && state.contains(&vert)
}

fn begin_top_hold(canary: &crate::Canary, args: &[String]) -> Result<TopHoldSetup, String> {
    let target = super::window_interaction::resolve_target(canary, args)
        .ok_or_else(|| "target_not_ready path=real-pointer".to_owned())?;
    let reset_floating = establish_floating(canary, &target);
    if !reset_floating {
        return Err(format!("id={} establish_floating=false", target.id));
    }
    let client = canary
        .by_id(target.id)
        .filter(|window| window.parent == target.parent)
        .ok_or_else(|| format!("id={} target_gone_after_establish", target.id))?;
    let frame = canary
        .by_id(client.parent)
        .filter(|window| window.parent == canary.root)
        .ok_or_else(|| format!("id={} frame_not_ready_after_establish", client.id))?;
    let selected_structure_notify = prepare_native_measurement(canary, &client);
    let initial_state = wm_state(canary, client.id);
    let start = title(&frame);
    let (screen_w, _) = canary.root_geometry();
    let top = ((screen_w as i16 / 2).max(1), 2);
    if !xtest_motion(canary, start.0, start.1) {
        return Err(format!("id={} initial_warp_failed", client.id));
    }
    if !xtest_button_press(canary, start.0, start.1, 1) {
        return Err(format!("id={} xtest_press_failed", client.id));
    }
    // This is the activation motion.  The button remains held after this
    // call; callers perform all hold assertions before sending a release.
    if !xtest_motion(canary, top.0, top.1) {
        let _ = xtest_button_release(canary, start.0, start.1, 1);
        return Err(format!("id={} activation_motion_failed", client.id));
    }
    let activated_floating = canary
        .by_id(client.id)
        .filter(|window| window.parent == frame.id)
        .zip(
            canary
                .by_id(frame.id)
                .filter(|window| window.parent == canary.root),
        )
        .is_some_and(|(current_client, current_frame)| {
            !maximized(canary, client.id)
                && current_client.width == client.width
                && current_client.height == client.height
                && current_frame.width == frame.width
                && current_frame.height == frame.height
        });
    Ok(TopHoldSetup {
        client,
        frame,
        start,
        top,
        initial_state,
        initial_preview: preview_visible(canary).is_some(),
        activated_floating,
        selected_structure_notify,
        ewmh: ewmh_available(canary),
        work_area: root_work_area(canary),
    })
}

fn observe_top_hold(canary: &crate::Canary, setup: &TopHoldSetup) -> TopHoldObservation {
    let mut motion_count = 0;
    let mut identity_stable = true;
    let mut dimensions_stable = true;
    let mut state_stable = true;
    let mut maximized_seen = false;
    for index in 0..TOP_HOLD_MOTIONS {
        let offset = (index % 9) as i16 - 4;
        let point = (
            setup.top.0.saturating_add(offset),
            setup.top.1.saturating_add((index % 3) as i16),
        );
        if xtest_motion(canary, point.0, point.1) {
            motion_count += 1;
        }
        let Some(client) = canary
            .by_id(setup.client.id)
            .filter(|window| window.parent == setup.frame.id)
        else {
            identity_stable = false;
            continue;
        };
        let Some(frame) = canary
            .by_id(setup.frame.id)
            .filter(|window| window.parent == canary.root)
        else {
            identity_stable = false;
            continue;
        };
        dimensions_stable &= client.width == setup.client.width
            && client.height == setup.client.height
            && frame.width == setup.frame.width
            && frame.height == setup.frame.height;
        let state = wm_state(canary, setup.client.id);
        state_stable &= state == setup.initial_state;
        maximized_seen |= maximized(canary, setup.client.id);
    }
    let native_route = x03_native_route(canary);
    let capture_route = native_route.is_some_and(|window| {
        x03_interaction_capture_route(canary, window, canary.root_geometry())
    });
    let final_client = canary
        .by_id(setup.client.id)
        .filter(|window| window.parent == setup.frame.id);
    let final_frame = canary
        .by_id(setup.frame.id)
        .filter(|window| window.parent == canary.root);
    let final_state = wm_state(canary, setup.client.id);
    let native_configures = native_configures(canary, &setup.client, 250);
    TopHoldObservation {
        motion_count,
        identity_stable,
        dimensions_stable,
        state_stable,
        maximized_seen,
        final_state,
        final_client,
        final_frame,
        native_route,
        capture_route,
        active: active_target(canary, setup.client.id),
        preview_visible: preview_visible(canary).is_some(),
        native_configures,
        process_health: process_health(canary, setup.client.id),
    }
}

fn release_top_hold_without_snap(canary: &crate::Canary, setup: &TopHoldSetup) -> bool {
    // Leave the maximize zone while still held, then release exactly once.
    // This is cleanup for X04 and happens only after its hold assertions.
    let moved = xtest_motion(canary, setup.start.0, setup.start.1);
    let released = xtest_button_release(canary, setup.start.0, setup.start.1, 1);
    moved && released
}

fn run_x04(canary: &crate::Canary, args: &[String]) -> (bool, String) {
    let setup = match begin_top_hold(canary, args) {
        Ok(setup) => setup,
        Err(error) => return (false, format!("scenario=X04 {error}")),
    };
    let hold = observe_top_hold(canary, &setup);
    let ewmh_state_unchanged = hold.state_stable;
    let route_ok = hold.capture_route
        || hold.native_route == Some(setup.client.id)
        || hold.native_route == Some(setup.frame.id);
    let counters_available = unsupported_assertions().is_empty();
    // Compute this verdict before cleanup release.  X04 must prove that the
    // held gesture itself did not release or commit a maximize.
    let hold_assertions = setup.activated_floating
        && hold.motion_count >= TOP_HOLD_MOTIONS
        && hold.identity_stable
        && hold.dimensions_stable
        && ewmh_state_unchanged
        && !hold.maximized_seen
        && setup.ewmh
        && setup.selected_structure_notify
        && hold.active
        && route_ok
        && hold.process_health == Some(true)
        && counters_available;
    let cleanup_release = release_top_hold_without_snap(canary, &setup);
    let pass = hold_assertions && cleanup_release;
    (
        pass,
        format!(
            "scenario=X04 id={} start=({}, {}) top=({}, {}) activated_floating={} motions={}/{} target_changes=unsupported-wm-counters preview_updates=unsupported-wm-counters toggle_max=unsupported-wm-counters client_message_maximize=unsupported-wm-counters state_mutations=unsupported-wm-counters maximized_transitions=unsupported-wm-counters ewmh={} work_area={} selected_structure_notify={} ewmh_state_before={:?} ewmh_state_after={:?} ewmh_state_unchanged={} client_initial={} client_final={} frame_initial={} frame_final={} dimensions_stable={} native_configures={} preview_before={} preview_during={} native_route={} capture_route={} route_ok={} grab=unsupported-x-pointer-grab active={} process_health={} cleanup_release_after_assertions={} assertions={} unsupported={} path={}",
            setup.client.id,
            setup.start.0,
            setup.start.1,
            setup.top.0,
            setup.top.1,
            setup.activated_floating,
            hold.motion_count,
            TOP_HOLD_MOTIONS,
            setup.ewmh,
            work_area_detail(setup.work_area),
            setup.selected_structure_notify,
            setup.initial_state,
            hold.final_state,
            ewmh_state_unchanged,
            x03_rect(&setup.client),
            hold.final_client
                .as_ref()
                .map_or_else(|| "gone".to_owned(), |client| x03_rect(client)),
            x03_rect(&setup.frame),
            hold.final_frame
                .as_ref()
                .map_or_else(|| "gone".to_owned(), |frame| x03_rect(frame)),
            hold.dimensions_stable,
            hold.native_configures,
            setup.initial_preview,
            hold.preview_visible,
            hold.native_route
                .map_or_else(|| "none".to_owned(), |id| id.to_string()),
            hold.capture_route,
            route_ok,
            hold.active,
            process_health_detail(hold.process_health),
            cleanup_release,
            pass,
            unsupported_assertions(),
            InputPath::RealPointer.as_str(),
        ),
    )
}

fn run_x05(canary: &crate::Canary, args: &[String]) -> (bool, String) {
    let setup = match begin_top_hold(canary, args) {
        Ok(setup) => setup,
        Err(error) => return (false, format!("scenario=X05 {error}")),
    };
    let hold = observe_top_hold(canary, &setup);
    let route_ok = hold.capture_route
        || hold.native_route == Some(setup.client.id)
        || hold.native_route == Some(setup.frame.id);
    let hold_floating = setup.activated_floating
        && hold.motion_count >= TOP_HOLD_MOTIONS
        && hold.identity_stable
        && hold.dimensions_stable
        && hold.state_stable
        && !hold.maximized_seen
        && setup.ewmh
        && setup.selected_structure_notify
        && hold.active
        && route_ok
        && hold.process_health == Some(true);
    // This is the only release in X05.  It is intentionally after the full
    // top-zone hold observation so the release is the commit under test.
    let release_count = 1;
    let released_once = xtest_button_release(canary, setup.top.0, setup.top.1, 1);
    let committed_maximize =
        released_once && wait_until(500, || maximized_both(canary, setup.client.id));
    let after_client = canary.by_id(setup.client.id);
    let after_frame = canary
        .by_id(setup.frame.id)
        .filter(|window| window.parent == canary.root);
    let after_state = wm_state(canary, setup.client.id);
    let work_area_geometry = setup.work_area.is_some_and(|(x, y, width, height)| {
        after_frame.as_ref().is_some_and(|frame| {
            i32::from(frame.x) == x
                && i32::from(frame.y) == y
                && u32::from(frame.width) == width
                && u32::from(frame.height) == height
        })
    });
    let left_snap = match (setup.work_area, after_frame.as_ref()) {
        (Some((_, _, width, _)), Some(frame)) => Some(u32::from(frame.width) < width),
        _ => None,
    };
    let preview_hidden = preview_visible(canary).is_none();
    let active = active_target(canary, setup.client.id);
    let health = process_health(canary, setup.client.id);
    let native = after_client
        .as_ref()
        .map_or(0, |client| native_configures(canary, client, 250));
    let assertions = hold_floating
        && released_once
        && committed_maximize
        && work_area_geometry
        && left_snap == Some(false)
        && preview_hidden
        && active
        && health == Some(true)
        && native > 0;
    (
        assertions,
        format!(
            "scenario=X05 id={} start=({}, {}) top=({}, {}) hold_floating={} motions={}/{} release_count={} released_once={} target_changes=unsupported-wm-counters preview_updates=unsupported-wm-counters toggle_max=unsupported-wm-counters client_message_maximize=unsupported-wm-counters state_mutations=unsupported-wm-counters maximized_transitions=unsupported-wm-counters ewmh_before={:?} ewmh_after={:?} state_changed={} committed_maximize={} work_area={} final_frame={} work_area_geometry={} left_snap={:?} preview_during={} preview_hidden={} native_configures={} selected_structure_notify={} native_route={} capture_route={} route_ok={} grab=unsupported-x-pointer-grab active={} process_health={} assertions={} unsupported={} path={}",
            setup.client.id,
            setup.start.0,
            setup.start.1,
            setup.top.0,
            setup.top.1,
            hold_floating,
            hold.motion_count,
            TOP_HOLD_MOTIONS,
            release_count,
            released_once,
            setup.initial_state,
            after_state,
            setup.initial_state != after_state,
            committed_maximize,
            work_area_detail(setup.work_area),
            after_frame
                .as_ref()
                .map_or_else(|| "gone".to_owned(), |frame| x03_rect(frame)),
            work_area_geometry,
            left_snap,
            hold.preview_visible,
            preview_hidden,
            native,
            setup.selected_structure_notify,
            hold.native_route
                .map_or_else(|| "none".to_owned(), |id| id.to_string()),
            hold.capture_route,
            route_ok,
            active,
            process_health_detail(health),
            assertions,
            unsupported_assertions(),
            InputPath::RealPointer.as_str(),
        ),
    )
}

#[derive(Debug, Clone, Copy)]
enum ProbeSnapTarget {
    LeftHalf,
    RightHalf,
    TopHalf,
    BottomHalf,
    TopLeftQuarter,
    TopRightQuarter,
    BottomLeftQuarter,
    BottomRightQuarter,
}

impl ProbeSnapTarget {
    fn label(self) -> &'static str {
        match self {
            Self::LeftHalf => "LeftHalf",
            Self::RightHalf => "RightHalf",
            Self::TopHalf => "TopHalf",
            Self::BottomHalf => "BottomHalf",
            Self::TopLeftQuarter => "TopLeftQuarter",
            Self::TopRightQuarter => "TopRightQuarter",
            Self::BottomLeftQuarter => "BottomLeftQuarter",
            Self::BottomRightQuarter => "BottomRightQuarter",
        }
    }
}

const PROBE_SNAP_TARGETS: [ProbeSnapTarget; 8] = [
    ProbeSnapTarget::LeftHalf,
    ProbeSnapTarget::RightHalf,
    ProbeSnapTarget::TopHalf,
    ProbeSnapTarget::BottomHalf,
    ProbeSnapTarget::TopLeftQuarter,
    ProbeSnapTarget::TopRightQuarter,
    ProbeSnapTarget::BottomLeftQuarter,
    ProbeSnapTarget::BottomRightQuarter,
];

pub(crate) fn root_point(x: i32, y: i32) -> (i16, i16) {
    (
        x.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
        y.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
    )
}

fn probe_snap_geometry(
    target: ProbeSnapTarget,
    work_area: (i32, i32, u32, u32),
) -> (i32, i32, u32, u32) {
    let (x, y, width, height) = work_area;
    let left_width = width / 2;
    let right_width = width - left_width;
    let top_height = height / 2;
    let bottom_height = height - top_height;
    match target {
        ProbeSnapTarget::LeftHalf => (x, y, left_width, height),
        ProbeSnapTarget::RightHalf => (
            x + i32::try_from(left_width).unwrap_or(i32::MAX),
            y,
            right_width,
            height,
        ),
        ProbeSnapTarget::TopHalf => (x, y, width, top_height),
        ProbeSnapTarget::BottomHalf => (
            x,
            y + i32::try_from(top_height).unwrap_or(i32::MAX),
            width,
            bottom_height,
        ),
        ProbeSnapTarget::TopLeftQuarter => (x, y, left_width, top_height),
        ProbeSnapTarget::TopRightQuarter => (
            x + i32::try_from(left_width).unwrap_or(i32::MAX),
            y,
            right_width,
            top_height,
        ),
        ProbeSnapTarget::BottomLeftQuarter => (
            x,
            y + i32::try_from(top_height).unwrap_or(i32::MAX),
            left_width,
            bottom_height,
        ),
        ProbeSnapTarget::BottomRightQuarter => (
            x + i32::try_from(left_width).unwrap_or(i32::MAX),
            y + i32::try_from(top_height).unwrap_or(i32::MAX),
            right_width,
            bottom_height,
        ),
    }
}

fn probe_snap_point(target: ProbeSnapTarget, work_area: (i32, i32, u32, u32)) -> (i16, i16) {
    let (x, y, width, height) = work_area;
    let right = x + i32::try_from(width).unwrap_or(i32::MAX);
    let bottom = y + i32::try_from(height).unwrap_or(i32::MAX);
    let middle_x = x + i32::try_from(width / 2).unwrap_or(i32::MAX);
    let middle_y = y + i32::try_from(height / 2).unwrap_or(i32::MAX);
    root_point(
        match target {
            ProbeSnapTarget::LeftHalf
            | ProbeSnapTarget::TopLeftQuarter
            | ProbeSnapTarget::BottomLeftQuarter => x,
            ProbeSnapTarget::RightHalf
            | ProbeSnapTarget::TopRightQuarter
            | ProbeSnapTarget::BottomRightQuarter => right.saturating_sub(1),
            ProbeSnapTarget::TopHalf | ProbeSnapTarget::BottomHalf => middle_x,
        },
        match target {
            ProbeSnapTarget::LeftHalf | ProbeSnapTarget::RightHalf => middle_y,
            ProbeSnapTarget::TopHalf
            | ProbeSnapTarget::TopLeftQuarter
            | ProbeSnapTarget::TopRightQuarter => y,
            ProbeSnapTarget::BottomHalf
            | ProbeSnapTarget::BottomLeftQuarter
            | ProbeSnapTarget::BottomRightQuarter => bottom.saturating_sub(1),
        },
    )
}

fn frame_matches_probe_target(
    frame: &crate::WinInfo,
    target: ProbeSnapTarget,
    work_area: (i32, i32, u32, u32),
) -> bool {
    let (x, y, width, height) = probe_snap_geometry(target, work_area);
    i32::from(frame.x) == x
        && i32::from(frame.y) == y
        && u32::from(frame.width) == width
        && u32::from(frame.height) == height
}

fn frame_matches_geometry(frame: &crate::WinInfo, geometry: (i32, i32, u32, u32)) -> bool {
    i32::from(frame.x) == geometry.0
        && i32::from(frame.y) == geometry.1
        && u32::from(frame.width) == geometry.2
        && u32::from(frame.height) == geometry.3
}

fn probe_expected_geometry(
    target: ProbeSnapTarget,
    work_area: (i32, i32, u32, u32),
    floating: &crate::WinInfo,
    pointer_delta: (i32, i32),
) -> (i32, i32, u32, u32) {
    match target {
        // The canonical pointer policy maps top-center to Maximize and
        // bottom-center to None; neither is a TopHalf/BottomHalf commit.
        ProbeSnapTarget::TopHalf => work_area,
        ProbeSnapTarget::BottomHalf => {
            let (work_x, work_y, work_width, work_height) = work_area;
            let max_x = work_x.saturating_add(
                i32::try_from(work_width.saturating_sub(u32::from(floating.width)))
                    .unwrap_or(i32::MAX),
            );
            let max_y = work_y.saturating_add(
                i32::try_from(work_height.saturating_sub(u32::from(floating.height)))
                    .unwrap_or(i32::MAX),
            );
            let x = i32::from(floating.x)
                .saturating_add(pointer_delta.0)
                .clamp(work_x.min(max_x), max_x.max(work_x));
            let y = i32::from(floating.y)
                .saturating_add(pointer_delta.1)
                .clamp(work_y.min(max_y), max_y.max(work_y));
            (x, y, u32::from(floating.width), u32::from(floating.height))
        }
        _ => probe_snap_geometry(target, work_area),
    }
}

fn probe_policy_ok(
    canary: &crate::Canary,
    target: ProbeSnapTarget,
    client_id: u32,
    initial_state: &[u32],
    final_state: &[u32],
) -> bool {
    match target {
        ProbeSnapTarget::TopHalf => maximized_both(canary, client_id),
        ProbeSnapTarget::BottomHalf => {
            !maximized(canary, client_id) && initial_state == final_state
        }
        _ => !maximized(canary, client_id) && initial_state == final_state,
    }
}

fn frame_matches_any_probe_snap(frame: &crate::WinInfo, work_area: (i32, i32, u32, u32)) -> bool {
    PROBE_SNAP_TARGETS
        .into_iter()
        .any(|target| frame_matches_probe_target(frame, target, work_area))
}

pub(crate) fn establish_known_floating(
    canary: &crate::Canary,
    target: &crate::WinInfo,
    work_area: (i32, i32, u32, u32),
) -> bool {
    let Some(surface) = input_surface(canary, target) else {
        return false;
    };
    let needs_drag =
        maximized(canary, target.id) || frame_matches_any_probe_snap(&surface, work_area);
    if needs_drag {
        let start = title(&surface);
        let end = root_point(i32::from(start.0) + 64, i32::from(start.1) + 48);
        if !xtest_drag_path(canary, start, end, 1, 8) {
            return false;
        }
        if !wait_until(750, || {
            canary
                .by_id(target.id)
                .and_then(|client| input_surface(canary, &client))
                .is_some_and(|frame| {
                    !maximized(canary, target.id)
                        && !frame_matches_any_probe_snap(&frame, work_area)
                })
        }) {
            return false;
        }
    }
    canary
        .by_id(target.id)
        .and_then(|client| input_surface(canary, &client))
        .is_some_and(|frame| {
            !maximized(canary, target.id) && !frame_matches_any_probe_snap(&frame, work_area)
        })
}

fn run_x06(canary: &crate::Canary, args: &[String]) -> (bool, String) {
    let Some(work_area) = root_work_area(canary) else {
        return (
            false,
            "scenario=X06 work_area_unavailable path=real-pointer".to_owned(),
        );
    };
    let mut all_pass = true;
    let mut details = Vec::with_capacity(PROBE_SNAP_TARGETS.len());
    let mut floating_baseline: Option<(u16, u16, u16, u16)> = None;
    for target_kind in PROBE_SNAP_TARGETS {
        let Some(target) = super::window_interaction::resolve_target(canary, args) else {
            all_pass = false;
            details.push(format!(
                "scenario=X06 target={} target_not_ready path=real-pointer",
                target_kind.label()
            ));
            continue;
        };
        let client_id = target.id;
        let frame_id = target.parent;
        let reset_floating = establish_known_floating(canary, &target, work_area);
        let Some(before) = canary
            .by_id(client_id)
            .filter(|window| window.parent == frame_id)
        else {
            all_pass = false;
            details.push(format!(
                "scenario=X06 target={} client={} frame={} target_gone_after_reset",
                target_kind.label(),
                client_id,
                frame_id
            ));
            continue;
        };
        let Some(surface) = input_surface(canary, &before) else {
            all_pass = false;
            details.push(format!(
                "scenario=X06 target={} client={} frame={} frame_not_ready",
                target_kind.label(),
                client_id,
                frame_id
            ));
            continue;
        };
        let selected = prepare_native_measurement(canary, &before);
        let restore_geometry = match floating_baseline {
            Some((client_width, client_height, frame_width, frame_height)) => {
                before.width == client_width
                    && before.height == client_height
                    && surface.width == frame_width
                    && surface.height == frame_height
            }
            None => {
                floating_baseline =
                    Some((before.width, before.height, surface.width, surface.height));
                true
            }
        };
        let initial_state = wm_state(canary, client_id);
        let start = title(&surface);
        let destination = probe_snap_point(target_kind, work_area);
        let pointer_delta = (
            i32::from(destination.0) - i32::from(start.0),
            i32::from(destination.1) - i32::from(start.1),
        );
        let (dragged, native_route, capture_route, route_observation) =
            if !xtest_motion(canary, start.0, start.1)
                || !xtest_button_press(canary, start.0, start.1, 1)
            {
                (false, None, false, false)
            } else {
                let mut motions = true;
                for step in 1..=8 {
                    let step = i64::from(step);
                    let x = i64::from(start.0)
                        + (i64::from(destination.0) - i64::from(start.0)) * step / 8;
                    let y = i64::from(start.1)
                        + (i64::from(destination.1) - i64::from(start.1)) * step / 8;
                    if !xtest_motion(canary, x as i16, y as i16) {
                        motions = false;
                    }
                }
                // Sample the route while the drag session remains held.
                let native_route = x03_native_route(canary);
                let capture_route = native_route.is_some_and(|window| {
                    x03_interaction_capture_route(canary, window, canary.root_geometry())
                });
                let route_observation = capture_route
                    || native_route == Some(client_id)
                    || native_route == Some(frame_id);
                let released = xtest_button_release(canary, destination.0, destination.1, 1);
                (
                    motions && released,
                    native_route,
                    capture_route,
                    route_observation,
                )
            };
        let expected_geometry =
            probe_expected_geometry(target_kind, work_area, &surface, pointer_delta);
        let committed = wait_until(750, || {
            canary
                .by_id(frame_id)
                .is_some_and(|frame| frame_matches_geometry(&frame, expected_geometry))
                && probe_policy_ok(
                    canary,
                    target_kind,
                    client_id,
                    &initial_state,
                    &wm_state(canary, client_id),
                )
        });
        let final_client = canary
            .by_id(client_id)
            .filter(|window| window.parent == frame_id);
        let final_frame = canary
            .by_id(frame_id)
            .filter(|window| window.parent == canary.root);
        let geometry_ok = final_frame
            .as_ref()
            .is_some_and(|frame| frame_matches_geometry(frame, expected_geometry));
        let final_state = wm_state(canary, client_id);
        let policy_ok =
            probe_policy_ok(canary, target_kind, client_id, &initial_state, &final_state);
        let ewmh = ewmh_available(canary) && policy_ok;
        let preview_hidden = preview_visible(canary).is_none();
        let active = active_target(canary, client_id);
        let health = process_health(canary, client_id);
        let native = final_client
            .as_ref()
            .map_or(0, |client| native_configures(canary, client, 250));
        let identity_stable = final_client.is_some() && final_frame.is_some();
        let mode_inferred_snapped = geometry_ok && policy_ok;
        let actual_geometry = final_frame
            .as_ref()
            .map_or_else(|| "gone".to_owned(), x03_rect);
        let assertions = reset_floating
            && selected
            && restore_geometry
            && dragged
            && committed
            && identity_stable
            && mode_inferred_snapped
            && ewmh
            && preview_hidden
            && active
            && health == Some(true)
            && native > 0;
        let classification = if assertions { "PASS" } else { "FAIL" };
        all_pass &= assertions;
        details.push(format!(
            "scenario=X06 classification={} target={} client={} frame={} start=({}, {}) destination=({}, {}) reset_floating={} selected_structure_notify={} floating_restore_baseline={} dragged={} committed={} expected_policy={} expected_geometry={} actual_geometry={} mode=policy-geometry={} geometry_ok={} ewmh={} state_before={:?} state_after={:?} preview_hidden={} policy_ok={} active={} process_health={} native_configures={} native_route={} capture_route={} route_observation={} route_status=DIAGNOSTIC_UNSUPPORTED snap_transition=unsupported-wm-counters assertions={} path={}",
            classification,
            target_kind.label(),
            client_id,
            frame_id,
            start.0,
            start.1,
            destination.0,
            destination.1,
            reset_floating,
            selected,
            restore_geometry,
            dragged,
            committed,
            match target_kind {
                ProbeSnapTarget::TopHalf => "Maximize",
                ProbeSnapTarget::BottomHalf => "None",
                _ => "Snap",
            },
            format!(
                "{}x{}+{}+{}",
                expected_geometry.2,
                expected_geometry.3,
                expected_geometry.0,
                expected_geometry.1
            ),
            actual_geometry,
            mode_inferred_snapped,
            geometry_ok,
            ewmh,
            initial_state,
            final_state,
            preview_hidden,
            policy_ok,
            active,
            process_health_detail(health),
            native,
            native_route.map_or_else(|| "none".to_owned(), |id| id.to_string()),
            capture_route,
            route_observation,
            assertions,
            InputPath::RealPointer.as_str(),
        ));
    }
    (
        all_pass,
        format!(
            "scenario=X06 classification={} targets={}/8 assertions={} details=[{}]",
            if all_pass { "PASS" } else { "FAIL" },
            details.len(),
            all_pass,
            details.join("; ")
        ),
    )
}

fn run_x07(canary: &crate::Canary, args: &[String]) -> (bool, String) {
    let Some(work_area) = root_work_area(canary) else {
        return (
            false,
            "scenario=X07 work_area_unavailable path=real-pointer".to_owned(),
        );
    };
    let Some(target) = super::window_interaction::resolve_target(canary, args) else {
        return (
            false,
            "scenario=X07 target_not_ready path=real-pointer".to_owned(),
        );
    };
    let client_id = target.id;
    let frame_id = target.parent;
    let reset_floating = establish_known_floating(canary, &target, work_area);
    let Some(initial_client) = canary
        .by_id(client_id)
        .filter(|window| window.parent == frame_id)
    else {
        return (
            false,
            format!(
                "scenario=X07 client={} frame={} target_gone_after_reset",
                client_id, frame_id
            ),
        );
    };
    let Some(initial_frame) = input_surface(canary, &initial_client) else {
        return (
            false,
            format!(
                "scenario=X07 client={} frame={} frame_not_ready",
                client_id, frame_id
            ),
        );
    };
    let selected = prepare_native_measurement(canary, &initial_client);
    let initial_state = wm_state(canary, client_id);
    let snap_start = title(&initial_frame);
    let snap_destination = probe_snap_point(ProbeSnapTarget::LeftHalf, work_area);
    let snap_dragged = xtest_motion(canary, snap_start.0, snap_start.1)
        && xtest_drag_path(canary, snap_start, snap_destination, 1, 8);
    let snap_committed = wait_until(750, || {
        canary.by_id(frame_id).is_some_and(|frame| {
            frame_matches_probe_target(&frame, ProbeSnapTarget::LeftHalf, work_area)
        })
    });
    let Some(snapped_client) = canary
        .by_id(client_id)
        .filter(|window| window.parent == frame_id)
    else {
        return (
            false,
            format!(
                "scenario=X07 client={} frame={} snap_dragged={} snap_committed={} snapped_client_missing",
                client_id, frame_id, snap_dragged, snap_committed
            ),
        );
    };
    let Some(snapped_frame) = input_surface(canary, &snapped_client) else {
        return (
            false,
            format!(
                "scenario=X07 client={} frame={} snap_dragged={} snap_committed={} snapped_frame_missing",
                client_id, frame_id, snap_dragged, snap_committed
            ),
        );
    };
    let snap_mode =
        frame_matches_probe_target(&snapped_frame, ProbeSnapTarget::LeftHalf, work_area)
            && !maximized(canary, client_id);
    let snap_native = native_configures(canary, &snapped_client, 250);
    let _ = native_configures(canary, &snapped_client, 0);
    let press_point = title(&snapped_frame);
    let pressed = xtest_motion(canary, press_point.0, press_point.1)
        && xtest_button_press(canary, press_point.0, press_point.1, 1);
    if !pressed {
        return (
            false,
            format!(
                "scenario=X07 client={} frame={} snap_mode={} snap_dragged={} snap_committed={} pressed=false",
                client_id, frame_id, snap_mode, snap_dragged, snap_committed
            ),
        );
    }
    let jitter_points = [
        root_point(i32::from(press_point.0) + 1, i32::from(press_point.1) + 1),
        root_point(i32::from(press_point.0) - 2, i32::from(press_point.1) + 2),
        root_point(i32::from(press_point.0) + 3, i32::from(press_point.1) - 1),
    ];
    let mut jittered = true;
    let mut jitter_stable = true;
    for point in jitter_points {
        if !xtest_motion(canary, point.0, point.1) {
            jittered = false;
            break;
        }
        let stable = canary
            .by_id(client_id)
            .filter(|client| client.parent == frame_id)
            .and_then(|client| input_surface(canary, &client))
            .is_some_and(|frame| {
                frame.x == snapped_frame.x
                    && frame.y == snapped_frame.y
                    && frame.width == snapped_frame.width
                    && frame.height == snapped_frame.height
                    && !maximized(canary, client_id)
                    && preview_visible(canary).is_none()
            });
        jitter_stable &= stable;
    }
    let jitter_native = native_configures(canary, &snapped_client, 250);
    if !jittered {
        let _ = xtest_button_release(canary, press_point.0, press_point.1, 1);
        return (
            false,
            format!(
                "scenario=X07 client={} frame={} snap_mode={} snap_dragged={} snap_committed={} pressed=true jittered=false release_cleanup=true",
                client_id, frame_id, snap_mode, snap_dragged, snap_committed
            ),
        );
    }
    let away = root_point(i32::from(press_point.0) + 96, i32::from(press_point.1) + 64);
    let activated_motion = xtest_motion(canary, away.0, away.1);
    let activated = activated_motion
        && wait_until(750, || {
            let Some(client) = canary.by_id(client_id).filter(|w| w.parent == frame_id) else {
                return false;
            };
            let Some(frame) = input_surface(canary, &client) else {
                return false;
            };
            !maximized(canary, client_id)
                && client.width == initial_client.width
                && client.height == initial_client.height
                && frame.width == initial_frame.width
                && frame.height == initial_frame.height
                && (frame.x != snapped_frame.x || frame.y != snapped_frame.y)
                && !frame_matches_any_probe_snap(&frame, work_area)
        });
    let activation_native = native_configures(canary, &snapped_client, 250);
    let Some(activated_frame) = canary
        .by_id(client_id)
        .filter(|client| client.parent == frame_id)
        .and_then(|client| input_surface(canary, &client))
    else {
        let _ = xtest_button_release(canary, away.0, away.1, 1);
        return (
            false,
            format!(
                "scenario=X07 client={} frame={} snap_mode={} jittered=true jitter_stable={} activated=false release_cleanup=true",
                client_id, frame_id, snap_mode, jitter_stable
            ),
        );
    };
    let later = root_point(i32::from(away.0) + 20, i32::from(away.1) + 12);
    let later_motion = xtest_motion(canary, later.0, later.1);
    let later_position_only = later_motion
        && wait_until(500, || {
            canary
                .by_id(client_id)
                .filter(|client| client.parent == frame_id)
                .and_then(|client| input_surface(canary, &client))
                .is_some_and(|frame| {
                    frame.width == activated_frame.width
                        && frame.height == activated_frame.height
                        && (frame.x != activated_frame.x || frame.y != activated_frame.y)
                        && !maximized(canary, client_id)
                        && !frame_matches_any_probe_snap(&frame, work_area)
                })
        });
    let later_native = native_configures(canary, &snapped_client, 250);
    let before_release = canary
        .by_id(client_id)
        .filter(|client| client.parent == frame_id)
        .and_then(|client| input_surface(canary, &client))
        .is_some_and(|frame| {
            !maximized(canary, client_id)
                && !frame_matches_any_probe_snap(&frame, work_area)
                && preview_visible(canary).is_none()
        });
    let released_once = xtest_button_release(canary, later.0, later.1, 1);
    let final_ready = wait_until(500, || {
        canary
            .by_id(client_id)
            .filter(|client| client.parent == frame_id)
            .and_then(|client| input_surface(canary, &client))
            .is_some_and(|frame| !maximized(canary, client_id))
    });
    let final_client = canary
        .by_id(client_id)
        .filter(|client| client.parent == frame_id);
    let final_frame = final_client
        .as_ref()
        .and_then(|client| input_surface(canary, client));
    let identity_stable = final_client.is_some() && final_frame.is_some();
    let restore_dimensions = final_client.as_ref().is_some_and(|client| {
        client.width == initial_client.width && client.height == initial_client.height
    }) && final_frame.as_ref().is_some_and(|frame| {
        frame.width == initial_frame.width && frame.height == initial_frame.height
    });
    let position_changed = final_frame
        .as_ref()
        .is_some_and(|frame| frame.x != snapped_frame.x || frame.y != snapped_frame.y);
    let no_resnap = final_frame
        .as_ref()
        .is_some_and(|frame| !frame_matches_any_probe_snap(frame, work_area));
    let final_state = wm_state(canary, client_id);
    let ewmh = ewmh_available(canary) && initial_state == final_state;
    let preview_hidden = preview_visible(canary).is_none();
    let active = active_target(canary, client_id);
    let health = process_health(canary, client_id);
    let native_route = x03_native_route(canary);
    let capture_route = native_route.is_some_and(|window| {
        x03_interaction_capture_route(canary, window, canary.root_geometry())
    });
    let route_ok =
        capture_route || native_route == Some(client_id) || native_route == Some(frame_id);
    let full_configure_observed = activation_native > 0 && restore_dimensions;
    let position_only_observed = later_native > 0 && later_position_only;
    let assertions = reset_floating
        && selected
        && snap_dragged
        && snap_committed
        && snap_mode
        && pressed
        && jittered
        && jitter_stable
        && activated_motion
        && activated
        && full_configure_observed
        && position_only_observed
        && before_release
        && released_once
        && final_ready
        && identity_stable
        && restore_dimensions
        && position_changed
        && no_resnap
        && ewmh
        && preview_hidden
        && active
        && health == Some(true)
        && route_ok;
    (
        assertions,
        format!(
            "scenario=X07 client={} frame={} reset_floating={} selected_structure_notify={} snap_start=({}, {}) snap_destination=({}, {}) snap_dragged={} snap_committed={} snapped_mode=geometry-inferred-snapped={} snap_native_configures={} pressed={} jittered={} jitter_stable={} jitter_native_configures={} activation_motion={} activated={} activation_native_configures={} full_configure_once=unsupported-native-effect-stream full_configure_observed={} later_motion={} position_only={} later_native_configures={} before_release_no_resnap={} released_once={} final_ready={} restore_dimensions={} position_changed={} no_resnap={} identity_stable={} ewmh={} state_before={:?} state_after={:?} preview_hidden={} active={} process_health={} native_route={} capture_route={} route_ok={} snapped_to_floating=one-inferred-transition snap_transition=unsupported-wm-counters x-pointer-grab=unsupported assertions={} path={}",
            client_id,
            frame_id,
            reset_floating,
            selected,
            snap_start.0,
            snap_start.1,
            snap_destination.0,
            snap_destination.1,
            snap_dragged,
            snap_committed,
            snap_mode,
            snap_native,
            pressed,
            jittered,
            jitter_stable,
            jitter_native,
            activated_motion,
            activated,
            activation_native,
            full_configure_observed,
            later_motion,
            position_only_observed,
            later_native,
            before_release,
            released_once,
            final_ready,
            restore_dimensions,
            position_changed,
            no_resnap,
            identity_stable,
            ewmh,
            initial_state,
            final_state,
            preview_hidden,
            active,
            process_health_detail(health),
            native_route.map_or_else(|| "none".to_owned(), |id| id.to_string()),
            capture_route,
            route_ok,
            assertions,
            InputPath::RealPointer.as_str(),
        ),
    )
}

/// X05-X09: title drag plus the four bounded resize representatives.  The
/// optional repeat count is a small matrix/soak hook, capped to keep display
/// lifetime and event volume bounded.
pub fn run(canary: &crate::Canary, args: &[String]) -> (bool, String) {
    if let Some(phase) = crate::arg(args, "--phase") {
        return match phase.to_ascii_lowercase().as_str() {
            "x01" => run_x01(canary, args),
            "x02" => run_x02(canary, args),
            "x03" => run_x03(canary, args),
            "x04" => run_x04(canary, args),
            "x05" => run_x05(canary, args),
            "x06" => run_x06(canary, args),
            "x07" => run_x07(canary, args),
            other => (
                false,
                format!("unknown_phase={other} expected=x01|x02|x03|x04|x05|x06|x07"),
            ),
        };
    }
    let Some(target) = super::window_interaction::resolve_target(canary, args) else {
        return (false, "no target path=real-pointer".to_owned());
    };
    let repeats = crate::arg(args, "--repeats")
        .or_else(|| crate::arg(args, "--cycles"))
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(1)
        .clamp(1, 3);
    let mut all_pass = true;
    let mut details = Vec::new();
    for cycle in 1..=repeats {
        let (pass, detail) = run_drag_max_probe(canary, args);
        all_pass &= pass;
        details.push(format!("cycle={cycle} {detail}"));
    }
    let cases = [
        Case {
            id: "X05",
            name: "titlebar-drag",
            region: "TitleDrag",
            start: title,
            delta: (64, 48),
        },
        Case {
            id: "X06",
            name: "left/L",
            region: "Resize(left)",
            start: left,
            delta: (-24, 0),
        },
        Case {
            id: "X07",
            name: "TL",
            region: "Resize(top_left)",
            start: top_left,
            delta: (-20, -20),
        },
        Case {
            id: "X08",
            name: "BL",
            region: "Resize(bottom_left)",
            start: bottom_left,
            delta: (-20, 20),
        },
        Case {
            id: "X09",
            name: "BR",
            region: "Resize(bottom_right)",
            start: bottom_right,
            delta: (20, 20),
        },
    ];
    for case in cases {
        let Some(before) = canary.by_id(target.id) else {
            all_pass = false;
            details.push(format!(
                "scenario={} case={} target_gone",
                case.id, case.name
            ));
            continue;
        };
        let Some(surface) = input_surface(canary, &before) else {
            all_pass = false;
            details.push(format!(
                "scenario={} case={} frame_not_ready",
                case.id, case.name
            ));
            continue;
        };
        let selected = prepare_native_measurement(canary, &before);
        let session_start = (case.start)(&surface);
        let before_max = maximized(canary, target.id);
        let before_state = wm_state(canary, target.id);
        let source = source_at(canary, session_start.0, session_start.1);
        let source_xid = source.as_ref().map_or(target.id, |w| w.id);
        let end = (
            session_start.0.saturating_add(case.delta.0),
            session_start.1.saturating_add(case.delta.1),
        );
        let _ = native_configures(canary, &before, 0);
        let dragged = xtest_motion(canary, session_start.0, session_start.1)
            && xtest_drag_path(canary, session_start, end, 1, 8);
        let _ = wait_until(500, || {
            canary
                .by_id(target.id)
                .is_some_and(|after| expected(&case, &before, &after))
        });
        let after = canary.by_id(target.id);
        let after_max = maximized(canary, target.id);
        let after_state = wm_state(canary, target.id);
        let geometry_ok = after.as_ref().is_some_and(|w| expected(&case, &before, w));
        let resize_no_snap = before_max == after_max;
        let native = native_configures(canary, &before, 250);
        let ewmh = ewmh_available(canary) && before_state == after_state;
        let active = active_target(canary, target.id);
        let preview = preview_visible(canary).is_none();
        let health = process_health(canary, target.id);
        let assertions = dragged
            && selected
            && geometry_ok
            && native > 0
            && ewmh
            && active
            && resize_no_snap
            && preview
            && health == Some(true)
            && unsupported_assertions().is_empty();
        all_pass &= assertions;
        let final_geometry = after.map_or_else(
            || "gone".to_owned(),
            |w| format!("{}x{}+{}+{}", w.width, w.height, w.x, w.y),
        );
        details.push(format!(
            "scenario={} case={} source_xid={} region={} button=1 target_xid={} session_start=({}, {}) selected_structure_notify={} dragged={} native_configure_window_calls={} final={} geometry_ok={} ewmh={} active={} max_before={} max_after={} resize_no_snap={} preview_hidden={} process_health={} assertions={} unsupported={} path={}",
            case.id,
            case.name,
            source_xid,
            case.region,
            source_xid,
            session_start.0,
            session_start.1,
            selected,
            dragged,
            native,
            final_geometry,
            geometry_ok,
            ewmh,
            active,
            before_max,
            after_max,
            resize_no_snap,
            preview,
            process_health_detail(health),
            assertions,
            unsupported_assertions(),
            InputPath::RealPointer.as_str()
        ));
    }
    (
        all_pass,
        format!(
            "id={} matrix_repeats={} {}",
            target.id,
            repeats,
            details.join("; ")
        ),
    )
}
