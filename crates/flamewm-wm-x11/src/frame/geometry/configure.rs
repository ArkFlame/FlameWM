use super::*;

/// Adjust a client origin for a size change under `gravity`.
///
/// The reference point (`anchor * size / 2`) stays fixed; the origin moves
/// so the reference point is preserved. `Static` keeps the origin fixed.
#[must_use]
pub fn gravity_adjust_origin(
    old_origin: RootPoint,
    old_size: (u32, u32),
    new_size: (u32, u32),
    gravity: WinGravity,
) -> RootPoint {
    if gravity == WinGravity::Static {
        return old_origin;
    }
    let (ax, ay) = gravity.anchor_num();
    // Reference-point invariant: new_origin + a*new/2 = old_origin + a*old/2,
    // i.e. new_origin = old_origin + (old - new) * a / 2.
    let shift = |old: u32, new: u32, a: i32| -> i32 {
        let delta = i64::from(old) - i64::from(new);
        i32::try_from(delta * i64::from(a) / 2).unwrap_or(0)
    };
    RootPoint::new(
        old_origin
            .x
            .saturating_add(shift(old_size.0, new_size.0, ax)),
        old_origin
            .y
            .saturating_add(shift(old_size.1, new_size.1, ay)),
    )
}

/// Client rect (root coords) -> outer frame rect.
#[must_use]
pub fn client_rect_to_frame(
    client_origin: RootPoint,
    client_size: (u32, u32),
    extents: FrameExtents,
) -> RootRect {
    let (dw, dh) = extents.delta();
    let (ox, oy) = extents.client_offset();
    RootRect::new(
        client_origin.x.saturating_sub(ox),
        client_origin.y.saturating_sub(oy),
        client_size.0.saturating_add(dw.max(0) as u32).max(1) as i32,
        client_size.1.saturating_add(dh.max(0) as u32).max(1) as i32,
    )
}

/// Outer frame rect -> client-local rect: origin `(inset, titlebar)`, size
/// `(frame.w - 2 * inset, frame.h - titlebar - inset)`.
#[must_use]
pub fn frame_to_client_local(frame: RootRect, extents: FrameExtents) -> ClientLocalRect {
    let (dw, dh) = extents.delta();
    let (ox, oy) = extents.client_offset();
    ClientLocalRect::new(
        ox,
        oy,
        frame.w.saturating_sub(dw).max(1),
        frame.h.saturating_sub(dh).max(1),
    )
}

/// Outer frame rect -> synthetic client-root rect (frame offset + local size).
#[must_use]
pub fn frame_to_client_root(frame: RootRect, extents: FrameExtents) -> ClientRootRect {
    let local = frame_to_client_local(frame, extents);
    let (ox, oy) = extents.client_offset();
    ClientRootRect::new(
        frame.x.saturating_add(ox),
        frame.y.saturating_add(oy),
        local.w,
        local.h,
    )
}

/// Client configure request: constrain the requested client size via hints,
/// move the origin per gravity, convert to a frame plan.
///
/// Maximized/Fullscreen frames are never corrupted by client requests: the
/// start rect is returned verbatim with `noop = true`.
/// Origin-gravity form used by tests: a position-only request keeps the requested origin.
#[cfg(test)]
#[must_use]
pub fn plan_client_configure(
    req: &GeometryRequest,
    req_client_origin: RootPoint,
    req_client_size: Option<(u32, u32)>,
) -> GeometryPlan {
    plan_client_configure_explicit(req, req_client_origin, req_client_size, false)
}

/// [`plan_client_configure`] with `origin_requested`: the request carried X or Y. An explicit
/// origin is honoured even when a size is also requested (ICCCM 4.1.5, `_NET_MOVERESIZE_WINDOW`);
/// without it, gravity places the origin on a size change.
#[must_use]
pub fn plan_client_configure_explicit(
    req: &GeometryRequest,
    req_client_origin: RootPoint,
    req_client_size: Option<(u32, u32)>,
    origin_requested: bool,
) -> GeometryPlan {
    if matches!(
        req.start.mode,
        PlacementMode::Maximized | PlacementMode::Fullscreen
    ) {
        return GeometryPlan {
            noop: true,
            ..GeometryPlan::of(req, req.start.rect)
        };
    }
    let gravity = WinGravity::of(&req.hints);
    let prev_root = frame_to_client_root(req.start.rect, req.frame_extents);
    let prev_size = prev_root.size_u32();
    let prev_origin = prev_root.origin();
    let new_size = match req_client_size {
        Some((w, h)) => snap_client_size(w.max(1), h.max(1), &req.hints),
        None => prev_size,
    };
    // Gravity moves the requested origin only when the size changed; a bare
    // position request (no size) honors the requested origin.
    let origin = if req_client_size.is_some() && !origin_requested {
        gravity_adjust_origin(prev_origin, prev_size, new_size, gravity)
    } else {
        req_client_origin
    };
    let frame = client_rect_to_frame(origin, new_size, req.frame_extents);
    // Floating frames obey the same keep-visible rule as user moves: a client
    // cannot park a window fully off-screen. Snapped frames keep their geometry.
    let frame = match req.start.mode {
        PlacementMode::Floating => keep_on_outputs(frame, req.outputs, req.work_area),
        _ => frame,
    };
    GeometryPlan::of(req, frame)
}

/// Initial map: client rect -> frame rect, clamped inside the work area.
#[must_use]
pub fn plan_initial_map(
    req: &GeometryRequest,
    req_client_origin: RootPoint,
    req_client_size: (u32, u32),
) -> GeometryPlan {
    let gravity = WinGravity::of(&req.hints);
    let size = snap_client_size(
        req_client_size.0.max(1),
        req_client_size.1.max(1),
        &req.hints,
    );
    // Reference point at the requested origin under gravity.
    let origin = gravity_adjust_origin(req_client_origin, size, size, gravity);
    let frame = client_rect_to_frame(origin, size, req.frame_extents);
    let wa = req.work_area;
    let x = clamp_span(frame.x, frame.w, wa.x, wa.w);
    let y = clamp_span(frame.y, frame.h, wa.y, wa.h);
    GeometryPlan::of(req, RootRect::new(x, y, frame.w, frame.h))
}

/// Only the unpositioned first map of a normal top-level window may follow the mouse.
/// A non-zero pre-map origin without position flags is ambiguous: it may have
/// come from an application `ConfigureWindow`, so we conservatively preserve it.
#[must_use]
pub fn can_place_near_pointer(
    adopt: bool,
    is_normal: bool,
    is_transient: bool,
    hints: ClientSizeHints,
    client_origin: RootPoint,
) -> bool {
    !adopt
        && is_normal
        && !is_transient
        && !hints.position_specified
        && client_origin == RootPoint::new(0, 0)
}

/// Place a newly created, unpositioned frame near the pointer on its selected
/// output. The caller supplies that output's work area (after panel struts).
/// Reuse the canonical initial-map planner for size hints, gravity and clamping.
/// Oversize windows anchor to the work area's top/left so controls stay reachable.
#[must_use]
pub fn plan_initial_map_near_pointer(
    req: &GeometryRequest<'_>,
    pointer: RootPoint,
    selected_work_area: RootRect,
    req_client_size: (u32, u32),
) -> GeometryPlan {
    pub(super) const OFFSET: i32 = 24;
    let target_frame_x = pointer.x.saturating_add(OFFSET);
    let target_frame_y = pointer.y.saturating_add(OFFSET);
    let (ox, oy) = req.frame_extents.client_offset();
    let origin = RootPoint::new(
        target_frame_x.saturating_add(ox),
        target_frame_y.saturating_add(oy),
    );
    let localized = GeometryRequest {
        work_area: selected_work_area,
        ..*req
    };
    plan_initial_map(&localized, origin, req_client_size)
}

/// Re-adopt after a WM restart: the client sits at its root position inside
/// the previous frame (frame origin + border + titlebar), so the frame is
/// the exact inverse. No gravity, snapping or work-area clamp: existing
/// geometry is preserved.
#[must_use]
pub fn plan_adopt(
    client_origin: RootPoint,
    client_size: (u32, u32),
    extents: FrameExtents,
) -> RootRect {
    let frame = client_rect_to_frame(client_origin, client_size, extents);
    RootRect::new(
        frame.x.saturating_sub(extents.border),
        frame.y.saturating_sub(extents.border),
        frame.w,
        frame.h,
    )
}
