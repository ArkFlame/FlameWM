//! Frame-controller reducer: one event in, one changeset and effect list out.
use super::*;

/// Pure reducer: one event -> (next state, native effect plan).
/// Borrowing convenience over [`reduce_owned`]; clones the state once.
#[must_use]
pub fn reduce(state: &ControllerState, event: FrameEvent) -> (ControllerState, Vec<FrameEffect>) {
    reduce_owned(state.clone(), event)
}

/// Pure reducer on owned state: consumes the previous state and returns the
/// next one without cloning the registry or outputs.
#[must_use]
pub fn reduce_owned(
    mut state: ControllerState,
    event: FrameEvent,
) -> (ControllerState, Vec<FrameEffect>) {
    match event {
        FrameEvent::Press { source, pointer } => {
            let target = target_for_xid(&state.registry, state.client, source);
            if !matches!(target.region, FrameRegion::TitleDrag) {
                state.last_title_click = None;
            }
            match target.region {
                FrameRegion::TitleDrag => {
                    let (start_mode, start_rect) = match state.placement.mode {
                        PlacementMode::Fullscreen => return (state, vec![FrameEffect::Noop]),
                        PlacementMode::Maximized | PlacementMode::Snapped(_) => {
                            // Re-anchor the floating restore rect to the visible frame.
                            let visible_anchor = move_anchor(
                                pointer,
                                state.placement.current,
                                state.extents.titlebar_h,
                            );
                            (
                                PlacementMode::Floating,
                                maximized_restore_rect(
                                    state.placement.floating_restore,
                                    pointer,
                                    visible_anchor.frac_num,
                                    visible_anchor.frac_den,
                                    pointer.y.saturating_sub(state.placement.current.y),
                                    state.extents.titlebar_h,
                                ),
                            )
                        }
                        PlacementMode::Floating => {
                            (PlacementMode::Floating, state.placement.current)
                        }
                    };
                    let anchor = move_anchor(pointer, start_rect, state.extents.titlebar_h);
                    state.session.begin_move(MoveSession {
                        client: state.client,
                        start_pointer: pointer,
                        start_placement: PlacementSnapshot {
                            mode: start_mode,
                            rect: start_rect,
                        },
                        anchor,
                        grab_window: state.grab,
                    });
                    let effects = vec![FrameEffect::Grab {
                        window: state.grab,
                        cursor: CursorKind::Move,
                    }];
                    (state, effects)
                }
                FrameRegion::Resize(edges) => {
                    if matches!(
                        state.placement.mode,
                        PlacementMode::Maximized | PlacementMode::Fullscreen
                    ) {
                        return (state, vec![FrameEffect::Noop]);
                    }
                    state.session.begin_resize(ResizeSession {
                        client: state.client,
                        start_pointer: pointer,
                        start_placement: PlacementSnapshot {
                            mode: state.placement.mode,
                            rect: state.placement.current,
                        },
                        edges,
                        grab_window: state.grab,
                    });
                    let effects = vec![FrameEffect::Grab {
                        window: state.grab,
                        cursor: cursor_for_region(FrameRegion::Resize(edges)),
                    }];
                    (state, effects)
                }
                FrameRegion::Control(control) => {
                    state.session.begin_control(ControlPressSession::new(
                        state.client,
                        control,
                        state.grab,
                    ));
                    let effects = vec![
                        FrameEffect::Grab {
                            window: state.grab,
                            cursor: cursor_for_region(FrameRegion::Control(control)),
                        },
                        FrameEffect::PaintChrome,
                    ];
                    (state, effects)
                }
                FrameRegion::Client => (state, vec![FrameEffect::Noop]),
            }
        }
        FrameEvent::Motion { pointer } => match state.session {
            InteractionSession::Move(m) => move_step(state, m, pointer, false),
            InteractionSession::Resize(r) => resize_step(state, r, pointer, false),
            InteractionSession::ControlPress(_) | InteractionSession::Idle => {
                (state, vec![FrameEffect::Noop])
            }
        },
        FrameEvent::Release { pointer, time_ms } => match state.session {
            InteractionSession::Move(m) => {
                if !m.anchor.is_activated() && !move_reached_activation(m.start_pointer, pointer) {
                    return title_click_release(state, pointer, time_ms);
                }
                let contents = state.contents_while_dragging;
                let (mut next, effects) = move_step(state, m, pointer, true);
                next.session = InteractionSession::Idle;
                next.last_title_click = None;
                // Move is position-only: no layout/paint/shape, just MoveFrameRoot + notify.
                (next, release_tail(contents, effects))
            }
            InteractionSession::Resize(r) => {
                let contents = state.contents_while_dragging;
                let (mut next, effects) = resize_step(state, r, pointer, true);
                next.session = InteractionSession::Idle;
                (next, release_tail(contents, effects))
            }
            InteractionSession::ControlPress(c) => {
                state.session = InteractionSession::Idle;
                if !c.armed {
                    return (state, vec![FrameEffect::Ungrab, FrameEffect::PaintChrome]);
                }
                match c.control {
                    FrameControl::MaximizeRestore => {
                        let (mut out, effects) = toggle_max(state);
                        out.session = InteractionSession::Idle;
                        (
                            out,
                            std::iter::once(FrameEffect::Ungrab)
                                .chain(effects)
                                .collect(),
                        )
                    }
                    FrameControl::Minimize | FrameControl::Close => {
                        (state, vec![FrameEffect::Ungrab, FrameEffect::PaintChrome])
                    }
                }
            }
            InteractionSession::Idle => (state, vec![FrameEffect::Noop]),
        },
        FrameEvent::Enter => {
            if matches!(state.session, InteractionSession::ControlPress(_)) {
                state.session.control_enter();
                (state, vec![FrameEffect::PaintChrome])
            } else {
                (state, vec![FrameEffect::Noop])
            }
        }
        FrameEvent::Leave => {
            if matches!(state.session, InteractionSession::ControlPress(_)) {
                state.session.control_leave();
                (state, vec![FrameEffect::PaintChrome])
            } else {
                (state, vec![FrameEffect::Noop])
            }
        }
        #[cfg(test)]
        FrameEvent::ClientConfigureRequest { root_origin, size } => {
            let req = request(
                &state,
                GeometryReason::ClientConfigure,
                PlacementSnapshot {
                    mode: state.placement.mode,
                    rect: state.placement.current,
                },
                (0, 0),
                ResizeEdges::none(),
            );
            let plan = plan_client_configure(&req, root_origin, size);
            if plan.noop {
                return (state, vec![FrameEffect::Noop]);
            }
            state.placement.current = plan.frame;
            let mut effects = configure_effects(plan.frame, state.extents);
            effects.extend([
                FrameEffect::LayoutInput,
                FrameEffect::PaintChrome,
                FrameEffect::ApplyShape,
            ]);
            (state, effects)
        }
        FrameEvent::Snap { target } => {
            let rect = state.layout_rect(snap_rect(state.work_area, target));
            state.session = InteractionSession::Idle;
            if state.placement.mode == PlacementMode::Floating {
                state.placement.resume = Some(PlacementSnapshot {
                    mode: PlacementMode::Floating,
                    rect: state.placement.floating_restore,
                });
            } else {
                state.placement.resume = Some(PlacementSnapshot {
                    mode: state.placement.mode,
                    rect: state.placement.current,
                });
            }
            state.placement.mode = PlacementMode::Snapped(target);
            state.placement.current = rect;
            // Final geometry first, Ungrab last.
            let mut effects = configure_effects(rect, state.extents);
            effects.extend([
                FrameEffect::LayoutInput,
                FrameEffect::PaintChrome,
                FrameEffect::ApplyShape,
                FrameEffect::SnapPreview {
                    target: Some(target),
                },
            ]);
            effects.push(FrameEffect::Ungrab);
            (state, effects)
        }
        FrameEvent::ToggleMax => toggle_max(state),
        FrameEvent::ToggleFullscreen => toggle_fullscreen(state),
        FrameEvent::Cancel => {
            if state.session.is_idle() {
                (state, vec![FrameEffect::Noop])
            } else {
                let contents = state.contents_while_dragging;
                state.session.cancel();
                let mut effects = vec![
                    FrameEffect::Ungrab,
                    FrameEffect::SnapPreview { target: None },
                ];
                if !contents {
                    effects.insert(0, FrameEffect::DragOutline { rect: None });
                }
                (state, effects)
            }
        }
    }
}

/// Title release that never activated a move. Second such release from the
/// same client within the window (same slop) toggles maximize; otherwise it
/// is recorded as the pending first click.
pub(super) fn title_click_release(
    mut state: ControllerState,
    pointer: RootPoint,
    time_ms: u32,
) -> (ControllerState, Vec<FrameEffect>) {
    state.session = InteractionSession::Idle;
    let completes = state.last_title_click.is_some_and(|prev| {
        let elapsed = time_ms.wrapping_sub(prev.time_ms);
        let dx = i64::from(pointer.x) - i64::from(prev.pointer.x);
        let dy = i64::from(pointer.y) - i64::from(prev.pointer.y);
        prev.client == state.client
            && elapsed <= TITLE_DOUBLE_CLICK_TIMEOUT_MS
            && dx * dx + dy * dy <= TITLE_DOUBLE_CLICK_SLOP_PX * TITLE_DOUBLE_CLICK_SLOP_PX
    });
    if !completes {
        state.last_title_click = Some(TitleClick {
            client: state.client,
            time_ms,
            pointer,
        });
        return (state, vec![FrameEffect::Ungrab]);
    }
    state.last_title_click = None;
    toggle_max(state)
}

/// Enter `mode` at `rect`, or return to the resume snapshot when already in it.
pub(super) fn toggle_placement(
    mut state: ControllerState,
    mode: PlacementMode,
    rect: RootRect,
) -> (ControllerState, Vec<FrameEffect>) {
    state.session = InteractionSession::Idle;
    if state.placement.mode == mode {
        let snap = state.placement.resume.unwrap_or(PlacementSnapshot {
            mode: PlacementMode::Floating,
            rect: state.placement.floating_restore,
        });
        state.placement.mode = snap.mode;
        state.placement.current = snap.rect;
        state.placement.resume = None;
    } else {
        state.placement.resume = Some(PlacementSnapshot {
            mode: state.placement.mode,
            rect: state.placement.current,
        });
        if state.placement.mode == PlacementMode::Floating {
            state.placement.floating_restore = state.placement.current;
        }
        state.placement.mode = mode;
        state.placement.current = rect;
    }
    let mut effects = full_configure(state.placement.current, state.extents);
    effects.push(FrameEffect::Ungrab);
    (state, effects)
}

pub(super) fn toggle_max(state: ControllerState) -> (ControllerState, Vec<FrameEffect>) {
    let work_area = state.layout_rect(state.work_area);
    toggle_placement(state, PlacementMode::Maximized, work_area)
}

pub(super) fn toggle_fullscreen(state: ControllerState) -> (ControllerState, Vec<FrameEffect>) {
    let screen_rect = state.screen_rect;
    toggle_placement(state, PlacementMode::Fullscreen, screen_rect)
}
