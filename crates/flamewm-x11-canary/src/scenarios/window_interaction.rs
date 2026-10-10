//! T06 real move (real-pointer drag) and T07 real 8-way resize.

use super::{real_drag, real_warp};
use x11rb::connection::Connection as _;

pub(crate) fn resolve_target(canary: &crate::Canary, args: &[String]) -> Option<crate::WinInfo> {
    let timeout_secs = crate::arg(args, "--timeout-secs")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(3)
        .clamp(1, 30);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        let candidate = if let Some(id) = crate::arg(args, "--window") {
            id.parse::<u32>()
                .ok()
                .and_then(|window| canary.by_id(window))
        } else {
            let name = crate::arg(args, "--name").unwrap_or_else(|| "flame".to_owned());
            canary
                .find(&name)
                .into_iter()
                .find(|window| managed_viewable(canary, window))
        };
        if let Some(window) = candidate.filter(|window| managed_viewable(canary, window)) {
            return Some(window);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// A client is interaction-ready only after the WM has reparented it into a
/// viewable, non-empty frame.  Resolving the client alone can observe the
/// short-lived 1x1/unmapped state during application startup.
fn managed_viewable(canary: &crate::Canary, window: &crate::WinInfo) -> bool {
    window.parent != canary.root
        && window.map_state == 2
        && window.width > 0
        && window.height > 0
        && canary.by_id(window.parent).is_some_and(|frame| {
            frame.parent == canary.root
                && frame.map_state == 2
                && frame.width > 0
                && frame.height > 0
        })
}

/// Return the managed frame's root-relative rectangle for real pointer input.
/// Client geometry is parent-relative, so using it as root coordinates can
/// place XTEST presses in the client body instead of the frame/title region.
pub(crate) fn input_surface(
    canary: &crate::Canary,
    client: &crate::WinInfo,
) -> Option<crate::WinInfo> {
    canary
        .by_id(client.parent)
        .filter(|frame| frame.parent == canary.root && managed_viewable(canary, client))
}

fn diagnostic(observed: bool, detail: impl Into<String>) -> (bool, String) {
    (
        observed,
        format!(
            "classification=REQUIRED_HARD product_gate=hard {}",
            detail.into()
        ),
    )
}

/// T06: a real-pointer drag must move the frame and leave it at that position
/// after the XTEST button release.
pub fn run_t06(canary: &crate::Canary, args: &[String]) -> (bool, String) {
    let Some(work_area) = super::interaction_regressions::root_work_area(canary) else {
        return diagnostic(false, "observed=false work area unavailable path=real-pointer");
    };
    let Some(target) = resolve_target(canary, args) else {
        return diagnostic(false, "observed=false no target path=real-pointer");
    };
    let reset_floating =
        super::interaction_regressions::establish_known_floating(canary, &target, work_area);
    let before = match canary.by_id(target.id) {
        Some(w) => w,
        None => {
            return diagnostic(false, format!(
                "observed=false target {} gone path=real-pointer",
                target.id
            ));
        }
    };
    let Some(surface) = input_surface(canary, &before) else {
        return diagnostic(false, format!(
            "observed=false target {} frame not ready path=real-pointer",
            target.id
        ));
    };
    // Refresh after reset_floating: baseline and input point must describe the
    // same current frame immediately before any real pointer action.
    let Some(before) = canary.by_id(target.id) else {
        return diagnostic(false, format!("observed=false target {} gone after reset path=real-pointer", target.id));
    };
    let Some(baseline) = input_surface(canary, &before) else {
        return diagnostic(false, format!("observed=false target {} frame not ready after reset path=real-pointer", target.id));
    };
    // Mirror the canonical frame layout: the title-drag child is the
    // titlebar band below the 6px top strip and left of the 114px controls.
    const TITLEBAR_H: i16 = 31;
    const CONTROLS_WIDTH: i16 = 114;
    const RESIZE_T: i16 = 6;
    let title_drag_width =
        (baseline.width as i16).saturating_sub(RESIZE_T.saturating_mul(2) + CONTROLS_WIDTH);
    let from = (
        baseline.x.saturating_add(RESIZE_T + title_drag_width / 2),
        baseline
            .y
            .saturating_add(RESIZE_T + (TITLEBAR_H - RESIZE_T) / 2),
    );
    let to = (from.0.saturating_add(64), from.1.saturating_add(48));
    if !real_warp(canary, from.0, from.1) {
        return diagnostic(false, format!(
            "observed=false id={} warp failed path=real-pointer",
            target.id
        ));
    }
    let dragged = real_drag(canary, from, to, 1, 8);
    // The path helper returns only after it has sent the release. Drain events
    // already queued at that boundary, then observe geometry without generating
    // further input so a replayed motion cannot masquerade as the drag result.
    while canary.conn.poll_for_event().is_ok_and(|event| event.is_some()) {}
    let work_left = i64::from(work_area.0);
    let work_top = i64::from(work_area.1);
    let work_right = work_left + i64::from(work_area.2);
    let work_bottom = work_top + i64::from(work_area.3);
    let min_x = work_left;
    let max_x = work_right - i64::from(baseline.width);
    let min_y = work_top;
    let max_y = work_bottom - i64::from(baseline.height);
    if work_area.2 <= 0
        || work_area.3 <= 0
        || max_x < min_x
        || max_y < min_y
    {
        return diagnostic(false, format!(
            "observed=false invalid work area or oversized frame work_area={work_area:?} frame={}x{} path=real-pointer",
            baseline.width, baseline.height
        ));
    }
    let requested_x = i64::from(baseline.x) + 64;
    let requested_y = i64::from(baseline.y) + 48;
    let expected_x = requested_x.clamp(min_x, max_x);
    let expected_y = requested_y.clamp(min_y, max_y);
    let expected_dx = expected_x - i64::from(baseline.x);
    let expected_dy = expected_y - i64::from(baseline.y);
    let fixture_clamped = expected_dx < 60 || expected_dy < 44;
    let mut post_release_settled_position = None;
    let settled = if fixture_clamped {
        false
    } else {
        super::interaction_regressions::wait_until(
            2_000,
            || {
                while canary.conn.poll_for_event().is_ok_and(|event| event.is_some()) {}
                let position = canary
                    .by_id(target.id)
                    .and_then(|after| input_surface(canary, &after))
                    .map(|frame| (frame.x, frame.y, frame.width, frame.height));
                let expected = position.is_some_and(|frame| {
                    i64::from(frame.0) == expected_x
                        && i64::from(frame.1) == expected_y
                        && frame.2 == baseline.width
                        && frame.3 == baseline.height
                });
                if expected {
                    post_release_settled_position = position;
                }
                expected && position.is_some_and(|frame| {
                        frame.2 == baseline.width
                            && frame.3 == baseline.height
                            && i64::from(frame.0) - i64::from(baseline.x) == expected_dx
                            && i64::from(frame.1) - i64::from(baseline.y) == expected_dy
                    })
            },
        )
    };
    let accepted_final = canary
        .by_id(target.id)
        .and_then(|after| input_surface(canary, &after))
        .map(|frame| (frame.x, frame.y, frame.width, frame.height));
    let mut stable_after_release = post_release_settled_position;
    let mut stable = post_release_settled_position.is_some() && settled;
    for _ in 0..10 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        while canary.conn.poll_for_event().is_ok_and(|event| event.is_some()) {}
        let sample = canary.by_id(target.id)
            .and_then(|after| input_surface(canary, &after))
            .map(|frame| (frame.x, frame.y, frame.width, frame.height));
        stable &= sample == post_release_settled_position
            && sample.is_some_and(|frame| {
                i64::from(frame.0) == expected_x
                    && i64::from(frame.1) == expected_y
                    && frame.2 == baseline.width
                    && frame.3 == baseline.height
            });
        stable_after_release = sample;
    }
    let after = canary.by_id(target.id);
    let (observed, detail) = match after {
        Some(w) => {
            let after_frame = input_surface(canary, &w);
            let (dx, dy) = match after_frame.as_ref() {
                Some(after) => (i32::from(after.x) - i32::from(baseline.x), i32::from(after.y) - i32::from(baseline.y)),
                _ => (0, 0),
            };
            let dimensions_unchanged = after_frame.as_ref().is_some_and(|after| after.width == baseline.width && after.height == baseline.height);
            let movement_matches = i64::from(dx) - expected_dx <= 4
                && expected_dx - i64::from(dx) <= 4
                && i64::from(dy) - expected_dy <= 4
                && expected_dy - i64::from(dy) <= 4;
            let moved = reset_floating
                && dragged
                && settled
                && !fixture_clamped
                && movement_matches
                && dimensions_unchanged
                && post_release_settled_position.is_some_and(|position| position.0 == after_frame.as_ref().map_or(0, |f| f.x) && position.1 == after_frame.as_ref().map_or(0, |f| f.y))
                && stable && post_release_settled_position == stable_after_release;
            (
                moved,
                format!(
                    "id={} frame_before={}x{}+{}+{} frame_after={}x{}+{}+{} dx={dx} dy={dy} expected_clipped_delta={}x{} fixture_clamped={fixture_clamped} dimensions_unchanged={dimensions_unchanged} reset_floating={reset_floating} dragged={dragged} settled={settled} post_release_settled_position={post_release_settled_position:?} accepted_final={accepted_final:?} stable_after_release={stable_after_release:?} stable={stable} path=real-pointer",
                    target.id,
                    baseline.width, baseline.height, baseline.x, baseline.y,
                    after_frame.as_ref().map_or(0, |frame| frame.width),
                    after_frame.as_ref().map_or(0, |frame| frame.height),
                    after_frame.as_ref().map_or(0, |frame| frame.x),
                    after_frame.as_ref().map_or(0, |frame| frame.y),
                    expected_dx, expected_dy,
                ),
            )
        }
        None => (
            false,
            format!("id={} gone after drag path=real-pointer", target.id),
        ),
    };
    diagnostic(observed, format!("observed={observed} {detail}"))
}

/// T07: real 8-way resize via corner/edge drags; at least one drag changes
/// geometry and the pure 6px edge contract still classifies all 8 points.
pub fn run_t07(canary: &crate::Canary, args: &[String]) -> (bool, String) {
    let Some(target) = resolve_target(canary, args) else {
        return (false, "no target path=real-pointer".to_owned());
    };
    let before = match canary.by_id(target.id) {
        Some(w) => w,
        None => {
            return (
                false,
                format!("target {} gone path=real-pointer", target.id),
            );
        }
    };
    let Some(surface) = input_surface(canary, &before) else {
        return (
            false,
            format!("target {} frame not ready path=real-pointer", target.id),
        );
    };
    // 8 edge/corner grips in root coordinates.
    let x0 = surface.x;
    let y0 = surface.y;
    let x1 = surface.x.saturating_add(surface.width as i16);
    let y1 = surface.y.saturating_add(surface.height as i16);
    let cx = surface.x.saturating_add(surface.width as i16 / 2);
    let cy = surface.y.saturating_add(surface.height as i16 / 2);
    let grips: [(i16, i16, i16, i16); 8] = [
        (cx, y0 + 2, 0, -24),
        (cx, y1 - 3, 0, 24),
        (x0 + 2, cy, -24, 0),
        (x1 - 3, cy, 24, 0),
        (x0 + 2, y0 + 2, -20, -20),
        (x1 - 3, y0 + 2, 20, -20),
        (x0 + 2, y1 - 3, -20, 20),
        (x1 - 3, y1 - 3, 20, 20),
    ];
    let mut drags = 0;
    let mut changed = 0;
    let mut cur = before.clone();
    for (gx, gy, dx, dy) in grips {
        let to = (gx.saturating_add(dx), gy.saturating_add(dy));
        if real_drag(canary, (gx, gy), to, 1, 6) {
            drags += 1;
        }
        std::thread::sleep(std::time::Duration::from_millis(120));
        if let Some(w) = canary.by_id(target.id) {
            if w.width != cur.width || w.height != cur.height {
                changed += 1;
                cur = w;
            }
        }
    }
    // Pure 6px contract check on the final geometry (mirrors product edge).
    let edge = |w: u32, h: u32, x: i16, y: i16| {
        let w = w.clamp(1, u32::from(u16::MAX)) as i16;
        let h = h.clamp(1, u32::from(u16::MAX)) as i16;
        x <= 6 || x >= w.saturating_sub(6) || y <= 6 || y >= h.saturating_sub(6)
    };
    let mut matched = 0;
    if cur.width > 0 && cur.height > 0 {
        let w = cur.width as i16;
        let h = cur.height as i16;
        let pts = [
            (w / 2, 2),
            (w / 2, h - 3),
            (2, h / 2),
            (w - 3, h / 2),
            (2, 2),
            (w - 3, 2),
            (2, h - 3),
            (w - 3, h - 3),
        ];
        for (x, y) in pts {
            if edge(cur.width.into(), cur.height.into(), x, y) {
                matched += 1;
            }
        }
    }
    let pass = drags >= 6 && changed >= 1 && matched == 8;
    (
        pass,
        format!(
            "id={} drags={drags}/8 geom_changed={changed} edges={matched}/8 final={}x{} path=real-pointer",
            target.id, cur.width, cur.height
        ),
    )
}
