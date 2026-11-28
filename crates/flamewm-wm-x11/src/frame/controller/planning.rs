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

/// Fill a caller-owned plan buffer with the full configure triad.
///
/// `out` is cleared first: capacity is retained across events so a caller can keep one
/// buffer for the whole motion path instead of allocating a fresh `Vec` per event.
/// Values and order match [`configure_effects`] exactly.
pub(super) fn configure_effects_into(
    out: &mut Vec<FrameEffect>,
    frame: RootRect,
    extents: FrameExtents,
) {
    out.clear();
    let local = frame_to_client_local(frame, extents);
    out.push(FrameEffect::ConfigureFrameRoot {
        x: frame.x,
        y: frame.y,
        w: frame.w,
        h: frame.h,
    });
    out.push(FrameEffect::ConfigureClientLocal {
        x: local.x,
        y: local.y,
        w: local.w.max(1) as u32,
        h: local.h.max(1) as u32,
    });
    out.push(notify_root(frame, extents));
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
    // Reserve once and fill through the reusable helper: the same six effects in the
    // same order, without the old 3 -> 6 reallocation.
    let mut effects = Vec::with_capacity(6);
    configure_effects_into(&mut effects, frame, extents);
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

/// Snapped rectangle for `target`, from the shared window-core snap geometry.
pub(super) fn snap_rect(work_area: RootRect, target: SnapTarget) -> RootRect {
    let work = flamewm_api::Rect::new(work_area.x, work_area.y, work_area.w, work_area.h);
    let rect = flamewm_window_core::snap_geometry(
        target.core(),
        work,
        flamewm_api::Size::new(work.width, work.height),
    );
    RootRect::new(rect.x, rect.y, rect.width, rect.height)
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

#[cfg(test)]
mod snap_vector_tests {
    use super::*;

    /// Odd-sized work area: halves are `floor(size / 2)`, right/bottom start at `size - half`.
    /// The same vectors pin `flamewm_window_core::snap_geometry`, which the Wayland model also uses.
    #[test]
    fn snap_rect_matches_shared_window_core_vectors() {
        let work = RootRect::new(3, 5, 1281, 801);
        let expect = |target| snap_rect(work, target);
        assert_eq!(expect(SnapTarget::Left), RootRect::new(3, 5, 640, 801));
        assert_eq!(expect(SnapTarget::Right), RootRect::new(644, 5, 640, 801));
        assert_eq!(expect(SnapTarget::Top), RootRect::new(3, 5, 1281, 400));
        assert_eq!(expect(SnapTarget::Bottom), RootRect::new(3, 406, 1281, 400));
        assert_eq!(expect(SnapTarget::TopLeft), RootRect::new(3, 5, 640, 400));
        assert_eq!(
            expect(SnapTarget::TopRight),
            RootRect::new(644, 5, 640, 400)
        );
        assert_eq!(
            expect(SnapTarget::BottomLeft),
            RootRect::new(3, 406, 640, 400)
        );
        assert_eq!(
            expect(SnapTarget::BottomRight),
            RootRect::new(644, 406, 640, 400)
        );
    }
}

#[cfg(test)]
mod reusable_buffer_tests {
    use super::*;

    /// Step 1 equivalence: a reused caller-owned buffer yields exactly the same
    /// effects, in the same order, as the original allocating helper.
    #[test]
    fn configure_effects_into_matches_allocating_original() {
        let extents = FrameExtents::new(31, 1);
        let cases = [
            RootRect::new(100, 100, 400, 300),
            RootRect::new(0, 0, 1, 1),
            RootRect::new(-7, 12, 0, -3),
            RootRect::new(1920, 1080, 1366, 768),
        ];
        let mut buffer = Vec::new();
        for frame in cases {
            let expected = configure_effects(frame, extents);
            configure_effects_into(&mut buffer, frame, extents);
            assert_eq!(buffer, expected);
            // A second fill overwrites; it never appends to the previous plan.
            configure_effects_into(&mut buffer, frame, extents);
            assert_eq!(buffer, expected);
        }
        // Capacity survives a refill: no per-event reallocation after warm-up.
        let capacity = buffer.capacity();
        configure_effects_into(&mut buffer, cases[0], extents);
        assert_eq!(buffer.capacity(), capacity);
    }

    /// `full_configure` keeps the six-effect order while using the reusable helper.
    #[test]
    fn full_configure_keeps_configure_triad_before_tails() {
        let extents = FrameExtents::new(31, 1);
        let frame = RootRect::new(100, 100, 400, 300);
        let mut triad = Vec::new();
        configure_effects_into(&mut triad, frame, extents);
        let mut expected = triad;
        expected.extend([
            FrameEffect::LayoutInput,
            FrameEffect::PaintChrome,
            FrameEffect::ApplyShape,
        ]);
        assert_eq!(full_configure(frame, extents), expected);
    }
}
