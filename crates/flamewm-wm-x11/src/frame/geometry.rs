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
pub struct GeometryRequest {
    pub reason: GeometryReason,
    pub start: PlacementSnapshot,
    pub pointer_delta: (i32, i32),
    pub edges: ResizeEdges,
    pub work_area: RootRect,
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

/// Interactive move: candidate = start + delta (never incremental),
/// clamped so the frame top-left stays inside the work area.
#[must_use]
pub fn plan_move(req: &GeometryRequest) -> GeometryPlan {
    let s = req.start.rect;
    let wa = req.work_area;
    let mut x = s.x.saturating_add(req.pointer_delta.0);
    let mut y = s.y.saturating_add(req.pointer_delta.1);
    // Clamp top-left inside; oversized frames pin to the work-area origin.
    let max_x = wa.x.saturating_add(wa.w.saturating_sub(s.w).max(0));
    let max_y = wa.y.saturating_add(wa.h.saturating_sub(s.h).max(0));
    x = x.clamp(wa.x.min(max_x), max_x.max(wa.x));
    y = y.clamp(wa.y.min(max_y), max_y.max(wa.y));
    GeometryPlan::of(req, RootRect::new(x, y, s.w, s.h))
}

/// Interactive resize with a fixed opposite-edge invariant.
///
/// 1. Fix the edge(s) opposite the drag from `start.rect`.
/// 2. Constrain only the moving edge against the work area.
/// 3. Constrain the requested client size via [`snap_client_size`]
///    (min/max/base/inc/aspect).
/// 4. Reconstruct the dragged edge as fixed edge +/- constrained size.
///
/// No `clamp_inside`-after-resize: the fixed edge never moves, even if the
/// constrained size pushes the moving edge outside the work area.
#[must_use]
pub fn plan_resize(req: &GeometryRequest) -> GeometryPlan {
    let s = req.start.rect;
    let (dx, dy) = req.pointer_delta;
    let wa = req.work_area;
    let (dw, dh) = req.frame_extents.delta();

    let fixed_left = s.x;
    let fixed_right = s.x.saturating_add(s.w);
    let fixed_top = s.y;
    let fixed_bottom = s.y.saturating_add(s.h);
    let wa_right = wa.x.saturating_add(wa.w);
    let wa_bottom = wa.y.saturating_add(wa.h);

    // Desired moving edges, constrained against the work area only.
    // Left/top moving edges must leave at least a 1px frame; the size-hint
    // floor is applied later via snap_client_size.
    let mut moving_left = s.x.saturating_add(dx);
    let mut moving_right = fixed_right.saturating_add(dx);
    let mut moving_top = s.y.saturating_add(dy);
    let mut moving_bottom = fixed_bottom.saturating_add(dy);
    if req.edges.left {
        moving_left = moving_left.clamp(wa.x, fixed_right.saturating_sub(1));
    }
    if req.edges.right {
        moving_right = moving_right.clamp(fixed_left.saturating_add(1), wa_right);
    }
    if req.edges.top {
        moving_top = moving_top.clamp(wa.y, fixed_bottom.saturating_sub(1));
    }
    if req.edges.bottom {
        moving_bottom = moving_bottom.clamp(fixed_top.saturating_add(1), wa_bottom);
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
    match req.start.mode {
        PlacementMode::Maximized | PlacementMode::Fullscreen => {
            let mut plan = GeometryPlan::of(req, req.start.rect);
            plan.noop = true;
            return plan;
        }
        PlacementMode::Floating | PlacementMode::Snapped(_) => {}
    }
    let gravity = WinGravity::of(&req.hints);
    let prev_root = frame_to_client_root(req.start.rect, req.frame_extents);
    let prev_size = prev_root.size_u32();
    let prev_origin = prev_root.origin();
    let new_size = match req_client_size {
        Some((w, h)) => snap_client_size(w.max(1), h.max(1), &req.hints),
        None => prev_size,
    };
    // Gravity moves the requested origin only when the size changed.
    let base_origin = if req_client_size.is_some() {
        gravity_adjust_origin(prev_origin, prev_size, new_size, gravity)
    } else {
        req_client_origin
    };
    // A bare position request (no size) honors the requested origin.
    let origin = if req_client_size.is_some() {
        let _ = req_client_origin;
        base_origin
    } else {
        req_client_origin
    };
    GeometryPlan::of(
        req,
        client_rect_to_frame(origin, new_size, req.frame_extents),
    )
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
    let x = frame.x.clamp(
        wa.x,
        wa.x.saturating_add(wa.w.saturating_sub(frame.w).max(0)),
    );
    let y = frame.y.clamp(
        wa.y,
        wa.y.saturating_add(wa.h.saturating_sub(frame.h).max(0)),
    );
    GeometryPlan::of(req, RootRect::new(x, y, frame.w, frame.h))
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
    use crate::size_hints::{
        HINT_P_ASPECT, HINT_P_BASE_SIZE, HINT_P_MAX_SIZE, HINT_P_MIN_SIZE, HINT_P_RESIZE_INC,
        HINT_P_WIN_GRAVITY,
    };

    const EXT: FrameExtents = FrameExtents::new(31, 1);
    const WA: RootRect = RootRect::new(0, 0, 1920, 1080);
    const START: RootRect = RootRect::new(100, 100, 400, 300);

    fn req(edges: ResizeEdges, delta: (i32, i32), hints: ClientSizeHints) -> GeometryRequest {
        GeometryRequest {
            reason: GeometryReason::InteractiveResize,
            start: PlacementSnapshot {
                mode: PlacementMode::Floating,
                rect: START,
            },
            pointer_delta: delta,
            edges,
            work_area: WA,
            hints,
            frame_extents: EXT,
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

    #[test]
    fn move_clamps_inside_work_area() {
        let mut r = req(ResizeEdges::none(), (5000, 5000), free());
        r.reason = GeometryReason::InteractiveMove;
        r.work_area = RootRect::new(0, 0, 800, 600);
        let p = plan_move(&r);
        assert_eq!(p.frame, RootRect::new(400, 300, 400, 300));
    }

    #[test]
    fn resize_moving_edge_constrained_by_work_area() {
        let mut r = req(ResizeEdges::right(), (5000, 0), free());
        r.work_area = RootRect::new(0, 0, 800, 600);
        let p = plan_resize(&r);
        // Moving (right) edge pinned at 800; fixed left = 100.
        assert_eq!(p.frame.x, 100);
        assert_eq!(p.frame.w, 700);
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
        let r = GeometryRequest {
            reason: GeometryReason::ClientConfigure,
            ..req(ResizeEdges::none(), (0, 0), min_max((100, 100), (500, 400)))
        };
        let p = plan_client_configure(&r, RootPoint::new(300, 300), Some((2000, 2000)));
        // NW gravity: origin pinned to previous client origin (100,131).
        assert_eq!(p.client_size, (500, 400));
        assert_eq!(p.frame, RootRect::new(100, 100, 500, 431));
    }

    #[test]
    fn client_configure_position_only_moves_frame() {
        let r = GeometryRequest {
            reason: GeometryReason::ClientConfigure,
            ..req(ResizeEdges::none(), (0, 0), free())
        };
        let p = plan_client_configure(&r, RootPoint::new(300, 300), None);
        assert_eq!(p.frame, RootRect::new(300, 269, 400, 300));
        assert_eq!(p.client_size, (400, 269));
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
            let r = GeometryRequest {
                reason: GeometryReason::ClientConfigure,
                ..req(ResizeEdges::none(), (0, 0), hints)
            };
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
