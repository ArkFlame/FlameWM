//! Move-grab edge and window snapping decisions.

use super::*;

pub(super) const MOVE_ACTIVATION_THRESHOLD_PX: u32 = 4;

pub(super) fn move_activated(dx: i32, dy: i32) -> bool {
    dx.unsigned_abs() >= MOVE_ACTIVATION_THRESHOLD_PX
        || dy.unsigned_abs() >= MOVE_ACTIVATION_THRESHOLD_PX
}

#[allow(clippy::too_many_arguments)]
pub(super) fn move_candidate(
    start: Rect,
    start_mode: Mode,
    restore: Rect,
    start_pointer: (f64, f64),
    pointer: Point<f64, Logical>,
    anchor_num: i32,
    anchor_den: i32,
    anchor_y: i32,
    activated: bool,
) -> Rect {
    if start_mode == Mode::Normal {
        return Rect::new(
            start.x + (pointer.x - start_pointer.0).round() as i32,
            start.y + (pointer.y - start_pointer.1).round() as i32,
            start.w,
            start.h,
        );
    }
    if !activated {
        return start;
    }
    let anchor_x = (f64::from(restore.w) * f64::from(anchor_num) / f64::from(anchor_den.max(1)))
        .round() as i32;
    Rect::new(
        pointer.x.round() as i32 - anchor_x,
        pointer.y.round() as i32 - anchor_y + TITLEBAR_HEIGHT,
        restore.w,
        restore.h,
    )
}

/// Clamp a free move/restore candidate on the physical output, accounting for the decorated frame.
/// Candidates and the window model use client geometry; only this boundary converts to/from frame
/// geometry, so titlebar height is included exactly once in the visibility calculation.
pub(super) fn clamp_move_candidate(candidate: Rect, output: Rect, titlebars: bool) -> Rect {
    let frame = if titlebars {
        Rect::new(
            candidate.x,
            candidate.y.saturating_sub(TITLEBAR_HEIGHT),
            candidate.w,
            candidate.h.saturating_add(TITLEBAR_HEIGHT),
        )
    } else {
        candidate
    };
    let visible = keep_visible(
        flamewm_api::Rect::new(frame.x, frame.y, frame.w, frame.h),
        flamewm_api::Rect::new(output.x, output.y, output.w, output.h),
    );
    let frame = Rect::new(visible.x, visible.y, visible.width, visible.height);
    if titlebars {
        Rect::new(
            frame.x,
            frame.y.saturating_add(TITLEBAR_HEIGHT),
            frame.w,
            frame.h.saturating_sub(TITLEBAR_HEIGHT),
        )
    } else {
        frame
    }
}

pub(super) fn commit_move_snap(
    model: &mut crate::model::WindowModel,
    id: WindowId,
    target: SnapTarget,
    work: Rect,
    candidate: Rect,
) {
    if target == SnapTarget::Maximize {
        model.maximize_to(id, work, candidate);
    } else {
        model.snap_to(id, target, work, candidate);
    }
}

pub(super) fn frame_for_client(client: Rect, titlebar: bool) -> Rect {
    if titlebar { client.frame() } else { client }
}

pub(super) fn snap_target_at(
    state: &State,
    pointer: Point<f64, Logical>,
    activated: bool,
) -> SnapTarget {
    let output = state.output_rect();
    let work = state.work_area();
    let (edge_snap, edge_px) = state.model.edge_snap();
    if !edge_snap {
        return SnapTarget::None;
    }
    snap_target_for_move_with(
        activated,
        flamewm_api::Point::new(pointer.x.round() as i32, pointer.y.round() as i32),
        flamewm_api::Rect::new(output.x, output.y, output.w, output.h),
        flamewm_api::Rect::new(work.x, work.y, work.w, work.h),
        edge_px,
    )
}

/// Default-threshold fixture used by the snap tests; the live path passes `edgeSnap.thresholdPx`.
#[cfg(test)]
pub(super) fn snap_target_for_move(
    activated: bool,
    pointer: flamewm_api::Point,
    output: flamewm_api::Rect,
    work: flamewm_api::Rect,
) -> SnapTarget {
    snap_target_for_move_with(
        activated,
        pointer,
        output,
        work,
        flamewm_window_core::EDGE_THRESHOLD,
    )
}

/// Snap target for a move with an explicit `edgeSnap.thresholdPx`.
pub(super) fn snap_target_for_move_with(
    activated: bool,
    pointer: flamewm_api::Point,
    output: flamewm_api::Rect,
    work: flamewm_api::Rect,
    edge_px: i32,
) -> SnapTarget {
    if !activated {
        return SnapTarget::None;
    }
    flamewm_window_core::snap_target_in_output_with(pointer, output, work, edge_px)
}
