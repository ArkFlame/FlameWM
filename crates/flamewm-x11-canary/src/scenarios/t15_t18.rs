//! T15-T18 deterministic native canaries.
//!
//! These probes only claim observations available at the X11 boundary:
//! managed-window identity, geometry, EWMH state, XTEST delivery, and pixels
//! returned by `GetImage`.  In particular, T18 is admitted as a static
//! scenario only because desktop selection is rendered inside a client
//! surface; this canary has no honest protocol-level observation of its DOM
//! state.

use std::time::{Duration, Instant};

use super::interaction_regressions::{
    bottom_right, maximized, maximized_both, native_configures, root_point, root_work_area, title,
    wait_until, wm_state,
};
use super::{xtest_button_press, xtest_button_release, xtest_drag_path, xtest_motion};
use x11rb::connection::Connection as _;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    ChangeWindowAttributesAux, ConfigureWindowAux, ConnectionExt as _, EventMask,
};

const WAIT_MS: u64 = 750;
const MIXED_MOTIONS: usize = 24;
const T18_NATIVE_SEMANTIC_OBSERVABLE: bool = false;
const T18_LIMITATION: &str = "desktop-selection-dom-state-is-inside-the-client-surface-and-has-no-native-x11-child-or-property";

fn same_frame_read(left: &crate::WinInfo, right: &crate::WinInfo) -> bool {
    left.id == right.id
        && left.parent == right.parent
        && left.x == right.x
        && left.y == right.y
        && left.width == right.width
        && left.height == right.height
        && left.map_state == right.map_state
}

fn wait_stable_frame<F>(timeout_ms: u64, mut read: F) -> Option<crate::WinInfo>
where
    F: FnMut() -> Option<crate::WinInfo>,
{
    let mut previous = None;
    let mut stable = None;
    let found = {
        let mut ready = || {
            let Some(current) = read() else {
                previous = None;
                return false;
            };
            let unchanged = previous
                .as_ref()
                .is_some_and(|previous| same_frame_read(previous, &current));
            previous = Some(current.clone());
            if unchanged {
                stable = Some(current);
                true
            } else {
                false
            }
        };
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            if ready() {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    };
    found.then_some(stable).flatten()
}

fn input_surface(canary: &crate::Canary, client: &crate::WinInfo) -> Option<crate::WinInfo> {
    canary.by_id(client.parent).filter(|frame| {
        frame.parent == canary.root
            && client.map_state == 2
            && frame.map_state == 2
            && client.width > 0
            && client.height > 0
            && frame.width > 0
            && frame.height > 0
    })
}

fn target(canary: &crate::Canary, args: &[String]) -> Option<crate::WinInfo> {
    super::window_interaction::resolve_target(canary, args)
}

fn left_snap_geometry(area: (i32, i32, u32, u32)) -> (i32, i32, u32, u32) {
    (area.0, area.1, area.2 / 2, area.3)
}

fn frame_matches(frame: &crate::WinInfo, geometry: (i32, i32, u32, u32)) -> bool {
    i32::from(frame.x) == geometry.0
        && i32::from(frame.y) == geometry.1
        && u32::from(frame.width) == geometry.2
        && u32::from(frame.height) == geometry.3
}

fn geom_of(frame: &crate::WinInfo) -> (i32, i32, u32, u32) {
    (
        i32::from(frame.x),
        i32::from(frame.y),
        u32::from(frame.width),
        u32::from(frame.height),
    )
}

fn clamp_i16(value: i32) -> i16 {
    value.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

fn configure_size(canary: &crate::Canary, window: u32, width: u16, height: u16) -> Option<()> {
    canary
        .conn
        .configure_window(
            window,
            &ConfigureWindowAux::new()
                .width(u32::from(width))
                .height(u32::from(height)),
        )
        .ok()?
        .check()
        .ok()?;
    canary.conn.flush().ok()
}

fn t17_geometry(frame: &crate::WinInfo) -> String {
    t17_geometry_values(geom_of(frame))
}

fn t17_geometry_values(geometry: (i32, i32, u32, u32)) -> String {
    format!(
        "{}x{}+{}+{}",
        geometry.2, geometry.3, geometry.0, geometry.1
    )
}

fn t17_atom_values(values: &[u32]) -> String {
    let shown = values
        .iter()
        .take(8)
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let remaining = values.len().saturating_sub(8);
    if remaining == 0 {
        format!("[{shown}]")
    } else {
        format!("[{shown},...+{remaining}]")
    }
}

fn t17_current_workarea(canary: &crate::Canary) -> String {
    root_work_area(canary).map_or_else(|| "unavailable".to_owned(), t17_geometry_values)
}

fn t17_optional_geometry(frame: Option<&crate::WinInfo>) -> String {
    frame.map_or_else(|| "unavailable".to_owned(), t17_geometry)
}

fn t17_configures(canary: &crate::Canary, frame: Option<&crate::WinInfo>) -> usize {
    frame.map_or(0, |frame| matching_frame_configures(canary, frame, 250))
}

#[derive(Default)]
struct T17WindowProbe {
    geometry: String,
    attributes: String,
    tree: String,
    geometry_values: Option<(i16, i16, u16, u16)>,
    map_state: Option<u8>,
    parent: Option<u32>,
    x11_error: bool,
}

fn t17_bound(value: String) -> String {
    value.chars().take(160).collect()
}

fn t17_probe_reply<C, R, E1: std::fmt::Display, E2: std::fmt::Display>(
    sent: Result<C, E1>,
    reply: impl FnOnce(C) -> Result<R, E2>,
) -> Result<R, String> {
    match sent {
        Err(error) => Err(format!("error=request:{}", t17_bound(error.to_string()))),
        Ok(cookie) => {
            reply(cookie).map_err(|error| format!("error=reply:{}", t17_bound(error.to_string())))
        }
    }
}

fn t17_window_probe(canary: &crate::Canary, window: u32) -> T17WindowProbe {
    let mut probe = T17WindowProbe::default();
    match t17_probe_reply(
        canary.conn.get_geometry(window),
        x11rb::cookie::Cookie::reply,
    ) {
        Err(error) => {
            probe.x11_error = true;
            probe.geometry = error;
        }
        Ok(reply) => {
            probe.geometry_values = Some((reply.x, reply.y, reply.width, reply.height));
            probe.geometry = format!(
                "ok=x:{} y:{} width:{} height:{}",
                reply.x, reply.y, reply.width, reply.height
            );
        }
    }
    match t17_probe_reply(
        canary.conn.get_window_attributes(window),
        x11rb::cookie::Cookie::reply,
    ) {
        Err(error) => {
            probe.x11_error = true;
            probe.attributes = error;
        }
        Ok(reply) => {
            let map_state = u8::from(reply.map_state);
            probe.map_state = Some(map_state);
            probe.attributes = format!(
                "ok=map_state:{} override_redirect:{}",
                map_state, reply.override_redirect
            );
        }
    }
    match t17_probe_reply(canary.conn.query_tree(window), x11rb::cookie::Cookie::reply) {
        Err(error) => {
            probe.x11_error = true;
            probe.tree = error;
        }
        Ok(reply) => {
            probe.parent = Some(reply.parent);
            let children: Vec<String> = reply
                .children
                .iter()
                .take(8)
                .map(ToString::to_string)
                .collect();
            probe.tree = format!(
                "ok=parent:{} children:[{}]",
                reply.parent,
                children.join(",")
            );
        }
    }
    probe
}

fn t17_snapshot(canary: &crate::Canary) -> String {
    let desktop = canary
        .prop_u32(canary.root, "_NET_CURRENT_DESKTOP")
        .first()
        .map_or_else(
            || "unavailable".to_owned(),
            std::string::ToString::to_string,
        );
    let area = root_work_area(canary).map_or_else(
        || "unavailable".to_owned(),
        |(x, y, width, height)| format!("x:{x} y:{y} width:{width} height:{height}"),
    );
    let active = canary
        .active_window()
        .map_or_else(|| "unavailable".to_owned(), |window| window.to_string());
    format!("desktop:{desktop} workarea:[{area}] active:{active}")
}

#[derive(Clone, Copy)]
enum T17GeometryExpectation {
    Move {
        x: i16,
        y: i16,
        width: u16,
        height: u16,
    },
    Resize {
        x: i16,
        y: i16,
        width: u16,
        height: u16,
    },
    Exact((i32, i32, u32, u32)),
    Restore {
        width: u16,
        height: u16,
        x: i16,
        y: i16,
    },
}

fn t17_geometry_matches(
    values: Option<(i16, i16, u16, u16)>,
    expectation: T17GeometryExpectation,
) -> bool {
    let Some((x, y, width, height)) = values else {
        return false;
    };
    match expectation {
        T17GeometryExpectation::Move {
            x: initial_x,
            y: initial_y,
            width: initial_width,
            height: initial_height,
        } => {
            (x != initial_x || y != initial_y) && width == initial_width && height == initial_height
        }
        T17GeometryExpectation::Resize {
            x: initial_x,
            y: initial_y,
            width: initial_width,
            height: initial_height,
        } => (width > initial_width || height > initial_height) && x == initial_x && y == initial_y,
        T17GeometryExpectation::Exact(geometry) => {
            i32::from(x) == geometry.0
                && i32::from(y) == geometry.1
                && u32::from(width) == geometry.2
                && u32::from(height) == geometry.3
        }
        T17GeometryExpectation::Restore {
            width: expected_width,
            height: expected_height,
            x: expected_x,
            y: expected_y,
        } => {
            width == expected_width
                && height == expected_height
                && x == expected_x
                && y == expected_y
        }
    }
}

fn t17_expectation_name(expectation: Option<T17GeometryExpectation>) -> &'static str {
    match expectation {
        Some(T17GeometryExpectation::Move { .. }) => "moved_same_size",
        Some(T17GeometryExpectation::Resize { .. }) => "larger_same_position",
        Some(T17GeometryExpectation::Exact(_)) => "exact_geometry",
        Some(T17GeometryExpectation::Restore { .. }) => "restored_size_and_position",
        None => "none",
    }
}

#[allow(clippy::too_many_arguments)]
fn t17_failure(
    canary: &crate::Canary,
    row: &str,
    stage: &str,
    client_id: u32,
    frame_id: u32,
    failure: &str,
    expectation: Option<T17GeometryExpectation>,
    setup_failure: bool,
) -> String {
    let client = t17_window_probe(canary, client_id);
    let frame = t17_window_probe(canary, frame_id);
    let reason = if client.x11_error || frame.x11_error {
        "x11_lookup_query_error"
    } else if client.map_state != Some(2)
        || frame.map_state != Some(2)
        || client.geometry_values.is_none()
        || frame.geometry_values.is_none()
    {
        "absent_or_unmapped"
    } else if client.parent != Some(frame_id) || frame.parent != Some(canary.root) {
        "wrong_parent"
    } else if expectation
        .is_some_and(|expected| !t17_geometry_matches(frame.geometry_values, expected))
    {
        "geometry_mismatch"
    } else if setup_failure {
        "workspace_precondition"
    } else {
        "interaction_session"
    };
    format!(
        "scenario=T17 client={} frame={} row={} stage={} {} reason={} failure_diagnostic=row={} stage={} client={} frame={} reason={} expected={} frame_remains_mapped={} client_geometry=[{}] client_attributes=[{}] client_tree=[{}] frame_geometry=[{}] frame_attributes=[{}] frame_tree=[{}] state=[{}]",
        client_id,
        frame_id,
        row,
        stage,
        failure,
        reason,
        row,
        stage,
        client_id,
        frame_id,
        reason,
        t17_expectation_name(expectation),
        frame.map_state == Some(2),
        client.geometry,
        client.attributes,
        client.tree,
        frame.geometry,
        frame.attributes,
        frame.tree,
        t17_snapshot(canary),
    )
}

fn t17_setup_failure(
    canary: &crate::Canary,
    row: &str,
    stage: &str,
    client_id: u32,
    frame_id: u32,
) -> (bool, String) {
    (
        false,
        t17_failure(
            canary,
            row,
            stage,
            client_id,
            frame_id,
            "setup_failed",
            None,
            true,
        ),
    )
}

#[allow(clippy::too_many_arguments)]
fn t17_restore_diagnostic(
    canary: &crate::Canary,
    client_id: u32,
    frame_id: u32,
    baseline: &T17Baseline,
    restore_expected: &crate::WinInfo,
    max_frame: &crate::WinInfo,
    max_state: &[u32],
    restore_state: &[u32],
    maximized: bool,
    restored: bool,
    max_configures: usize,
    restore_configures: usize,
) -> String {
    format!(
        "restore_diagnostic=saved_baseline_client={} saved_baseline_frame={} maximize_frame={} restore_expected_frame={} restore_actual_frame={} maximized={} restored={} net_wm_state_maximize={} net_wm_state_restore={} current_workarea={} configure_acknowledgements_maximize={} configure_acknowledgements_restore={} client={} frame={}",
        t17_geometry(&baseline.client),
        t17_geometry(&baseline.frame),
        t17_geometry(max_frame),
        t17_geometry(restore_expected),
        t17_optional_geometry(canary.by_id(frame_id).as_ref()),
        maximized,
        restored,
        t17_atom_values(max_state),
        t17_atom_values(restore_state),
        t17_current_workarea(canary),
        max_configures,
        restore_configures,
        client_id,
        frame_id,
    )
}

fn known_snap_geometries(area: (i32, i32, u32, u32)) -> [(i32, i32, u32, u32); 8] {
    let (x, y, width, height) = area;
    let left_width = width / 2;
    let right_width = width - left_width;
    let top_height = height / 2;
    let bottom_height = height - top_height;
    [
        (x, y, left_width, height),
        (x + left_width as i32, y, right_width, height),
        (x, y, width, top_height),
        (x, y + top_height as i32, width, bottom_height),
        (x, y, left_width, top_height),
        (x + left_width as i32, y, right_width, top_height),
        (x, y + top_height as i32, left_width, bottom_height),
        (
            x + left_width as i32,
            y + top_height as i32,
            right_width,
            bottom_height,
        ),
    ]
}

fn frame_matches_known_snap(frame: &crate::WinInfo, area: (i32, i32, u32, u32)) -> bool {
    known_snap_geometries(area)
        .into_iter()
        .any(|geometry| frame_matches(frame, geometry))
}

fn establish_known_floating(
    canary: &crate::Canary,
    target: &crate::WinInfo,
    work_area: (i32, i32, u32, u32),
) -> bool {
    let Some(surface) = input_surface(canary, target) else {
        return false;
    };
    let needs_drag = maximized(canary, target.id) || frame_matches_known_snap(&surface, work_area);
    if needs_drag {
        let start = title(&surface);
        let end = root_point(i32::from(start.0) + 64, i32::from(start.1) + 48);
        if !xtest_drag_path(canary, start, end, 1, 8) {
            return false;
        }
        if !wait_until(WAIT_MS, || {
            canary
                .by_id(target.id)
                .and_then(|client| input_surface(canary, &client))
                .is_some_and(|frame| {
                    !maximized(canary, target.id) && !frame_matches_known_snap(&frame, work_area)
                })
        }) {
            return false;
        }
    }
    canary
        .by_id(target.id)
        .and_then(|client| input_surface(canary, &client))
        .is_some_and(|frame| {
            !maximized(canary, target.id) && !frame_matches_known_snap(&frame, work_area)
        })
}

#[derive(Clone)]
struct T17Baseline {
    client: crate::WinInfo,
    frame: crate::WinInfo,
    work_area: (i32, i32, u32, u32),
}

fn t17_baseline_size(
    client: &crate::WinInfo,
    frame: &crate::WinInfo,
    work_area: (i32, i32, u32, u32),
) -> (u16, u16) {
    let frame_extra_width = u32::from(frame.width.saturating_sub(client.width));
    let frame_extra_height = u32::from(frame.height.saturating_sub(client.height));
    let roomy_width = work_area.2.saturating_mul(3) / 5;
    let roomy_height = work_area.3.saturating_mul(3) / 5;
    (
        client.width.min(
            roomy_width
                .saturating_sub(frame_extra_width)
                .clamp(1, u32::from(u16::MAX)) as u16,
        ),
        client.height.min(
            roomy_height
                .saturating_sub(frame_extra_height)
                .clamp(1, u32::from(u16::MAX)) as u16,
        ),
    )
}

fn establish_t17_baseline(
    canary: &crate::Canary,
    target: &crate::WinInfo,
    work_area: (i32, i32, u32, u32),
) -> Option<T17Baseline> {
    let client = canary.by_id(target.id)?;
    let frame = input_surface(canary, &client)?;
    let (width, height) = t17_baseline_size(&client, &frame, work_area);
    if client.width > width || client.height > height {
        configure_size(canary, client.id, width, height)?;
        if !wait_until(WAIT_MS, || {
            canary
                .by_id(client.id)
                .is_some_and(|current| current.width <= width && current.height <= height)
        }) {
            return None;
        }
    }
    let client = canary.by_id(target.id)?;
    let frame = input_surface(canary, &client)?;
    let centered_title = root_point(
        work_area.0.saturating_add((work_area.2 / 2) as i32),
        work_area
            .1
            .saturating_add(work_area.3.saturating_sub(u32::from(frame.height)) as i32 / 2)
            .saturating_add(8),
    );
    if !xtest_drag_path(canary, title(&frame), centered_title, 1, 8) {
        return None;
    }
    let ready = wait_until(WAIT_MS, || {
        canary
            .by_id(target.id)
            .and_then(|current| input_surface(canary, &current))
            .is_some_and(|current| {
                let center_x = i32::from(current.x) + i32::from(current.width) / 2;
                let center_y = i32::from(current.y) + i32::from(current.height) / 2;
                let area_center_x = work_area.0 + (work_area.2 / 2) as i32;
                let area_center_y = work_area.1 + (work_area.3 / 2) as i32;
                !maximized(canary, target.id)
                    && (center_x - area_center_x).abs() <= 2
                    && (center_y - area_center_y).abs() <= 2
                    && i32::from(current.x) + i32::from(current.width) + 24
                        <= work_area.0 + work_area.2 as i32
                    && i32::from(current.y) + i32::from(current.height) + 16
                        <= work_area.1 + work_area.3 as i32
            })
    });
    if !ready {
        return None;
    }
    let client = canary.by_id(target.id)?;
    let frame = input_surface(canary, &client)?;
    Some(T17Baseline {
        client,
        frame,
        work_area,
    })
}

fn normalize_t17_baseline(
    canary: &crate::Canary,
    target: &crate::WinInfo,
    baseline: &T17Baseline,
) -> Option<(crate::WinInfo, crate::WinInfo)> {
    let client = canary.by_id(target.id)?;
    let _frame = input_surface(canary, &client)?;
    if client.width != baseline.client.width || client.height != baseline.client.height {
        configure_size(
            canary,
            client.id,
            baseline.client.width,
            baseline.client.height,
        )?;
        if !wait_until(WAIT_MS, || {
            canary.by_id(client.id).is_some_and(|current| {
                current.width == baseline.client.width && current.height == baseline.client.height
            })
        }) {
            return None;
        }
    }
    let client = canary.by_id(target.id)?;
    let frame = input_surface(canary, &client)?;
    if !frame_matches(&frame, geom_of(&baseline.frame)) {
        if !xtest_drag_path(canary, title(&frame), title(&baseline.frame), 1, 8) {
            return None;
        }
        if !wait_until(WAIT_MS, || {
            canary
                .by_id(target.id)
                .and_then(|current| input_surface(canary, &current))
                .is_some_and(|current| {
                    frame_matches(&current, geom_of(&baseline.frame))
                        && current.width == baseline.frame.width
                        && current.height == baseline.frame.height
                })
        }) {
            return None;
        }
    }
    let client = canary.by_id(target.id)?;
    let frame = input_surface(canary, &client)?;
    (client.width == baseline.client.width
        && client.height == baseline.client.height
        && frame_matches(&frame, geom_of(&baseline.frame)))
    .then_some((client, frame))
}

fn t17_resize_target(frame: &crate::WinInfo, work_area: (i32, i32, u32, u32)) -> (i16, i16) {
    let from = bottom_right(frame);
    let right = work_area.0 + work_area.2 as i32 - 2;
    let bottom = work_area.1 + work_area.3 as i32 - 2;
    root_point(
        (i32::from(from.0) + 24).min(right),
        (i32::from(from.1) + 16).min(bottom),
    )
}

fn matching_frame_configures(
    canary: &crate::Canary,
    frame: &crate::WinInfo,
    timeout_ms: u64,
) -> usize {
    let mut count = 0;
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        match canary.conn.poll_for_event() {
            Ok(Some(Event::ConfigureNotify(event)))
                if (event.event == frame.id || event.window == frame.id)
                    && event.x == frame.x
                    && event.y == frame.y
                    && event.width == frame.width
                    && event.height == frame.height =>
            {
                count += 1;
            }
            Ok(Some(_)) => {}
            Ok(None) => {
                if Instant::now() >= deadline {
                    return count;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return count,
        }
    }
}

fn t17_stage_start(
    canary: &crate::Canary,
    client_id: u32,
    baseline: &T17Baseline,
) -> Result<(bool, crate::WinInfo, crate::WinInfo), &'static str> {
    let target = canary.by_id(client_id).ok_or("target_gone_before_stage")?;
    let reset_floating = establish_known_floating(canary, &target, baseline.work_area);
    if !reset_floating {
        return Err("establish_floating=false");
    }
    let (client, frame) = normalize_t17_baseline(canary, &target, baseline)
        .ok_or("baseline_not_ready_after_stage_reset")?;
    let _ = native_configures(canary, &client, 0);
    Ok((reset_floating, client, frame))
}

fn hold_drag(canary: &crate::Canary, from: (i16, i16), to: (i16, i16), steps: usize) -> bool {
    if !xtest_motion(canary, from.0, from.1) || !xtest_button_press(canary, from.0, from.1, 1) {
        return false;
    }
    let steps = steps.max(1);
    for index in 1..=steps {
        let i = index as i64;
        let total = steps as i64;
        let x = i64::from(from.0) + (i64::from(to.0) - i64::from(from.0)) * i / total;
        let y = i64::from(from.1) + (i64::from(to.1) - i64::from(from.1)) * i / total;
        if !xtest_motion(canary, x as i16, y as i16) {
            let _ = xtest_button_release(canary, x as i16, y as i16, 1);
            return false;
        }
    }
    true
}

fn release(canary: &crate::Canary, point: (i16, i16)) -> bool {
    xtest_button_release(canary, point.0, point.1, 1)
}

fn select_structure_notify(canary: &crate::Canary, client: &crate::WinInfo) -> bool {
    let mask = ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY);
    [client.id, client.parent].into_iter().all(|window| {
        canary
            .conn
            .change_window_attributes(window, &mask)
            .is_ok_and(|cookie| cookie.check().is_ok())
    }) && canary.conn.flush().is_ok()
}

fn preview(canary: &crate::Canary) -> Option<crate::WinInfo> {
    canary
        .find("FlameWM snap preview")
        .into_iter()
        .find(|window| window.parent == canary.root && window.map_state == 2 && window.width > 0)
}

fn preview_border_only(canary: &crate::Canary, window: &crate::WinInfo) -> (usize, bool, bool) {
    let width = i16::try_from(window.width).unwrap_or(i16::MAX);
    let height = i16::try_from(window.height).unwrap_or(i16::MAX);
    if width < 8 || height < 8 {
        return (0, false, false);
    }
    let corners = [
        (0_i16, 0_i16),
        (width - 2, 0),
        (0, height - 2),
        (width - 2, height - 2),
    ];
    let mut delivered = 0;
    let mut accent_border = true;
    for (x, y) in corners {
        let Ok(data) = canary.sample(window.id, x, y, 2, 2) else {
            accent_border = false;
            continue;
        };
        delivered += 1;
        let (_, _, r, g, b) = crate::Canary::pixel_stats(&data);
        accent_border &= r > g.saturating_add(4) && r > b.saturating_add(4);
    }
    let center = canary
        .sample(window.id, width / 2 - 2, height / 2 - 2, 4, 4)
        .ok()
        .is_some_and(|data| crate::Canary::pixel_stats(&data).0);
    (delivered, accent_border, center)
}

/// T15: real titlebar drag to the left snap edge. The observable is a
/// root-owned, viewable preview with the expected native snap geometry, an
/// accent border, and no captured pixels in its clear center.
pub fn run_t15(canary: &crate::Canary, args: &[String]) -> (bool, String) {
    let Some(area) = root_work_area(canary) else {
        return (
            false,
            "scenario=T15 work_area_unavailable path=real-pointer".to_owned(),
        );
    };
    let Some(client) = target(canary, args) else {
        return (
            false,
            "scenario=T15 target_not_ready path=real-pointer".to_owned(),
        );
    };
    let Some(frame) = input_surface(canary, &client) else {
        return (
            false,
            format!(
                "scenario=T15 client={} frame_not_ready path=real-pointer",
                client.id
            ),
        );
    };
    let destination = (clamp_i16(area.0), clamp_i16(area.1 + (area.3 / 2) as i32));
    let start = title(&frame);
    let held = hold_drag(canary, start, destination, 8);
    let held_preview = held && wait_until(WAIT_MS, || preview(canary).is_some());
    let border_only_ready = held_preview
        && wait_until(WAIT_MS, || {
            preview(canary).is_some_and(|window| {
                let (delivered, accent_border, clear_center) = preview_border_only(canary, &window);
                delivered == 4 && accent_border && clear_center
            })
        });
    let preview_window = preview(canary);
    let geometry = preview_window
        .as_ref()
        .is_some_and(|window| frame_matches(window, left_snap_geometry(area)));
    let (delivered, accent_border, clear_center) =
        preview_window.as_ref().map_or((0, false, false), |window| {
            preview_border_only(canary, window)
        });
    let cleaned = release(canary, destination);
    let committed = cleaned
        && wait_until(WAIT_MS, || {
            canary
                .by_id(client.id)
                .and_then(|current| input_surface(canary, &current))
                .is_some_and(|current| frame_matches(&current, left_snap_geometry(area)))
        });
    let hidden = wait_until(WAIT_MS, || preview(canary).is_none());
    let pass = held_preview
        && border_only_ready
        && geometry
        && delivered == 4
        && accent_border
        && clear_center
        && cleaned
        && committed
        && hidden;
    (
        pass,
        format!(
            "scenario=T15 client={} preview_native={} held_preview={} border_only_ready={} geometry_left_half={} border_samples={delivered}/4 accent_border={} clear_center_no_backdrop={} cleanup_release={} snap_committed={} preview_hidden={} assertions={} observable=native-border-only-preview-and-snap-commit failure_mutation=preview-geometry-border-backdrop-or-commit-change path=real-pointer",
            client.id,
            preview_window
                .as_ref()
                .is_some_and(|window| window.parent == canary.root),
            held_preview,
            border_only_ready,
            geometry,
            accent_border,
            clear_center,
            cleaned,
            committed,
            hidden,
            pass
        ),
    )
}

/// T16: mixed titlebar and resize input.  Every XTEST motion is acknowledged
/// by the checked real-input request, then both geometry effects must become
/// observable in a bounded interval.  This deliberately avoids private
/// frame-time claims.
pub fn run_t16(canary: &crate::Canary, args: &[String]) -> (bool, String) {
    let Some(client) = target(canary, args) else {
        return (
            false,
            "scenario=T16 target_not_ready path=real-pointer".to_owned(),
        );
    };
    let Some(initial_frame) = input_surface(canary, &client) else {
        return (
            false,
            format!("scenario=T16 client={} frame_not_ready", client.id),
        );
    };
    let move_from = title(&initial_frame);
    let move_to = (
        move_from.0.saturating_add(48),
        move_from.1.saturating_add(32),
    );
    let started = Instant::now();
    let moved_input = hold_drag(canary, move_from, move_to, MIXED_MOTIONS / 2);
    let move_released = release(canary, move_to);
    let moved = wait_until(WAIT_MS, || {
        canary
            .by_id(client.id)
            .and_then(|current| input_surface(canary, &current))
            .is_some_and(|frame| frame.x != initial_frame.x || frame.y != initial_frame.y)
    });
    let Some(after_move) = canary
        .by_id(client.id)
        .and_then(|current| input_surface(canary, &current))
    else {
        return (
            false,
            format!("scenario=T16 client={} gone_after_move", client.id),
        );
    };
    let resize_from = bottom_right(&after_move);
    let resize_to = (
        resize_from.0.saturating_add(24),
        resize_from.1.saturating_add(16),
    );
    let resized_input = hold_drag(canary, resize_from, resize_to, MIXED_MOTIONS / 2);
    let resize_released = release(canary, resize_to);
    let resized = wait_until(WAIT_MS, || {
        canary
            .by_id(client.id)
            .is_some_and(|current| current.width > client.width || current.height > client.height)
    });
    let elapsed_ms = started.elapsed().as_millis();
    let pass = moved_input
        && move_released
        && moved
        && resized_input
        && resize_released
        && resized
        && elapsed_ms <= 2_000;
    (
        pass,
        format!(
            "scenario=T16 client={} mixed_motion_budget={} move_input={} move_release={} moved={} resize_input={} resize_release={} resized={} elapsed_ms={} assertions={} observable=xtest-ack-and-geometry failure_mutation=mixed-drag-event-or-geometry-update-drop path=real-pointer",
            client.id,
            MIXED_MOTIONS,
            moved_input,
            move_released,
            moved,
            resized_input,
            resize_released,
            resized,
            elapsed_ms,
            pass
        ),
    )
}

/// T17: protected move/resize/maximize/restore/snap/titlebar sequence.  The
/// verdict uses only stable XIDs, root-relative frame geometry, EWMH state,
/// and real XTEST input; no synthetic event or private WM counter is treated
/// as evidence.
pub fn run_t17(canary: &crate::Canary, args: &[String]) -> (bool, String) {
    let row = crate::arg(args, "--row").map_or_else(|| "unspecified".to_owned(), t17_bound);
    let Some(area) = root_work_area(canary) else {
        return (
            false,
            format!(
                "scenario=T17 client=0 frame=0 row={} stage=precondition work_area_unavailable reason=workspace_precondition failure_diagnostic=row={} stage=precondition client=0 frame=0 reason=workspace_precondition expected=workarea_available=false state=[{}]",
                row,
                row,
                t17_snapshot(canary),
            ),
        );
    };
    let Some(client) = target(canary, args) else {
        return (
            false,
            format!(
                "scenario=T17 client=0 frame=0 row={} stage=precondition target_not_ready reason=workspace_precondition failure_diagnostic=row={} stage=precondition client=0 frame=0 reason=workspace_precondition expected=target_available=false state=[{}]",
                row,
                row,
                t17_snapshot(canary),
            ),
        );
    };
    let client_id = client.id;
    let frame_id = client.parent;
    let selected = select_structure_notify(canary, &client);
    let initial_reset = establish_known_floating(canary, &client, area);
    let Some(baseline) = initial_reset
        .then(|| establish_t17_baseline(canary, &client, area))
        .flatten()
    else {
        return t17_setup_failure(canary, &row, "move", client_id, frame_id);
    };
    let initial_client = baseline.client.clone();
    let initial_frame = baseline.frame.clone();
    let baseline_area = baseline.work_area;
    let initial_state = wm_state(canary, client_id);
    let move_from = title(&initial_frame);
    let move_to = (
        move_from.0.saturating_add(48),
        move_from.1.saturating_add(32),
    );
    let move_sent = xtest_drag_path(canary, move_from, move_to, 1, 8);
    let moved_frame = wait_stable_frame(WAIT_MS, || {
        canary.by_id(frame_id).filter(|frame| {
            (frame.x != initial_frame.x || frame.y != initial_frame.y)
                && frame.width == initial_frame.width
                && frame.height == initial_frame.height
        })
    });
    let move_seen = moved_frame.is_some();
    let move_configures = t17_configures(canary, moved_frame.as_ref());
    let move_actual = t17_optional_geometry(moved_frame.as_ref());
    let Some(_moved_frame) = moved_frame else {
        return (
            false,
            t17_failure(
                canary,
                &row,
                "move",
                client_id,
                frame_id,
                "frame_gone_after_move",
                Some(T17GeometryExpectation::Move {
                    x: initial_frame.x,
                    y: initial_frame.y,
                    width: initial_frame.width,
                    height: initial_frame.height,
                }),
                false,
            ),
        );
    };
    let Ok((resize_reset, _resize_client, resize_frame)) =
        t17_stage_start(canary, client_id, &baseline)
    else {
        return t17_setup_failure(canary, &row, "resize", client_id, frame_id);
    };
    let resize_from = bottom_right(&resize_frame);
    let resize_to = t17_resize_target(&resize_frame, baseline_area);
    let resize_sent = xtest_drag_path(canary, resize_from, resize_to, 1, 8);
    let resized_frame = wait_stable_frame(WAIT_MS, || {
        canary.by_id(frame_id).filter(|frame| {
            (frame.width > resize_frame.width || frame.height > resize_frame.height)
                && frame.x == resize_frame.x
                && frame.y == resize_frame.y
        })
    });
    let resize_seen = resized_frame.is_some();
    let resize_configures = t17_configures(canary, resized_frame.as_ref());
    let resize_actual = t17_optional_geometry(resized_frame.as_ref());
    let Some(_resized_frame) = resized_frame else {
        return (
            false,
            t17_failure(
                canary,
                &row,
                "resize",
                client_id,
                frame_id,
                "frame_gone_after_resize",
                Some(T17GeometryExpectation::Resize {
                    x: resize_frame.x,
                    y: resize_frame.y,
                    width: resize_frame.width,
                    height: resize_frame.height,
                }),
                false,
            ),
        );
    };
    let Ok((max_reset, max_client, max_before)) = t17_stage_start(canary, client_id, &baseline)
    else {
        return t17_setup_failure(canary, &row, "maximize", client_id, frame_id);
    };
    let max_from = title(&max_before);
    let max_to = (
        clamp_i16(baseline_area.0 + (baseline_area.2 / 2) as i32),
        clamp_i16(baseline_area.1),
    );
    let max_sent = hold_drag(canary, max_from, max_to, 8) && release(canary, max_to);
    let maximized_seen = max_sent && wait_until(WAIT_MS, || maximized_both(canary, client_id));
    let max_frame = maximized_seen
        .then(|| {
            wait_stable_frame(WAIT_MS, || {
                canary.by_id(frame_id).filter(|frame| {
                    frame_matches(frame, baseline_area) && maximized_both(canary, client_id)
                })
            })
        })
        .flatten();
    let max_configures = t17_configures(canary, max_frame.as_ref());
    let Some(max_frame) = max_frame else {
        return (
            false,
            t17_failure(
                canary,
                &row,
                "maximize",
                client_id,
                frame_id,
                "frame_gone_after_maximize",
                Some(T17GeometryExpectation::Exact(baseline_area)),
                false,
            ),
        );
    };
    let max_geometry = frame_matches(&max_frame, baseline_area);
    let max_actual = t17_geometry(&max_frame);
    let max_state = wm_state(canary, client_id);
    let restore_from = title(&max_frame);
    let restore_to = (
        clamp_i16(baseline_area.0.saturating_add((baseline_area.2 / 2) as i32)),
        clamp_i16(baseline_area.1.saturating_add((baseline_area.3 / 2) as i32)),
    );
    // Mirror the controller's maximized_restore_rect + plan_move path: the
    // release point is root-relative, while the restored size comes from the
    // saved floating frame and the pointer keeps its frame anchor.
    let restore_anchor_x = i32::from(restore_from.0).saturating_sub(i32::from(max_frame.x));
    let restore_anchor_y = i32::from(restore_from.1).saturating_sub(i32::from(max_frame.y));
    let restore_anchor_dx = (restore_anchor_x as f32 / f32::from(max_frame.width.max(1))
        * f32::from(max_before.width))
    .round() as i32;
    let restore_raw_x = i32::from(restore_to.0).saturating_sub(restore_anchor_dx);
    let restore_raw_y = i32::from(restore_to.1).saturating_sub(restore_anchor_y);
    let restore_max_x = baseline_area
        .0
        .saturating_add(baseline_area.2.saturating_sub(u32::from(max_before.width)) as i32);
    let restore_max_y = baseline_area
        .1
        .saturating_add(baseline_area.3.saturating_sub(u32::from(max_before.height)) as i32);
    let mut restore_expected = max_before.clone();
    restore_expected.x = restore_raw_x
        .clamp(
            baseline_area.0.min(restore_max_x),
            restore_max_x.max(baseline_area.0),
        )
        .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
    restore_expected.y = restore_raw_y
        .clamp(
            baseline_area.1.min(restore_max_y),
            restore_max_y.max(baseline_area.1),
        )
        .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
    let _ = native_configures(canary, &max_client, 0);
    let restore_sent = xtest_drag_path(canary, restore_from, restore_to, 1, 8);
    let restored = restore_sent && wait_until(WAIT_MS, || !maximized(canary, client_id));
    let restored_frame = restored
        .then(|| {
            wait_stable_frame(WAIT_MS, || {
                canary.by_id(frame_id).filter(|frame| {
                    frame_matches(frame, geom_of(&restore_expected))
                        && !maximized(canary, client_id)
                })
            })
        })
        .flatten();
    let restore_configures = t17_configures(canary, restored_frame.as_ref());
    let restore_state = wm_state(canary, client_id);
    let Some(restored_frame) = restored_frame else {
        let diagnostic = t17_restore_diagnostic(
            canary,
            client_id,
            frame_id,
            &baseline,
            &restore_expected,
            &max_frame,
            &max_state,
            &restore_state,
            maximized_seen,
            restored,
            max_configures,
            restore_configures,
        );
        return (
            false,
            format!(
                "{} {}",
                t17_failure(
                    canary,
                    &row,
                    "restore",
                    client_id,
                    frame_id,
                    "frame_gone_after_restore",
                    Some(T17GeometryExpectation::Restore {
                        width: restore_expected.width,
                        height: restore_expected.height,
                        x: restore_expected.x,
                        y: restore_expected.y,
                    }),
                    false,
                ),
                diagnostic,
            ),
        );
    };
    let restore_geometry = frame_matches(&restored_frame, geom_of(&restore_expected));
    let restore_position =
        restored_frame.x == restore_expected.x && restored_frame.y == restore_expected.y;
    let restore_actual = t17_geometry(&restored_frame);
    let Ok((snap_reset, _snap_client, snap_frame)) = t17_stage_start(canary, client_id, &baseline)
    else {
        return t17_setup_failure(canary, &row, "snap", client_id, frame_id);
    };
    let snap_from = title(&snap_frame);
    let snap_to = (
        clamp_i16(baseline_area.0),
        clamp_i16(baseline_area.1 + (baseline_area.3 / 2) as i32),
    );
    let snap_sent = xtest_drag_path(canary, snap_from, snap_to, 1, 8);
    let expected_snap = left_snap_geometry(baseline_area);
    let snapped_frame = snap_sent
        .then(|| {
            wait_stable_frame(WAIT_MS, || {
                canary
                    .by_id(frame_id)
                    .filter(|frame| frame_matches(frame, expected_snap))
            })
        })
        .flatten();
    let snapped = snapped_frame.is_some();
    let snap_configures = t17_configures(canary, snapped_frame.as_ref());
    let snap_actual = t17_optional_geometry(snapped_frame.as_ref());
    let Some(_snapped_frame) = snapped_frame else {
        return (
            false,
            t17_failure(
                canary,
                &row,
                "snap",
                client_id,
                frame_id,
                "frame_gone_after_snap",
                Some(T17GeometryExpectation::Exact(expected_snap)),
                false,
            ),
        );
    };
    let Ok((titlebar_reset, _titlebar_client, titlebar_initial_frame)) =
        t17_stage_start(canary, client_id, &baseline)
    else {
        return t17_setup_failure(canary, &row, "titlebar", client_id, frame_id);
    };
    let title_from = title(&titlebar_initial_frame);
    let title_to = (
        title_from.0.saturating_add(80),
        title_from.1.saturating_add(64),
    );
    let titlebar_sent = xtest_drag_path(canary, title_from, title_to, 1, 8);
    let titlebar_frame = titlebar_sent
        .then(|| {
            wait_stable_frame(WAIT_MS, || {
                canary.by_id(frame_id).filter(|frame| {
                    !frame_matches_known_snap(frame, baseline_area)
                        && (frame.x != titlebar_initial_frame.x
                            || frame.y != titlebar_initial_frame.y)
                })
            })
        })
        .flatten();
    let titlebar_moved = titlebar_frame.is_some();
    let titlebar_configures = t17_configures(canary, titlebar_frame.as_ref());
    let titlebar_actual = t17_optional_geometry(titlebar_frame.as_ref());
    let final_identity = canary
        .by_id(client_id)
        .is_some_and(|current| current.parent == frame_id)
        && canary
            .by_id(frame_id)
            .is_some_and(|frame| frame.parent == canary.root && frame.map_state == 2);
    let final_state = wm_state(canary, client_id);
    let cleanup_floating = canary.by_id(frame_id).is_some_and(|frame| {
        !maximized(canary, client_id) && !frame_matches_known_snap(&frame, baseline_area)
    });
    let pass = selected
        && initial_reset
        && move_sent
        && move_seen
        && move_configures > 0
        && resize_reset
        && resize_sent
        && resize_seen
        && resize_configures > 0
        && max_reset
        && max_sent
        && maximized_seen
        && max_geometry
        && max_configures > 0
        && restore_sent
        && restored
        && restore_geometry
        && restore_position
        && restore_configures > 0
        && snap_reset
        && snap_sent
        && snapped
        && snap_configures > 0
        && titlebar_reset
        && titlebar_sent
        && titlebar_moved
        && titlebar_configures > 0
        && final_identity
        && cleanup_floating
        && initial_state == final_state;
    (
        pass,
        format!(
            "scenario=T17 client={} frame={} workarea={}x{}+{}+{} current_workarea={} saved_baseline_client={} saved_baseline_frame={} maximize_frame={} restore_expected_frame={} restore_actual_frame={} net_wm_state_maximize={} net_wm_state_restore={} net_wm_state_final={} baseline_client={}x{}+{}+{} baseline_frame={}x{}+{}+{} move_expected={} move_actual={} resize_expected={} resize_actual={} maximize_expected={} maximize_actual={} restore_expected={} restore_actual={} snap_expected={} snap_actual={} titlebar_expected={} titlebar_actual={} selected_structure_notify={} initial_reset_floating={} move_sent={} move_seen={} move_configures={} resize_reset_floating={} resize_sent={} resize_seen={} resize_configures={} maximize_reset_floating={} maximize_sent={} maximized_seen={} maximized={} max_geometry={} max_configures={} configure_acknowledgements_maximize={} restore_sent={} restored={} restore_geometry={} restore_position={} restore_configures={} configure_acknowledgements_restore={} snap_reset_floating={} snap_sent={} snapped={} snap_configures={} titlebar_reset_floating={} titlebar_sent={} titlebar_moved={} titlebar_configures={} final_identity={} cleanup_floating={} ewmh_restored={} assertions={} configure_payload=matching-frame-root-geometry observable=stage-aware-xid-geometry-configure-ewmh failure_mutation=protected-move-resize-maximize-restore-snap-titlebar-route-change path=real-pointer",
            client_id,
            frame_id,
            baseline_area.2,
            baseline_area.3,
            baseline_area.0,
            baseline_area.1,
            t17_current_workarea(canary),
            t17_geometry(&baseline.client),
            t17_geometry(&baseline.frame),
            t17_geometry(&max_frame),
            t17_geometry(&restore_expected),
            t17_geometry(&restored_frame),
            t17_atom_values(&max_state),
            t17_atom_values(&restore_state),
            t17_atom_values(&final_state),
            initial_client.width,
            initial_client.height,
            initial_client.x,
            initial_client.y,
            initial_frame.width,
            initial_frame.height,
            initial_frame.x,
            initial_frame.y,
            t17_geometry(&initial_frame),
            move_actual,
            t17_geometry(&baseline.frame),
            resize_actual,
            t17_geometry_values(baseline_area),
            max_actual,
            t17_geometry(&restore_expected),
            restore_actual,
            t17_geometry_values(expected_snap),
            snap_actual,
            t17_geometry(&baseline.frame),
            titlebar_actual,
            selected,
            initial_reset,
            move_sent,
            move_seen,
            move_configures,
            resize_reset,
            resize_sent,
            resize_seen,
            resize_configures,
            max_reset,
            max_sent,
            maximized_seen,
            maximized_seen,
            max_geometry,
            max_configures,
            max_configures,
            restore_sent,
            restored,
            restore_geometry,
            restore_position,
            restore_configures,
            restore_configures,
            snap_reset,
            snap_sent,
            snapped,
            snap_configures,
            titlebar_reset,
            titlebar_sent,
            titlebar_moved,
            titlebar_configures,
            final_identity,
            cleanup_floating,
            initial_state == final_state,
            pass
        ),
    )
}

/// T18 static admission.  X11 can expose the desktop client surface and its
/// pixels, but not the selection element's semantic visibility, bounds, or
/// stacking relation inside that surface.  The admission result is retained;
/// native semantic observation is explicitly unverified.
pub fn run_t18(canary: &crate::Canary, args: &[String]) -> (bool, String) {
    let target_ready =
        if crate::arg(args, "--window").is_some() || crate::arg(args, "--name").is_some() {
            target(canary, args)
                .and_then(|client| input_surface(canary, &client))
                .is_some()
        } else {
            canary
                .find("desktop")
                .into_iter()
                .any(|client| input_surface(canary, &client).is_some())
        };
    (
        target_ready,
        format!(
            "scenario=T18 classification=UNVERIFIED admission=static static_admission={target_ready} target_surface_ready={target_ready} observable=client-pixels-only native_semantic_observable={T18_NATIVE_SEMANTIC_OBSERVABLE} native_semantic_observation=SKIPPED failure_mutation=removing-selection-update-is-indistinguishable-at-x11-boundary unsupported={T18_LIMITATION} path=static-admission"
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::{T18_LIMITATION, T18_NATIVE_SEMANTIC_OBSERVABLE};

    #[test]
    fn t18_admission_keeps_native_limit_explicit() {
        const { assert!(!T18_NATIVE_SEMANTIC_OBSERVABLE) };
        assert!(T18_LIMITATION.contains("desktop-selection"));
    }
}
