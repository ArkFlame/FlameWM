use super::*;

/// Interactive move: candidate = start + delta (never incremental),
/// kept visible on the outputs (see [`keep_on_outputs`]).
#[must_use]
pub fn plan_move(req: &GeometryRequest<'_>) -> GeometryPlan {
    let s = req.start.rect;
    let x = s.x.saturating_add(req.pointer_delta.0);
    let y = s.y.saturating_add(req.pointer_delta.1);
    let frame = keep_on_outputs(RootRect::new(x, y, s.w, s.h), req.outputs, req.work_area);
    let frame = keep_titlebar_in_work_area(
        frame,
        req.work_area,
        req.outputs,
        req.frame_extents.titlebar_h,
    );
    GeometryPlan::of(req, frame)
}

/// Interactive resize with a fixed opposite-edge invariant.
///
/// 1. Fix the edge(s) opposite the drag from `start.rect`.
/// 2. Constrain only the moving edge so a quarter of the frame stays visible
///    within the outputs' bounding box.
/// 3. Constrain the requested client size via [`snap_client_size`]
///    (min/max/base/inc/aspect).
/// 4. Reconstruct the dragged edge as fixed edge +/- constrained size.
///
/// No `clamp_inside`-after-resize: the fixed edge never moves, even if the
/// constrained size pushes the moving edge outside the work area.
#[must_use]
pub fn plan_resize(req: &GeometryRequest<'_>) -> GeometryPlan {
    let s = req.start.rect;
    let (dx, dy) = req.pointer_delta;
    let bounds = outputs_bounds(req.outputs, req.work_area);
    let (dw, dh) = req.frame_extents.delta();

    let fixed_left = s.x;
    let fixed_right = s.x.saturating_add(s.w);
    let fixed_top = s.y;
    let fixed_bottom = s.y.saturating_add(s.h);

    // Desired moving edges, constrained against the outputs' bounds only.
    // Left/top moving edges must leave at least a 1px frame; the size-hint
    // floor is applied later via snap_client_size.
    let mut moving_left = s.x.saturating_add(dx);
    let mut moving_right = fixed_right.saturating_add(dx);
    let mut moving_top = s.y.saturating_add(dy);
    let mut moving_bottom = fixed_bottom.saturating_add(dy);
    if req.edges.left {
        let (lo, hi) = left_edge_range(fixed_right, bounds.x, bounds.x.saturating_add(bounds.w));
        moving_left = moving_left.clamp(lo, hi);
    }
    if req.edges.right {
        let (lo, hi) = right_edge_range(fixed_left, bounds.x, bounds.x.saturating_add(bounds.w));
        moving_right = moving_right.clamp(lo, hi);
    }
    if req.edges.top {
        let (lo, hi) = left_edge_range(fixed_bottom, bounds.y, bounds.y.saturating_add(bounds.h));
        moving_top = moving_top.clamp(lo, hi);
    }
    if req.edges.bottom {
        let (lo, hi) = right_edge_range(fixed_top, bounds.y, bounds.y.saturating_add(bounds.h));
        moving_bottom = moving_bottom.clamp(lo, hi);
    }

    let raw_w = if req.edges.left {
        fixed_right.saturating_sub(moving_left)
    } else if req.edges.right {
        moving_right.saturating_sub(fixed_left)
    } else {
        s.w
    }
    .max(1);
    let raw_h = if req.edges.top {
        fixed_bottom.saturating_sub(moving_top)
    } else if req.edges.bottom {
        moving_bottom.saturating_sub(fixed_top)
    } else {
        s.h
    }
    .max(1);

    // The dragged edge stops at the work-area edge (dock struts excluded), so a resize never
    // extends under a dock. The fixed edge never moves. A fixed edge already outside the work
    // area leaves no span to cap against.
    let wa = req.work_area;
    let span_w = if req.edges.left {
        fixed_right.saturating_sub(wa.x)
    } else if req.edges.right {
        wa.x.saturating_add(wa.w).saturating_sub(fixed_left)
    } else {
        i32::MAX
    };
    let span_h = if req.edges.top {
        fixed_bottom.saturating_sub(wa.y)
    } else if req.edges.bottom {
        wa.y.saturating_add(wa.h).saturating_sub(fixed_top)
    } else {
        i32::MAX
    };
    // The cap stops growth past the work area; it must not shrink a frame that already overhangs
    // it, so it never falls below the frame's current size on that axis.
    let raw_w = if span_w > 0 {
        raw_w.min(span_w.max(s.w))
    } else {
        raw_w
    };
    let raw_h = if span_h > 0 {
        raw_h.min(span_h.max(s.h))
    } else {
        raw_h
    };

    // Frame domain -> client domain -> hints -> frame domain.
    let raw_cw = raw_w.saturating_sub(dw).max(1) as u32;
    let raw_ch = raw_h.saturating_sub(dh).max(1) as u32;
    let (ccw, cch) = snap_client_size(raw_cw, raw_ch, &req.hints);
    let outer_w = ccw.saturating_add(dw.max(0) as u32).max(1) as i32;
    let outer_h = cch.saturating_add(dh.max(0) as u32).max(1) as i32;

    // Reconstruct the dragged edge from the fixed edge + constrained size.
    let x = if req.edges.left {
        fixed_right.saturating_sub(outer_w)
    } else {
        fixed_left
    };
    let y = if req.edges.top {
        fixed_bottom.saturating_sub(outer_h)
    } else {
        fixed_top
    };
    let w = if req.edges.left || req.edges.right {
        outer_w
    } else {
        s.w
    };
    let h = if req.edges.top || req.edges.bottom {
        outer_h
    } else {
        s.h
    };
    GeometryPlan::of(req, RootRect::new(x, y, w.max(1), h.max(1)))
}
