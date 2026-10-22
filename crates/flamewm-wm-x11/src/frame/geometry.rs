//! Pure frame geometry planner: move / resize / configure (F08-F10).
//!
//! No X calls, no x11rb, no live-frame mutation. All planners take a
//! [`GeometryRequest`] value (snapshot + delta) and return a
//! [`GeometryPlan`]. Deltas are always relative to `start.rect` — never
//! incremental — so callers can replay from the drag/press snapshot.

use super::coords::{ClientLocalRect, ClientRootRect, RootPoint, RootRect};
use super::model::{PlacementMode, PlacementSnapshot, ResizeEdges};
use crate::size_hints::{ClientSizeHints, WinGravity, snap_client_size};

/// Why a geometry plan was requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GeometryReason {
    InitialMap,
    InteractiveMove,
    InteractiveResize,
    ClientConfigure,
}

/// Frame chrome extents in pixels.
///
/// Old-good model (see `manage`/`configure_frame` in the pre-refactor wm):
/// the X frame interior width equals the client width, the interior height
/// equals titlebar + client height, the reparented client sits at local
/// (0, titlebar), and the X border is an outer frame property only — it is
/// never added to the local offset or size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameExtents {
    pub titlebar_h: i32,
    pub border: i32,
}

impl FrameExtents {
    #[must_use]
    pub const fn new(titlebar_h: i32, border: i32) -> Self {
        Self { titlebar_h, border }
    }

    /// Extra frame pixels over the client size: `(dw, dh)`.
    ///
    /// Border never inflates the client area: only the titlebar counts.
    #[must_use]
    pub const fn delta(self) -> (i32, i32) {
        (0, self.titlebar_h)
    }

    /// Client origin inside the frame: reparented at (0, titlebar).
    #[must_use]
    pub const fn client_offset(self) -> (i32, i32) {
        (0, self.titlebar_h)
    }
}

/// Pure input to every planner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeometryRequest<'a> {
    pub reason: GeometryReason,
    pub start: PlacementSnapshot,
    pub pointer_delta: (i32, i32),
    pub edges: ResizeEdges,
    pub work_area: RootRect,
    /// Output geometries; drags keep 1/4 of the window visible on these, not on the work area.
    /// Empty means no output info: `work_area` is used as the bound.
    pub outputs: &'a [RootRect],
    pub hints: ClientSizeHints,
    pub frame_extents: FrameExtents,
}

/// Pure planner output: frame rect plus derived client geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeometryPlan {
    pub frame: RootRect,
    /// Client origin offset inside the frame (0, titlebar).
    pub client_offset: (i32, i32),
    pub client_size: (u32, u32),
    /// True when the plan equals `start.rect` (no-op).
    pub noop: bool,
    pub keep_mode: PlacementMode,
}

impl GeometryPlan {
    fn of(reason_start: &GeometryRequest, frame: RootRect) -> Self {
        let (dw, dh) = reason_start.frame_extents.delta();
        let offset = reason_start.frame_extents.client_offset();
        let cw = frame.w.saturating_sub(dw).max(1) as u32;
        let ch = frame.h.saturating_sub(dh).max(1) as u32;
        let noop = frame == reason_start.start.rect;
        Self {
            frame,
            client_offset: offset,
            client_size: (cw, ch),
            noop,
            keep_mode: reason_start.start.mode,
        }
    }
}

/// Minimum visible extent on one axis: a quarter of the window, rounded up.
fn min_visible(extent: i32) -> i32 {
    extent.max(0).saturating_add(3) / 4
}

/// Start of a span of `len` so that at least a quarter of it stays inside `[lo, hi)`.
fn keep_span(start: i32, len: i32, lo: i32, hi: i32) -> i32 {
    let min_start = lo.saturating_sub(len.saturating_sub(min_visible(len)));
    let max_start = hi.saturating_sub(min_visible(len)).max(min_start);
    start.clamp(min_start, max_start)
}

/// Keeps `rect` in `bounds` with at least a quarter of each axis visible; up to 3/4 may overflow.
#[must_use]
pub fn keep_visible(rect: RootRect, bounds: RootRect) -> RootRect {
    RootRect::new(
        keep_span(rect.x, rect.w, bounds.x, bounds.x.saturating_add(bounds.w)),
        keep_span(rect.y, rect.h, bounds.y, bounds.y.saturating_add(bounds.h)),
        rect.w,
        rect.h,
    )
}

/// Bounding box of the outputs, or `fallback` when there are none.
fn outputs_bounds(outputs: &[RootRect], fallback: RootRect) -> RootRect {
    let Some((first, rest)) = outputs.split_first() else {
        return fallback;
    };
    let (mut x0, mut y0) = (first.x, first.y);
    let (mut x1, mut y1) = (
        first.x.saturating_add(first.w),
        first.y.saturating_add(first.h),
    );
    for o in rest {
        x0 = x0.min(o.x);
        y0 = y0.min(o.y);
        x1 = x1.max(o.x.saturating_add(o.w));
        y1 = y1.max(o.y.saturating_add(o.h));
    }
    RootRect::new(x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
}

/// True when `r` keeps a quarter of its width and height visible on the single output `o`.
fn visible_on(r: RootRect, o: RootRect) -> bool {
    let vw =
        r.x.saturating_add(r.w)
            .min(o.x.saturating_add(o.w))
            .saturating_sub(r.x.max(o.x));
    let vh =
        r.y.saturating_add(r.h)
            .min(o.y.saturating_add(o.h))
            .saturating_sub(r.y.max(o.y));
    vw >= min_visible(r.w) && vh >= min_visible(r.h)
}

/// Squared gap distance between `r` and `o` (0 when they touch or overlap).
fn gap_sq(r: RootRect, o: RootRect) -> i64 {
    let (r_right, o_right) = (
        i64::from(r.x) + i64::from(r.w),
        i64::from(o.x) + i64::from(o.w),
    );
    let (r_bottom, o_bottom) = (
        i64::from(r.y) + i64::from(r.h),
        i64::from(o.y) + i64::from(o.h),
    );
    let dx = (i64::from(o.x) - r_right)
        .max(i64::from(r.x) - o_right)
        .max(0);
    let dy = (i64::from(o.y) - r_bottom)
        .max(i64::from(r.y) - o_bottom)
        .max(0);
    dx * dx + dy * dy
}

/// Keeps `rect` visible on the outputs. Bounds are the outputs' bounding box; a rect parked in a
/// gap of a non-rectangular union moves minimally onto the nearest output.
#[must_use]
pub fn keep_on_outputs(rect: RootRect, outputs: &[RootRect], fallback: RootRect) -> RootRect {
    let bounded = keep_visible(rect, outputs_bounds(outputs, fallback));
    let Some(nearest) = outputs.iter().copied().min_by_key(|o| gap_sq(bounded, *o)) else {
        return bounded;
    };
    if outputs.iter().any(|o| visible_on(bounded, *o)) {
        bounded
    } else {
        keep_visible(bounded, nearest)
    }
}

/// Valid `[lo, hi]` for a left edge at `x` while the right edge `right` is fixed, against `[b0, b1)`.
/// Visible width must stay >= a quarter of the width (ceil); derived from `4 * visible >= width`.
fn left_edge_range(right: i32, b0: i32, b1: i32) -> (i32, i32) {
    let (r, b0, b1) = (i64::from(right), i64::from(b0), i64::from(b1));
    let m = r.min(b1);
    let outside_min = r - 4 * (m - b0);
    let inside_max = (4 * m - r).div_euclid(3);
    let hi = (b0 - 1).max(inside_max).min(r - 1);
    let lo = outside_min.min(b0).min(hi);
    (to_i32(lo), to_i32(hi))
}

/// Valid `[lo, hi]` for a right edge at `x` while the left edge `left` is fixed, against `[b0, b1)`.
fn right_edge_range(left: i32, b0: i32, b1: i32) -> (i32, i32) {
    let (l, b0, b1) = (i64::from(left), i64::from(b0), i64::from(b1));
    let a = l.max(b0);
    let inside_min = (4 * a - l + 2).div_euclid(3);
    let outside_max = l + 4 * (b1 - a);
    let lo = inside_min.min(b1 + 1).max(l + 1);
    let hi = b1.max(outside_max).max(lo);
    (to_i32(lo), to_i32(hi))
}

fn to_i32(v: i64) -> i32 {
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// Interactive move: candidate = start + delta (never incremental),
/// kept visible on the outputs (see [`keep_on_outputs`]).
#[must_use]
pub fn plan_move(req: &GeometryRequest<'_>) -> GeometryPlan {
    let s = req.start.rect;
    let x = s.x.saturating_add(req.pointer_delta.0);
    let y = s.y.saturating_add(req.pointer_delta.1);
    let frame = keep_on_outputs(RootRect::new(x, y, s.w, s.h), req.outputs, req.work_area);
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
    let raw_w = if span_w > 0 { raw_w.min(span_w) } else { raw_w };
    let raw_h = if span_h > 0 { raw_h.min(span_h) } else { raw_h };

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

/// Outer frame rect -> client-local rect: origin (0, titlebar), size
/// (frame.w, frame.h - titlebar).
#[must_use]
pub fn frame_to_client_local(frame: RootRect, extents: FrameExtents) -> ClientLocalRect {
    let (_, dh) = extents.delta();
    let (ox, oy) = extents.client_offset();
    ClientLocalRect::new(ox, oy, frame.w, frame.h.saturating_sub(dh).max(1))
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
#[must_use]
pub fn plan_client_configure(
    req: &GeometryRequest,
    req_client_origin: RootPoint,
    req_client_size: Option<(u32, u32)>,
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
    let origin = if req_client_size.is_some() {
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
/// come from an application ConfigureWindow, so we conservatively preserve it.
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
    const OFFSET: i32 = 24;
    let target_frame_x = pointer.x.saturating_add(OFFSET);
    let target_frame_y = pointer.y.saturating_add(OFFSET);
    let origin = RootPoint::new(
        target_frame_x,
        target_frame_y.saturating_add(req.frame_extents.titlebar_h),
    );
    let localized = GeometryRequest {
        work_area: selected_work_area,
        ..*req
    };
    plan_initial_map(&localized, origin, req_client_size)
}

/// Clamp a span of `len` at `start` so it fits inside `[lo, lo + extent)`.
fn clamp_span(start: i32, len: i32, lo: i32, extent: i32) -> i32 {
    start.clamp(lo, lo.saturating_add(extent.saturating_sub(len).max(0)))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::model::SnapTarget;
    use crate::size_hints::{
        HINT_P_ASPECT, HINT_P_BASE_SIZE, HINT_P_MAX_SIZE, HINT_P_MIN_SIZE, HINT_P_RESIZE_INC,
        HINT_P_WIN_GRAVITY,
    };

    const EXT: FrameExtents = FrameExtents::new(31, 1);
    const WA: RootRect = RootRect::new(0, 0, 1920, 1080);
    const START: RootRect = RootRect::new(100, 100, 400, 300);
    const OUTS: &[RootRect] = &[WA];

    fn req(
        edges: ResizeEdges,
        delta: (i32, i32),
        hints: ClientSizeHints,
    ) -> GeometryRequest<'static> {
        GeometryRequest {
            reason: GeometryReason::InteractiveResize,
            start: PlacementSnapshot {
                mode: PlacementMode::Floating,
                rect: START,
            },
            pointer_delta: delta,
            edges,
            work_area: WA,
            outputs: &[],
            hints,
            frame_extents: EXT,
        }
    }

    fn client_configure(hints: ClientSizeHints) -> GeometryRequest<'static> {
        GeometryRequest {
            reason: GeometryReason::ClientConfigure,
            ..req(ResizeEdges::none(), (0, 0), hints)
        }
    }

    fn free() -> ClientSizeHints {
        ClientSizeHints::default()
    }

    fn min_max(min: (u32, u32), max: (u32, u32)) -> ClientSizeHints {
        ClientSizeHints {
            min: Some(min),
            max: Some(max),
            resizable: min != max,
            ..ClientSizeHints::default()
        }
    }

    #[test]
    fn adopt_is_inverse_of_frame_reparent_without_clamp_or_drift() {
        // Frame at (0,0) outside a top panel work area: client abs is
        // (border, border + titlebar). Adopt must return (0,0) again.
        let frame = plan_adopt(RootPoint::new(1, 32), (400, 300), EXT);
        assert_eq!(frame, RootRect::new(0, 0, 400, 331));
        let again = plan_adopt(RootPoint::new(1, 32), (400, 300), EXT);
        assert_eq!(again, frame);
    }

    #[test]
    fn cursor_placement_respects_output_origin_and_panel_struts() {
        // Each work area comes from the selected physical output, never the root union.
        let cases = [
            (
                RootPoint::new(150, 170),
                RootRect::new(0, 36, 1920, 1044),
                RootRect::new(174, 194, 700, 531),
            ),
            (
                RootPoint::new(2150, 50),
                RootRect::new(1920, 0, 1280, 984),
                RootRect::new(2174, 74, 700, 531),
            ),
            (
                RootPoint::new(-1810, 100),
                RootRect::new(-1920, 0, 1920, 1030),
                RootRect::new(-1786, 124, 700, 531),
            ),
            (
                RootPoint::new(1700, 100),
                RootRect::new(56, 0, 2504, 1440),
                RootRect::new(1724, 124, 700, 531),
            ),
            // Bottom/right edge: fit entirely within the same hovered output.
            (
                RootPoint::new(3150, 950),
                RootRect::new(1920, 0, 1280, 984),
                RootRect::new(2500, 453, 700, 531),
            ),
            // Left panel and right panel work areas.
            (
                RootPoint::new(20, 70),
                RootRect::new(64, 0, 1856, 1080),
                RootRect::new(64, 94, 700, 531),
            ),
            (
                RootPoint::new(1880, 40),
                RootRect::new(0, 0, 1860, 1080),
                RootRect::new(1160, 64, 700, 531),
            ),
        ];
        for (pointer, work_area, expected) in cases {
            let request = GeometryRequest {
                reason: GeometryReason::InitialMap,
                ..req(ResizeEdges::none(), (0, 0), free())
            };
            assert_eq!(
                plan_initial_map_near_pointer(&request, pointer, work_area, (700, 500)).frame,
                expected,
                "pointer {pointer:?}, work area {work_area:?}",
            );
        }
    }

    #[test]
    fn oversize_cursor_window_preserves_reachable_titlebar() {
        let request = GeometryRequest {
            reason: GeometryReason::InitialMap,
            ..req(ResizeEdges::none(), (0, 0), free())
        };
        let area = RootRect::new(-1920, 48, 900, 580);
        let plan =
            plan_initial_map_near_pointer(&request, RootPoint::new(-1100, 620), area, (1400, 900));
        assert_eq!(plan.frame, RootRect::new(-1920, 48, 1400, 931));
        assert_eq!(plan.client_size, (1400, 900));
    }

    #[test]
    fn initial_cursor_policy_preserves_all_special_client_origins() {
        let origin = RootPoint::new(0, 0);
        assert!(can_place_near_pointer(false, true, false, free(), origin));
        assert!(!can_place_near_pointer(true, true, false, free(), origin));
        assert!(!can_place_near_pointer(false, false, false, free(), origin));
        assert!(!can_place_near_pointer(false, true, true, free(), origin));
        assert!(!can_place_near_pointer(
            false,
            true,
            false,
            free(),
            RootPoint::new(-1920, 0)
        ));
        assert!(!can_place_near_pointer(
            false,
            true,
            false,
            free(),
            RootPoint::new(200, 130)
        ));
        assert!(!can_place_near_pointer(
            false,
            true,
            false,
            ClientSizeHints {
                position_specified: true,
                ..free()
            },
            origin,
        ));
    }

    #[test]
    fn pointer_placement_keeps_size_hints_and_adoption_unchanged() {
        let hints = min_max((350, 250), (600, 400));
        let request = GeometryRequest {
            reason: GeometryReason::InitialMap,
            ..req(ResizeEdges::none(), (0, 0), hints)
        };
        let plan =
            plan_initial_map_near_pointer(&request, RootPoint::new(100, 100), WA, (900, 600));
        assert_eq!(plan.client_size, (600, 400));
        assert_eq!(plan.frame, RootRect::new(124, 124, 600, 431));
        assert_eq!(
            plan_adopt(RootPoint::new(120, 160), (600, 400), EXT),
            RootRect::new(119, 128, 600, 431)
        );
    }

    #[test]
    fn resize_right_fixes_left_edge() {
        let r = req(ResizeEdges::right(), (50, 0), free());
        let p = plan_resize(&r);
        assert_eq!(p.frame.x, 100);
        assert_eq!(p.frame.w, 450);
        assert_eq!(p.frame.h, 300);
        assert!(!p.noop);
    }

    #[test]
    fn resize_left_fixes_right_edge() {
        let r = req(ResizeEdges::left(), (50, 0), free());
        let p = plan_resize(&r);
        // Fixed right = 500; constrained width 350 -> x = 150.
        assert_eq!(p.frame.x + p.frame.w, 500);
        assert_eq!(p.frame.w, 350);
        assert_eq!(p.frame.y, 100);
    }

    #[test]
    fn resize_top_fixes_bottom_edge() {
        let r = req(ResizeEdges::top(), (0, 40), free());
        let p = plan_resize(&r);
        assert_eq!(p.frame.y + p.frame.h, 400);
        assert_eq!(p.frame.h, 260);
        assert_eq!(p.frame.x, 100);
    }

    #[test]
    fn resize_bottom_fixes_top_edge() {
        let r = req(ResizeEdges::bottom(), (0, 40), free());
        let p = plan_resize(&r);
        assert_eq!(p.frame.y, 100);
        assert_eq!(p.frame.h, 340);
    }

    #[test]
    fn resize_top_left_fixes_opposite_corner() {
        let r = req(ResizeEdges::top_left(), (20, 30), free());
        let p = plan_resize(&r);
        assert_eq!(p.frame.x + p.frame.w, 500);
        assert_eq!(p.frame.y + p.frame.h, 400);
        assert_eq!((p.frame.w, p.frame.h), (380, 270));
    }

    #[test]
    fn resize_top_right_fixes_opposite_corner() {
        let r = req(ResizeEdges::top_right(), (20, 30), free());
        let p = plan_resize(&r);
        assert_eq!(p.frame.x, 100);
        assert_eq!(p.frame.y + p.frame.h, 400);
        assert_eq!((p.frame.w, p.frame.h), (420, 270));
    }

    #[test]
    fn resize_bottom_left_fixes_opposite_corner() {
        let r = req(ResizeEdges::bottom_left(), (20, 30), free());
        let p = plan_resize(&r);
        assert_eq!(p.frame.x + p.frame.w, 500);
        assert_eq!(p.frame.y, 100);
        assert_eq!((p.frame.w, p.frame.h), (380, 330));
    }

    #[test]
    fn resize_bottom_right_fixes_opposite_corner() {
        let r = req(ResizeEdges::bottom_right(), (20, 30), free());
        let p = plan_resize(&r);
        assert_eq!(p.frame.x, 100);
        assert_eq!(p.frame.y, 100);
        assert_eq!((p.frame.w, p.frame.h), (420, 330));
    }

    #[test]
    fn move_is_start_plus_delta_and_never_incremental() {
        let mut r = req(ResizeEdges::none(), (60, 25), free());
        r.reason = GeometryReason::InteractiveMove;
        let p = plan_move(&r);
        assert_eq!(p.frame, RootRect::new(160, 125, 400, 300));
        // Same delta applied to the plan output must equal applying twice
        // the delta to the start (no drift), not delta-on-delta.
        let mut r2 = r;
        r2.start.rect = p.frame;
        let p2 = plan_move(&r2);
        assert_eq!(p2.frame, RootRect::new(220, 150, 400, 300));
        assert_eq!(p2.frame.x - START.x, 120);
    }

    fn move_by(delta: (i32, i32)) -> RootRect {
        let mut r = req(ResizeEdges::none(), delta, free());
        r.reason = GeometryReason::InteractiveMove;
        r.outputs = OUTS;
        plan_move(&r).frame
    }

    #[test]
    fn move_stops_at_three_quarters_outside_each_side() {
        // START 400x300 on WA 1920x1080: at most 3/4 may leave, so 100x75 stays visible.
        // Old work-area clamp would pin these at x 1520 / 0 and y 0 / 780.
        assert_eq!(move_by((5000, 0)), RootRect::new(1820, 100, 400, 300));
        assert_eq!(move_by((-5000, 0)), RootRect::new(-300, 100, 400, 300));
        assert_eq!(move_by((0, -5000)), RootRect::new(100, -225, 400, 300));
        assert_eq!(move_by((0, 5000)), RootRect::new(100, 1005, 400, 300));
    }

    #[test]
    fn move_over_taskbar_band_is_allowed() {
        // A 24px taskbar at the bottom is not excluded from the drag bound.
        assert_eq!(move_by((0, 800)), RootRect::new(100, 900, 400, 300));
    }

    #[test]
    fn oversized_window_keeps_quarter_visible_and_never_wraps() {
        let mut r = req(ResizeEdges::none(), (-5000, -5000), free());
        r.reason = GeometryReason::InteractiveMove;
        r.start.rect = RootRect::new(0, 0, 3000, 2000);
        r.outputs = OUTS;
        // min visible = 750 x 500 of the 3000 x 2000 window.
        assert_eq!(plan_move(&r).frame, RootRect::new(-2250, -1500, 3000, 2000));
    }

    #[test]
    fn negative_origin_output_bounds_move_without_unsigned_wrap() {
        let left = RootRect::new(-1920, 0, 1920, 1080);
        let mut r = req(ResizeEdges::none(), (-5000, 0), free());
        r.reason = GeometryReason::InteractiveMove;
        let outs = [left, WA];
        r.outputs = &outs;
        assert_eq!(plan_move(&r).frame, RootRect::new(-2220, 100, 400, 300));
    }

    #[test]
    fn window_parked_in_gap_of_output_union_moves_to_nearest_output() {
        let a = RootRect::new(0, 0, 800, 600);
        let b = RootRect::new(1000, 0, 800, 600);
        // 100 px window fully inside the 200 px gap: no output keeps 1/4 visible.
        // Nearest is A (touching at x = 800): moved to x = 775 so 25 px stay on A.
        assert_eq!(
            keep_on_outputs(RootRect::new(800, 10, 100, 100), &[a, b], WA),
            RootRect::new(775, 10, 100, 100)
        );
        // A window straddling both outputs with 1/4 on one of them is left alone.
        assert_eq!(
            keep_on_outputs(RootRect::new(600, 10, 400, 300), &[a, b], WA),
            RootRect::new(600, 10, 400, 300)
        );
    }

    #[test]
    fn resize_top_left_past_work_area_stops_at_work_area_edge() {
        let mut r = req(ResizeEdges::top_left(), (-5000, -5000), free());
        r.outputs = OUTS;
        let p = plan_resize(&r);
        // Fixed bottom-right corner is (500, 400); left and top stop at the work area's 0,0 edge.
        assert_eq!(p.frame, RootRect::new(0, 0, 500, 400));
        assert_eq!(p.frame.x + p.frame.w, 500);
        assert_eq!(p.frame.y + p.frame.h, 400);
    }

    #[test]
    fn resize_stops_at_work_area_edge_not_output_edge() {
        // 56 px taskbar reserve: the work area ends at y=1024 while the output ends at 1080.
        let mut r = req(ResizeEdges::bottom_right(), (5000, 5000), free());
        r.work_area = RootRect::new(0, 0, 1920, 1024);
        let outputs = [RootRect::new(0, 0, 1920, 1080)];
        r.outputs = &outputs;
        let p = plan_resize(&r);
        // Fixed top-left corner is (100, 100); the frame fills the work area to its right/bottom edges.
        assert_eq!(p.frame, RootRect::new(100, 100, 1820, 924));
        assert!(p.frame.y + p.frame.h <= 1024);
    }

    #[test]
    fn resize_fixed_edge_outside_work_area_is_not_capped() {
        // Fixed left edge already right of the work area: no span to cap against, so the
        // raw resize result stands (the output-bounds rule still applies).
        let mut r = req(ResizeEdges::right(), (100, 0), free());
        r.start.rect = RootRect::new(2000, 100, 400, 300);
        r.work_area = RootRect::new(0, 0, 1920, 1024);
        let outputs = [RootRect::new(0, 0, 1920, 1080)];
        r.outputs = &outputs;
        let p = plan_resize(&r);
        assert_eq!(p.frame.x, 2000);
        assert!(p.frame.w > 0);
    }

    #[test]
    fn resize_moving_edge_stops_at_work_area_edge() {
        let mut r = req(ResizeEdges::right(), (5000, 0), free());
        r.start.rect = RootRect::new(1500, 100, 400, 300);
        r.outputs = OUTS;
        let p = plan_resize(&r);
        // Left edge fixed at 1500; the right edge stops at the 1920 px work-area edge.
        assert_eq!(p.frame, RootRect::new(1500, 100, 420, 300));
    }

    #[test]
    fn resize_min_floor_reconstructs_dragged_edge() {
        // Frame delta = (0, 31); min client 200x150 -> min frame 200x181.
        let r = req(
            ResizeEdges::left(),
            (300, 0),
            min_max((200, 150), (2000, 2000)),
        );
        let p = plan_resize(&r);
        assert_eq!(p.frame.w, 200);
        assert_eq!(p.frame.x + p.frame.w, 500);
    }

    #[test]
    fn resize_max_ceiling_reconstructs_dragged_edge() {
        let r = req(
            ResizeEdges::bottom_right(),
            (2000, 2000),
            min_max((100, 100), (500, 400)),
        );
        let p = plan_resize(&r);
        // Max client 500x400 -> frame 500x431, anchored at fixed top-left.
        assert_eq!((p.frame.w, p.frame.h), (500, 431));
        assert_eq!((p.frame.x, p.frame.y), (100, 100));
    }

    #[test]
    fn resize_increment_snaps_in_8px_steps() {
        let hints = ClientSizeHints {
            base: Some((100, 100)),
            inc: Some((8, 8)),
            ..ClientSizeHints::default()
        };
        // Client width 400+50=450 raw -> base 100 + 43*8 = 444 -> frame 444.
        let r = req(ResizeEdges::right(), (50, 0), hints);
        let p = plan_resize(&r);
        assert_eq!(p.client_size.0 % 8, 100 % 8);
        assert_eq!(p.frame.w, 444);
    }

    #[test]
    fn resize_aspect_shrinks_width() {
        let hints = ClientSizeHints {
            max_aspect: Some((2, 1)),
            ..ClientSizeHints::default()
        };
        // Drag far wider than 2:1; width must shrink to 2*h.
        let r = req(ResizeEdges::right(), (1500, 0), hints);
        let p = plan_resize(&r);
        let (cw, ch) = p.client_size;
        assert!(u64::from(cw) <= 2u64 * u64::from(ch));
        assert_eq!(p.frame.x, 100);
    }

    #[test]
    fn resize_fixed_size_window_is_noop() {
        let hints = ClientSizeHints {
            min: Some((400, 269)),
            max: Some((400, 269)),
            resizable: false,
            ..ClientSizeHints::default()
        };
        let r = req(ResizeEdges::bottom_right(), (80, 60), hints);
        let p = plan_resize(&r);
        assert_eq!(p.frame, START);
        assert!(p.noop);
    }

    #[test]
    fn gravity_static_keeps_origin() {
        let o = RootPoint::new(200, 150);
        let n = gravity_adjust_origin(o, (400, 300), (200, 100), WinGravity::Static);
        assert_eq!(n, o);
    }

    #[test]
    fn gravity_northwest_keeps_origin() {
        let o = RootPoint::new(200, 150);
        let n = gravity_adjust_origin(o, (400, 300), (200, 100), WinGravity::NorthWest);
        assert_eq!(n, o);
    }

    #[test]
    fn gravity_southeast_moves_origin_by_full_delta() {
        let o = RootPoint::new(200, 150);
        let n = gravity_adjust_origin(o, (400, 300), (200, 100), WinGravity::SouthEast);
        // SE corner fixed: 200+400=600 -> x = 600-200 = 400; 150+300=450 -> y = 350.
        assert_eq!(n, RootPoint::new(400, 350));
    }

    #[test]
    fn gravity_north_centers_horizontally() {
        let o = RootPoint::new(200, 150);
        let n = gravity_adjust_origin(o, (400, 300), (200, 100), WinGravity::North);
        // Top-center fixed: old cx = 400 -> x = 400-100 = 300; top anchored.
        assert_eq!(n, RootPoint::new(300, 150));
    }

    #[test]
    fn gravity_south_anchors_bottom() {
        let o = RootPoint::new(200, 150);
        let n = gravity_adjust_origin(o, (400, 300), (200, 300), WinGravity::South);
        // Bottom-center fixed: old cx = 400 -> x = 300; bottom 450 unchanged.
        assert_eq!(n, RootPoint::new(300, 150));
        let n2 = gravity_adjust_origin(o, (400, 300), (400, 100), WinGravity::South);
        // Bottom fixed: 150+300=450 -> y = 450-100 = 350.
        assert_eq!(n2, RootPoint::new(200, 350));
    }

    #[test]
    fn gravity_center_and_east_west() {
        let o = RootPoint::new(200, 150);
        assert_eq!(
            gravity_adjust_origin(o, (400, 300), (200, 100), WinGravity::Center),
            RootPoint::new(300, 250)
        );
        assert_eq!(
            gravity_adjust_origin(o, (400, 300), (200, 300), WinGravity::West),
            RootPoint::new(200, 150)
        );
        assert_eq!(
            gravity_adjust_origin(o, (400, 300), (200, 300), WinGravity::East),
            RootPoint::new(400, 150)
        );
    }

    #[test]
    fn gravity_parses_from_size_hints() {
        let mut values = vec![0u32; 18];
        values[0] |= HINT_P_WIN_GRAVITY;
        values[17] = 9; // SouthEast
        let hints = ClientSizeHints::parse(&values);
        assert_eq!(WinGravity::of(&hints), WinGravity::SouthEast);
        assert_eq!(
            WinGravity::of(&ClientSizeHints::default()),
            WinGravity::NorthWest
        );
    }

    #[test]
    fn client_rect_frame_roundtrip() {
        let local = frame_to_client_local(START, EXT);
        assert_eq!(local, ClientLocalRect::new(0, 31, 400, 269));
        let root = frame_to_client_root(START, EXT);
        assert_eq!(root, ClientRootRect::new(100, 131, 400, 269));
        assert_eq!(root.size_u32(), (400, 269));
        assert_eq!(
            client_rect_to_frame(root.origin(), root.size_u32(), EXT),
            START
        );
    }

    #[test]
    fn client_configure_applies_hints_and_gravity() {
        // min 100x100, max 500x400; request 2000x2000 at (300,300).
        let r = client_configure(min_max((100, 100), (500, 400)));
        let p = plan_client_configure(&r, RootPoint::new(300, 300), Some((2000, 2000)));
        // NW gravity: origin pinned to previous client origin (100,131).
        assert_eq!(p.client_size, (500, 400));
        assert_eq!(p.frame, RootRect::new(100, 100, 500, 431));
    }

    #[test]
    fn client_configure_position_only_moves_frame() {
        let r = client_configure(free());
        let p = plan_client_configure(&r, RootPoint::new(300, 300), None);
        assert_eq!(p.frame, RootRect::new(300, 269, 400, 300));
        assert_eq!(p.client_size, (400, 269));
    }

    /// Position-only request for a frame origin `(x, y)` on a 400x300 START window.
    fn configure_at(
        outs: &'static [RootRect],
        frame_xy: (i32, i32),
        size: Option<(u32, u32)>,
    ) -> RootRect {
        let r = GeometryRequest {
            reason: GeometryReason::ClientConfigure,
            outputs: outs,
            ..req(ResizeEdges::none(), (0, 0), free())
        };
        let (ox, oy) = EXT.client_offset();
        plan_client_configure(&r, RootPoint::new(frame_xy.0 + ox, frame_xy.1 + oy), size).frame
    }

    #[test]
    fn client_configure_far_offscreen_position_is_kept_visible() {
        // START 400x300 frame on WA 1920x1080, one output: 1/4 of each axis stays visible.
        const ONE: &[RootRect] = &[WA];
        assert_eq!(
            configure_at(ONE, (-10000, 100), None),
            RootRect::new(-300, 100, 400, 300)
        );
        assert_eq!(
            configure_at(ONE, (10000, 100), None),
            RootRect::new(1820, 100, 400, 300)
        );
        assert_eq!(
            configure_at(ONE, (100, -10000), None),
            RootRect::new(100, -225, 400, 300)
        );
        assert_eq!(
            configure_at(ONE, (100, 10000), None),
            RootRect::new(100, 1005, 400, 300)
        );
    }

    #[test]
    fn client_configure_clamps_on_second_output_with_negative_origin() {
        const TWO: &[RootRect] = &[
            RootRect::new(-1920, 0, 1920, 1080),
            RootRect::new(0, 0, 1920, 1080),
        ];
        // Far left lands at the left edge of the negative-origin output, not the work area.
        assert_eq!(
            configure_at(TWO, (-10000, 100), None),
            RootRect::new(-2220, 100, 400, 300)
        );
        assert_eq!(
            configure_at(TWO, (10000, 100), None),
            RootRect::new(1820, 100, 400, 300)
        );
    }

    #[test]
    fn client_configure_in_gap_moves_to_nearest_output() {
        const GAP: &[RootRect] = &[
            RootRect::new(0, 0, 800, 600),
            RootRect::new(1000, 0, 800, 600),
        ];
        // 100x100 frame fully inside the 200px gap (x 800..1000): nearest output A moves it to x 775.
        let (ox, oy) = EXT.client_offset();
        let r = GeometryRequest {
            reason: GeometryReason::ClientConfigure,
            start: PlacementSnapshot {
                mode: PlacementMode::Floating,
                rect: RootRect::new(0, 0, 100, 100),
            },
            outputs: GAP,
            ..req(ResizeEdges::none(), (0, 0), free())
        };
        let p = plan_client_configure(&r, RootPoint::new(820 + ox, 10 + oy), None);
        assert_eq!(p.frame, RootRect::new(775, 10, 100, 100));
    }

    #[test]
    fn client_configure_oversized_window_keeps_quarter_visible() {
        const ONE: &[RootRect] = &[WA];
        let r = GeometryRequest {
            reason: GeometryReason::ClientConfigure,
            start: PlacementSnapshot {
                mode: PlacementMode::Floating,
                rect: RootRect::new(0, 0, 3000, 2000),
            },
            outputs: ONE,
            ..req(ResizeEdges::none(), (0, 0), free())
        };
        let p = plan_client_configure(&r, RootPoint::new(-10000, 10000), None);
        assert_eq!(p.frame, RootRect::new(-2250, 1080 - 500, 3000, 2000));
    }

    #[test]
    fn client_configure_within_rule_is_unchanged() {
        const ONE: &[RootRect] = &[WA];
        assert_eq!(
            configure_at(ONE, (300, 300), None),
            RootRect::new(300, 300, 400, 300)
        );
        assert_eq!(
            configure_at(ONE, (-300, 100), None),
            RootRect::new(-300, 100, 400, 300)
        );
    }

    #[test]
    fn client_configure_snapped_is_not_clamped() {
        const ONE: &[RootRect] = &[WA];
        let mut r = GeometryRequest {
            reason: GeometryReason::ClientConfigure,
            outputs: ONE,
            ..req(ResizeEdges::none(), (0, 0), free())
        };
        r.start.mode = PlacementMode::Snapped(SnapTarget::Left);
        let p = plan_client_configure(&r, RootPoint::new(-10000, 100), None);
        assert_eq!(p.frame.x, -10000 - EXT.client_offset().0);
    }

    #[test]
    fn user_move_is_unchanged_by_configure_clamp() {
        // Same far-left target through the user drag path keeps the identical rect.
        assert_eq!(
            move_by((-10000, 0)),
            configure_at(&[WA], (-10000, 100), None)
        );
    }

    #[test]
    fn client_configure_maximized_is_noncorrupting() {
        let mut r = GeometryRequest {
            reason: GeometryReason::ClientConfigure,
            ..req(ResizeEdges::none(), (0, 0), free())
        };
        r.start.mode = PlacementMode::Maximized;
        r.start.rect = RootRect::new(0, 0, 1920, 1080);
        let p = plan_client_configure(&r, RootPoint::new(5, 5), Some((10, 10)));
        assert_eq!(p.frame, RootRect::new(0, 0, 1920, 1080));
        assert!(p.noop);
        assert_eq!(p.keep_mode, PlacementMode::Maximized);
    }

    #[test]
    fn client_configure_fullscreen_is_noncorrupting() {
        let mut r = GeometryRequest {
            reason: GeometryReason::ClientConfigure,
            ..req(ResizeEdges::none(), (0, 0), free())
        };
        r.start.mode = PlacementMode::Fullscreen;
        r.start.rect = RootRect::new(0, 0, 1920, 1080);
        let p = plan_client_configure(&r, RootPoint::new(5, 5), Some((10, 10)));
        assert_eq!(p.frame, RootRect::new(0, 0, 1920, 1080));
        assert!(p.noop);
    }

    #[test]
    fn client_configure_clamps_requested_size_to_min_max_hints() {
        let client_size = |hints: ClientSizeHints, w: u32, h: u32| {
            let r = client_configure(hints);
            let p = plan_client_configure(&r, RootPoint::new(100, 100), Some((w, h)));
            frame_to_client_root(p.frame, EXT).size_u32()
        };
        let fixed = min_max((275, 116), (275, 116));
        assert_eq!(client_size(fixed.clone(), 100, 50), (275, 116));
        assert_eq!(client_size(fixed.clone(), 10, 10), (275, 116));
        assert_eq!(client_size(fixed, 400, 300), (275, 116));
        let ranged = min_max((200, 100), (300, 200));
        assert_eq!(client_size(ranged.clone(), 50, 50), (200, 100));
        assert_eq!(client_size(ranged.clone(), 500, 500), (300, 200));
        let incs = ClientSizeHints {
            base: Some((0, 0)),
            inc: Some((10, 20)),
            ..min_max((10, 20), (1000, 1000))
        };
        assert_eq!(client_size(incs, 105, 55), (100, 40));
    }

    #[test]
    fn reason_variants_constructible() {
        for reason in [
            GeometryReason::InitialMap,
            GeometryReason::InteractiveMove,
            GeometryReason::InteractiveResize,
            GeometryReason::ClientConfigure,
        ] {
            let mut r = req(ResizeEdges::none(), (0, 0), free());
            r.reason = reason;
            let plan = GeometryPlan::of(&r, START);
            assert!(plan.noop);
            assert_eq!(plan.keep_mode, PlacementMode::Floating);
        }
    }

    #[test]
    fn right_bottom_right_local_origin_constant() {
        for edges in [
            ResizeEdges::right(),
            ResizeEdges::bottom_right(),
            ResizeEdges::none(),
        ] {
            let r = req(edges, (37, 19), free());
            let p = plan_resize(&r);
            let local = frame_to_client_local(p.frame, EXT);
            assert_eq!((local.x, local.y), (0, 31));
        }
    }

    #[test]
    fn border_is_outer_only_no_local_change() {
        let ext1 = FrameExtents::new(31, 0);
        let ext2 = FrameExtents::new(31, 5);
        assert_eq!(
            frame_to_client_local(START, ext1),
            frame_to_client_local(START, ext2)
        );
        assert_eq!(ext1.client_offset(), (0, 31));
        assert_eq!(ext2.client_offset(), (0, 31));
        assert_eq!(ext1.delta(), (0, 31));
        assert_eq!(ext2.delta(), (0, 31));
    }

    #[test]
    fn client_offset_matches_old_good_model() {
        // Reparented client local x=0 y=titlebar; interior w=client w,
        // h=titlebar+client h.
        assert_eq!(EXT.client_offset(), (0, 31));
        assert_eq!(EXT.delta(), (0, 31));
        let local = frame_to_client_local(START, EXT);
        assert_eq!(local.w, START.w);
        assert_eq!(local.h, START.h - 31);
    }

    #[test]
    fn synthetic_root_differs_from_local_at_nonzero_origin() {
        let frame = RootRect::new(400, 200, 400, 300);
        let local = frame_to_client_local(frame, EXT);
        let root = frame_to_client_root(frame, EXT);
        assert_eq!((local.x, local.y), (0, 31));
        assert_eq!(root.origin(), RootPoint::new(400, 231));
        assert_eq!(root.size_u32(), (400, 269));
    }

    #[test]
    fn move_frame_keeps_client_local() {
        let mut r = req(ResizeEdges::none(), (300, 100), free());
        r.reason = GeometryReason::InteractiveMove;
        let p = plan_move(&r);
        assert_eq!(p.frame, RootRect::new(400, 200, 400, 300));
        assert_eq!(
            frame_to_client_local(p.frame, EXT),
            ClientLocalRect::new(0, 31, 400, 269)
        );
    }

    #[test]
    fn parse_all_hint_fields_used_by_geometry() {
        let mut values = vec![0u32; 18];
        values[0] |= HINT_P_MIN_SIZE
            | HINT_P_MAX_SIZE
            | HINT_P_RESIZE_INC
            | HINT_P_ASPECT
            | HINT_P_BASE_SIZE
            | HINT_P_WIN_GRAVITY;
        values[5] = 100;
        values[6] = 100;
        values[7] = 800;
        values[8] = 600;
        values[9] = 8;
        values[10] = 8;
        values[11] = 1;
        values[12] = 1;
        values[13] = 2;
        values[14] = 1;
        values[15] = 100;
        values[16] = 100;
        values[17] = 1;
        let hints = ClientSizeHints::parse(&values);
        assert_eq!(hints.min, Some((100, 100)));
        assert_eq!(hints.max, Some((800, 600)));
        assert_eq!(hints.inc, Some((8, 8)));
        assert_eq!(hints.base, Some((100, 100)));
        assert_eq!(hints.min_aspect, Some((1, 1)));
        assert_eq!(hints.max_aspect, Some((2, 1)));
        assert_eq!(hints.gravity, Some(1));
        let r = req(ResizeEdges::bottom_right(), (75, 75), hints);
        let p = plan_resize(&r);
        assert!(p.client_size.0 >= 100 && p.client_size.1 >= 100);
    }
}
