mod bounds;
mod configure;

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

fn move_by(delta: (i32, i32)) -> RootRect {
    let mut r = req(ResizeEdges::none(), delta, free());
    r.reason = GeometryReason::InteractiveMove;
    r.outputs = OUTS;
    plan_move(&r).frame
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
