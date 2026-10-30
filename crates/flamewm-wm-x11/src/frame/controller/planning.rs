//! Pure planning helpers for frame-controller effect lists.
use super::*;

pub(super) fn request(
    state: &ControllerState,
    reason: GeometryReason,
    start: PlacementSnapshot,
    delta: (i32, i32),
    edges: ResizeEdges,
) -> GeometryRequest<'_> {
    GeometryRequest {
        reason,
        start,
        pointer_delta: delta,
        edges,
        work_area: state.work_area,
        outputs: state.outputs.as_slice(),
        hints: state.hints,
        frame_extents: state.extents,
    }
}

/// Client-root notify for `frame`; the client rect is clamped to >= 1 px.
pub(super) fn notify_root(frame: RootRect, extents: FrameExtents) -> FrameEffect {
    let root: ClientRootRect = frame_to_client_root(frame, extents);
    FrameEffect::NotifyClientRoot {
        x: root.x,
        y: root.y,
        w: root.w.max(1) as u32,
        h: root.h.max(1) as u32,
    }
}

/// Full plan split: frame root rect + client local size + client root notify.
pub(super) fn configure_effects(frame: RootRect, extents: FrameExtents) -> Vec<FrameEffect> {
    let local = frame_to_client_local(frame, extents);
    vec![
        FrameEffect::ConfigureFrameRoot {
            x: frame.x,
            y: frame.y,
            w: frame.w,
            h: frame.h,
        },
        FrameEffect::ConfigureClientLocal {
            x: local.x,
            y: local.y,
            w: local.w.max(1) as u32,
            h: local.h.max(1) as u32,
        },
        notify_root(frame, extents),
    ]
}

/// Commit native frame/client geometry BEFORE painting its new dimensions.
/// The old order painted at the old size, then resized the X window: the
/// server exposed dark/black new titlebar pixels until an unrelated Expose.
pub(super) fn full_configure(frame: RootRect, extents: FrameExtents) -> Vec<FrameEffect> {
    let mut effects = configure_effects(frame, extents);
    effects.extend([
        FrameEffect::LayoutInput,
        FrameEffect::PaintChrome,
        FrameEffect::ApplyShape,
    ]);
    effects
}

/// Move plan: position-only frame origin + root notify. No client
/// configure, no layout, no paint, no shape. Position-only by type:
/// `MoveFrameRoot` carries no w/h so a move cannot resize.
pub(super) fn move_effects(frame: RootRect, extents: FrameExtents) -> Vec<FrameEffect> {
    vec![
        FrameEffect::MoveFrameRoot {
            x: frame.x,
            y: frame.y,
        },
        notify_root(frame, extents),
    ]
}

/// Release tail: final geometry first, Ungrab last. A no-op release only ungrabs.
pub(super) fn release_effects(mut effects: Vec<FrameEffect>) -> Vec<FrameEffect> {
    if effects == [FrameEffect::Noop] {
        return vec![FrameEffect::Ungrab];
    }
    effects.push(FrameEffect::Ungrab);
    effects
}

/// Release tail with the XP outline hidden first when it was shown during the drag.
pub(super) fn release_tail(
    contents_while_dragging: bool,
    effects: Vec<FrameEffect>,
) -> Vec<FrameEffect> {
    let mut tail = release_effects(effects);
    if !contents_while_dragging {
        tail.insert(0, FrameEffect::DragOutline { rect: None });
    }
    tail
}

pub(super) fn margined_rect(tile: RootRect, work_area: RootRect, margin: i32) -> RootRect {
    let rect = flamewm_window_core::apply_window_margin(
        flamewm_api::Rect::new(tile.x, tile.y, tile.w, tile.h),
        flamewm_api::Rect::new(work_area.x, work_area.y, work_area.w, work_area.h),
        margin,
    );
    RootRect::new(rect.x, rect.y, rect.width, rect.height)
}

pub(super) fn snap_rect(work_area: RootRect, target: SnapTarget) -> RootRect {
    let (x, y, w, h) = (work_area.x, work_area.y, work_area.w, work_area.h);
    let hw = (w / 2).max(1);
    let hh = (h / 2).max(1);
    match target {
        SnapTarget::Left => RootRect::new(x, y, hw, h),
        SnapTarget::Right => RootRect::new(x.saturating_add(w.saturating_sub(hw)), y, hw, h),
        SnapTarget::Top => RootRect::new(x, y, w, hh),
        SnapTarget::Bottom => RootRect::new(x, y.saturating_add(h.saturating_sub(hh)), w, hh),
        SnapTarget::TopLeft => RootRect::new(x, y, hw, hh),
        SnapTarget::TopRight => RootRect::new(x.saturating_add(w.saturating_sub(hw)), y, hw, hh),
        SnapTarget::BottomLeft => RootRect::new(x, y.saturating_add(h.saturating_sub(hh)), hw, hh),
        SnapTarget::BottomRight => RootRect::new(
            x.saturating_add(w.saturating_sub(hw)),
            y.saturating_add(h.saturating_sub(hh)),
            hw,
            hh,
        ),
    }
}

pub(super) fn move_anchor(pointer: RootPoint, rect: RootRect, titlebar_h: i32) -> MoveAnchor {
    let den = rect.w.max(1) as u32;
    let num = pointer.x.saturating_sub(rect.x).clamp(0, rect.w.max(0)) as u32;
    let vert = pointer.y.saturating_sub(rect.y).clamp(0, titlebar_h);
    MoveAnchor::new(num, den, vert)
}

pub(super) const MOVE_ACTIVATION_THRESHOLD_PX: i32 = 4;

pub(super) fn move_delta(start: RootPoint, current: RootPoint) -> (i32, i32) {
    (
        current.x.saturating_sub(start.x),
        current.y.saturating_sub(start.y),
    )
}

pub(super) fn move_reached_activation(start: RootPoint, current: RootPoint) -> bool {
    let (dx, dy) = move_delta(start, current);
    dx >= MOVE_ACTIVATION_THRESHOLD_PX
        || dx <= -MOVE_ACTIVATION_THRESHOLD_PX
        || dy >= MOVE_ACTIVATION_THRESHOLD_PX
        || dy <= -MOVE_ACTIVATION_THRESHOLD_PX
}

pub(super) fn move_step(
    mut state: ControllerState,
    mut session: MoveSession,
    pointer: RootPoint,
    final_commit: bool,
) -> (ControllerState, Vec<FrameEffect>) {
    let first_activation = !session.anchor.is_activated();
    if first_activation && !move_reached_activation(session.start_pointer, pointer) {
        return (state, vec![FrameEffect::Noop]);
    }
    let preview = !state.contents_while_dragging && !final_commit;
    let start_mode = session.start_placement.mode;

    let delta = move_delta(session.start_pointer, pointer);
    let req = request(
        &state,
        GeometryReason::InteractiveMove,
        session.start_placement,
        delta,
        ResizeEdges::none(),
    );
    let plan = plan_move(&req);
    session.anchor.activate();
    state.session.begin_move(session);

    if first_activation && state.placement.mode != PlacementMode::Floating {
        state.placement.mode = PlacementMode::Floating;
        state.placement.resume = None;
        state.placement.current = plan.frame;
        state.placement.floating_restore = plan.frame;
        // Leaving snapped/maximized restores the floating size now, also under the outline: the
        // press already recorded the start mode as floating, so release would only move the frame
        // and keep the tiled size (and paint chrome for the restored width).
        let mut effects = full_configure(plan.frame, state.extents);
        if preview {
            effects.push(FrameEffect::DragOutline {
                rect: Some(plan.frame),
            });
        }
        return (state, effects);
    }

    if plan.noop && (!final_commit || state.placement.current == session.start_placement.rect) {
        return (state, vec![FrameEffect::Noop]);
    }
    let moved = plan.frame != state.placement.current;
    state.placement.current = plan.frame;
    if state.placement.mode == PlacementMode::Floating {
        state.placement.floating_restore = plan.frame;
    }
    if preview {
        return (
            state,
            vec![FrameEffect::DragOutline {
                rect: Some(plan.frame),
            }],
        );
    }
    if !final_commit && !moved {
        // Pointer moved but the output clamp kept the frame where it was: the native
        // frame, its ConfigureNotify and the repaint are all already current.
        return (state, vec![FrameEffect::Noop]);
    }
    if final_commit && !state.contents_while_dragging && start_mode != PlacementMode::Floating {
        // Preview left a snapped/maximized window untouched; apply its real size now.
        let effects = full_configure(plan.frame, state.extents);
        return (state, effects);
    }
    let mut effects = move_effects(plan.frame, state.extents);
    if moved || (final_commit && !state.contents_while_dragging) {
        // Titlebar pixels are frame-relative; repaint after the frame moves.
        effects.push(FrameEffect::PaintChrome);
    }
    (state, effects)
}

pub(super) fn resize_step(
    mut state: ControllerState,
    session: ResizeSession,
    pointer: RootPoint,
    final_effects: bool,
) -> (ControllerState, Vec<FrameEffect>) {
    if matches!(
        state.placement.mode,
        PlacementMode::Maximized | PlacementMode::Fullscreen
    ) {
        return (state, vec![FrameEffect::Noop]);
    }
    let delta = move_delta(session.start_pointer, pointer);
    let req = request(
        &state,
        GeometryReason::InteractiveResize,
        session.start_placement,
        delta,
        session.edges,
    );
    let plan = plan_resize(&req);
    let should_commit = !plan.noop || state.placement.current != session.start_placement.rect;
    if !should_commit {
        return (state, vec![FrameEffect::Noop]);
    }

    // A live motion that leaves the frame where it is (minimum/output clamp) owes no
    // native configure, repaint or shape pass. A snapped frame still exits its layout.
    let unchanged = !matches!(state.placement.mode, PlacementMode::Snapped(_))
        && state.placement.current == plan.frame;
    if matches!(state.placement.mode, PlacementMode::Snapped(_)) {
        state.placement.mode = PlacementMode::Floating;
        state.placement.resume = None;
    }
    state.placement.current = plan.frame;
    if state.placement.mode == PlacementMode::Floating {
        state.placement.floating_restore = plan.frame;
    }
    if !state.contents_while_dragging && !final_effects {
        return (
            state,
            vec![FrameEffect::DragOutline {
                rect: Some(plan.frame),
            }],
        );
    }
    if unchanged && !final_effects {
        return (state, vec![FrameEffect::Noop]);
    }
    let effects = if final_effects {
        full_configure(plan.frame, state.extents)
    } else {
        let mut effects = configure_effects(plan.frame, state.extents);
        // Interactive resize changes exposed titlebar pixels on every committed
        // size. Redraw after the resize (not only on ButtonRelease); otherwise
        // the newly uncovered frame background remains black during drag.
        effects.extend([FrameEffect::PaintChrome, FrameEffect::ApplyShape]);
        effects
    };
    (state, effects)
}
