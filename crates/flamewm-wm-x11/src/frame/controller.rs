//! Frame controller: pure INPUT -> SESSION -> GEOMETRY -> effect plan reducer.
//!
//! No X calls, no x11rb, no live-frame mutation. Composes the `input`
//! registry view (source-XID-is-truth), the `session` state machine, and the
//! `geometry` planners into one changeset per event plus a native effect
//! list applied by the live owner.

use std::collections::HashMap;

use flamewm_render_core::CursorKind;

use super::coords::{ClientRootRect, RootPoint, RootRect};
#[cfg(test)]
use super::geometry::plan_client_configure;
use super::geometry::{
    FrameExtents, GeometryReason, GeometryRequest, frame_to_client_local, frame_to_client_root,
    plan_move, plan_resize,
};
use super::input::target_for_xid;
use super::model::{
    FrameControl, FrameRegion, PlacementMode, PlacementSnapshot, PlacementState, ResizeEdges,
    SnapTarget,
};
use super::resources::cursor_for_region;
use super::session::{
    ControlPressSession, InteractionSession, MoveAnchor, MoveSession, ResizeSession,
    maximized_restore_rect,
};
use crate::size_hints::ClientSizeHints;

/// Native effect intents executed by the live owner. Descriptions only.
///
/// Domain split: `ConfigureClientLocal` carries client-local geometry
/// (origin (0, titlebar) + client size) for the reparented window;
/// `ConfigureFrameRoot` carries the frame outer rect in root coords
/// (resize/general paths only);
/// `MoveFrameRoot` carries a position-only frame origin in root coords
/// for move sessions (w/h unrepresentable by construction);
/// `NotifyClientRoot` carries the synthetic client-root rect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameEffect {
    Grab { window: u32, cursor: CursorKind },
    Ungrab,
    ConfigureFrameRoot { x: i32, y: i32, w: i32, h: i32 },
    MoveFrameRoot { x: i32, y: i32 },
    ConfigureClientLocal { x: i32, y: i32, w: u32, h: u32 },
    LayoutInput,
    PaintChrome,
    ApplyShape,
    NotifyClientRoot { x: i32, y: i32, w: u32, h: u32 },
    SnapPreview { target: Option<SnapTarget> },
    Noop,
}

/// Pure controller state: session + placement + registry view + geometry ctx.
#[derive(Debug, Clone)]
pub struct ControllerState {
    pub session: InteractionSession,
    pub placement: PlacementState,
    pub registry: HashMap<u32, FrameRegion>,
    pub client: u32,
    pub grab: u32,
    pub work_area: RootRect,
    /// Output geometries bounding drags; empty falls back to `work_area`.
    pub outputs: Vec<RootRect>,
    pub screen_rect: RootRect,
    pub hints: ClientSizeHints,
    pub extents: FrameExtents,
    /// Last title click (release without drag activation); a second one
    /// inside the double-click window toggles maximize.
    pub last_title_click: Option<TitleClick>,
}

/// Non-drag title release awaiting a possible second click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TitleClick {
    pub client: u32,
    pub time_ms: u32,
    pub pointer: RootPoint,
}

const TITLE_DOUBLE_CLICK_TIMEOUT_MS: u32 = 500;
const TITLE_DOUBLE_CLICK_SLOP_PX: i64 = 4;

impl ControllerState {
    #[must_use]
    pub fn new(
        client: u32,
        grab: u32,
        placement: PlacementState,
        work_area: RootRect,
        hints: ClientSizeHints,
        extents: FrameExtents,
    ) -> Self {
        Self {
            session: InteractionSession::Idle,
            placement,
            registry: HashMap::new(),
            client,
            grab,
            work_area,
            outputs: Vec::new(),
            screen_rect: work_area,
            hints,
            extents,
            last_title_click: None,
        }
    }

    #[must_use]
    pub fn with_screen_rect(mut self, screen_rect: RootRect) -> Self {
        self.screen_rect = screen_rect;
        self
    }

    #[must_use]
    pub fn with_outputs(mut self, outputs: Vec<RootRect>) -> Self {
        self.outputs = outputs;
        self
    }

    #[must_use]
    pub fn with_registry(mut self, registry: HashMap<u32, FrameRegion>) -> Self {
        self.registry = registry;
        self
    }
}

/// One input event per reducer step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameEvent {
    Press {
        source: u32,
        pointer: RootPoint,
    },
    Motion {
        pointer: RootPoint,
    },
    /// `time_ms` is the X server event time (wrapping) for double-click timing.
    Release {
        pointer: RootPoint,
        time_ms: u32,
    },
    Enter,
    Leave,
    /// Client intent only: a ConfigureRequest from the client window.
    /// `root_origin`/`size` are client-window geometry (client-root rect),
    /// never the frame outer rect and never a ConfigureNotify observation.
    /// The live owner must never construct this from notify events (M01).
    #[cfg(test)]
    ClientConfigureRequest {
        root_origin: RootPoint,
        size: Option<(u32, u32)>,
    },
    Snap {
        target: SnapTarget,
    },
    ToggleMax,
    ToggleFullscreen,
    Cancel,
}

fn request(
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
        outputs: &state.outputs,
        hints: state.hints,
        frame_extents: state.extents,
    }
}

/// Client-root notify for `frame`; the client rect is clamped to >= 1 px.
fn notify_root(frame: RootRect, extents: FrameExtents) -> FrameEffect {
    let root: ClientRootRect = frame_to_client_root(frame, extents);
    FrameEffect::NotifyClientRoot {
        x: root.x,
        y: root.y,
        w: root.w.max(1) as u32,
        h: root.h.max(1) as u32,
    }
}

/// Full plan split: frame root rect + client local size + client root notify.
fn configure_effects(frame: RootRect, extents: FrameExtents) -> Vec<FrameEffect> {
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
fn full_configure(frame: RootRect, extents: FrameExtents) -> Vec<FrameEffect> {
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
fn move_effects(frame: RootRect, extents: FrameExtents) -> Vec<FrameEffect> {
    vec![
        FrameEffect::MoveFrameRoot {
            x: frame.x,
            y: frame.y,
        },
        notify_root(frame, extents),
    ]
}

/// Release tail: final geometry first, Ungrab last. A no-op release only ungrabs.
fn release_effects(mut effects: Vec<FrameEffect>) -> Vec<FrameEffect> {
    if effects == [FrameEffect::Noop] {
        return vec![FrameEffect::Ungrab];
    }
    effects.push(FrameEffect::Ungrab);
    effects
}

fn snap_rect(work_area: RootRect, target: SnapTarget) -> RootRect {
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

fn move_anchor(pointer: RootPoint, rect: RootRect, titlebar_h: i32) -> MoveAnchor {
    let den = rect.w.max(1) as u32;
    let num = pointer.x.saturating_sub(rect.x).clamp(0, rect.w.max(0)) as u32;
    let vert = pointer.y.saturating_sub(rect.y).clamp(0, titlebar_h);
    MoveAnchor::new(num, den, vert)
}

const MOVE_ACTIVATION_THRESHOLD_PX: i32 = 4;

fn move_delta(start: RootPoint, current: RootPoint) -> (i32, i32) {
    (
        current.x.saturating_sub(start.x),
        current.y.saturating_sub(start.y),
    )
}

fn move_reached_activation(start: RootPoint, current: RootPoint) -> bool {
    let (dx, dy) = move_delta(start, current);
    dx >= MOVE_ACTIVATION_THRESHOLD_PX
        || dx <= -MOVE_ACTIVATION_THRESHOLD_PX
        || dy >= MOVE_ACTIVATION_THRESHOLD_PX
        || dy <= -MOVE_ACTIVATION_THRESHOLD_PX
}

fn move_step(
    mut state: ControllerState,
    mut session: MoveSession,
    pointer: RootPoint,
    final_commit: bool,
) -> (ControllerState, Vec<FrameEffect>) {
    let first_activation = !session.anchor.is_activated();
    if first_activation && !move_reached_activation(session.start_pointer, pointer) {
        return (state, vec![FrameEffect::Noop]);
    }

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
        let effects = full_configure(plan.frame, state.extents);
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
    let mut effects = move_effects(plan.frame, state.extents);
    if moved {
        // Titlebar pixels are frame-relative; repaint after the frame moves.
        effects.push(FrameEffect::PaintChrome);
    }
    (state, effects)
}

fn resize_step(
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

    if matches!(state.placement.mode, PlacementMode::Snapped(_)) {
        state.placement.mode = PlacementMode::Floating;
        state.placement.resume = None;
    }
    state.placement.current = plan.frame;
    if state.placement.mode == PlacementMode::Floating {
        state.placement.floating_restore = plan.frame;
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
                let (mut next, effects) = move_step(state, m, pointer, true);
                next.session = InteractionSession::Idle;
                next.last_title_click = None;
                // Move is position-only: no layout/paint/shape, just MoveFrameRoot + notify.
                (next, release_effects(effects))
            }
            InteractionSession::Resize(r) => {
                let (mut next, effects) = resize_step(state, r, pointer, true);
                next.session = InteractionSession::Idle;
                (next, release_effects(effects))
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
            let rect = snap_rect(state.work_area, target);
            state.session = InteractionSession::Idle;
            if state.placement.mode != PlacementMode::Floating {
                state.placement.resume = Some(PlacementSnapshot {
                    mode: state.placement.mode,
                    rect: state.placement.current,
                });
            } else {
                state.placement.resume = Some(PlacementSnapshot {
                    mode: PlacementMode::Floating,
                    rect: state.placement.floating_restore,
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
                state.session.cancel();
                (
                    state,
                    vec![
                        FrameEffect::Ungrab,
                        FrameEffect::SnapPreview { target: None },
                    ],
                )
            }
        }
    }
}

/// Title release that never activated a move. Second such release from the
/// same client within the window (same slop) toggles maximize; otherwise it
/// is recorded as the pending first click.
fn title_click_release(
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
fn toggle_placement(
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

fn toggle_max(state: ControllerState) -> (ControllerState, Vec<FrameEffect>) {
    let work_area = state.work_area;
    toggle_placement(state, PlacementMode::Maximized, work_area)
}

fn toggle_fullscreen(state: ControllerState) -> (ControllerState, Vec<FrameEffect>) {
    let screen_rect = state.screen_rect;
    toggle_placement(state, PlacementMode::Fullscreen, screen_rect)
}

#[cfg(test)]
mod tests {
    use super::super::geometry::FrameExtents;
    use super::*;
    use flamewm_api::{Rect as CoreRect, Size as CoreSize};
    use flamewm_window_core::{SnapTarget as CoreSnapTarget, snap_geometry};

    const WA: RootRect = RootRect::new(0, 0, 1920, 1080);
    const EXT: FrameExtents = FrameExtents::new(31, 1);
    const START: RootRect = RootRect::new(100, 100, 400, 300);

    const SCREEN: RootRect = RootRect::new(0, 0, 1920, 1080);

    fn press(source: u32, x: i32, y: i32) -> FrameEvent {
        FrameEvent::Press {
            source,
            pointer: RootPoint::new(x, y),
        }
    }

    fn motion(x: i32, y: i32) -> FrameEvent {
        FrameEvent::Motion {
            pointer: RootPoint::new(x, y),
        }
    }

    fn pressed(state: &ControllerState, source: u32, x: i32, y: i32) -> ControllerState {
        reduce(state, press(source, x, y)).0
    }

    fn release(x: i32, y: i32) -> FrameEvent {
        release_at(x, y, 0)
    }

    fn release_at(x: i32, y: i32, time_ms: u32) -> FrameEvent {
        FrameEvent::Release {
            pointer: RootPoint::new(x, y),
            time_ms,
        }
    }

    fn floating_state() -> ControllerState {
        let placement = PlacementState {
            mode: PlacementMode::Floating,
            current: START,
            floating_restore: START,
            resume: None,
        };
        let mut registry = HashMap::new();
        registry.insert(1, FrameRegion::TitleDrag);
        registry.insert(2, FrameRegion::Resize(ResizeEdges::bottom_right()));
        registry.insert(3, FrameRegion::Control(FrameControl::MaximizeRestore));
        ControllerState::new(7, 71, placement, WA, ClientSizeHints::default(), EXT)
            .with_screen_rect(SCREEN)
            .with_registry(registry)
    }

    /// Floating fixture moved into `mode` at `current`, resumable back to `START`.
    fn resumed_state(mode: PlacementMode, current: RootRect) -> ControllerState {
        let mut state = floating_state();
        state.placement.mode = mode;
        state.placement.current = current;
        state.placement.resume = Some(PlacementSnapshot {
            mode: PlacementMode::Floating,
            rect: START,
        });
        state
    }

    fn maximized_state() -> ControllerState {
        resumed_state(PlacementMode::Maximized, WA)
    }

    fn snapped_state() -> ControllerState {
        resumed_state(
            PlacementMode::Snapped(SnapTarget::Left),
            RootRect::new(0, 0, 960, 1080),
        )
    }

    fn has_full_configure(effects: &[FrameEffect]) -> bool {
        effects
            .iter()
            .any(|effect| matches!(effect, FrameEffect::ConfigureFrameRoot { .. }))
            && effects
                .iter()
                .any(|effect| matches!(effect, FrameEffect::ConfigureClientLocal { .. }))
            && effects
                .iter()
                .any(|effect| matches!(effect, FrameEffect::NotifyClientRoot { .. }))
    }

    fn assert_position_only(effects: &[FrameEffect]) {
        assert_eq!(effects.len(), 3);
        assert!(matches!(effects[0], FrameEffect::MoveFrameRoot { .. }));
        assert!(matches!(effects[1], FrameEffect::NotifyClientRoot { .. }));
        assert!(matches!(effects[2], FrameEffect::PaintChrome));
        assert!(!has_any_frame_root(effects));
        assert!(!effects.contains(&FrameEffect::LayoutInput));
        assert!(!effects.contains(&FrameEffect::ApplyShape));
    }

    fn has_frame(target: &[FrameEffect], x: i32, y: i32) -> bool {
        target.iter().any(|e| {
            matches!(
                e,
                FrameEffect::ConfigureFrameRoot { x: fx, y: fy, .. } if *fx == x && *fy == y
            )
        })
    }

    fn has_move_frame(target: &[FrameEffect], x: i32, y: i32) -> bool {
        target.iter().any(|e| {
            matches!(
                e,
                FrameEffect::MoveFrameRoot { x: fx, y: fy } if *fx == x && *fy == y
            )
        })
    }

    fn has_any_frame_root(target: &[FrameEffect]) -> bool {
        target
            .iter()
            .any(|e| matches!(e, FrameEffect::ConfigureFrameRoot { .. }))
    }

    fn count_local(target: &[FrameEffect]) -> usize {
        target
            .iter()
            .filter(|e| matches!(e, FrameEffect::ConfigureClientLocal { .. }))
            .count()
    }

    #[test]
    fn press_motion_release_move() {
        let s0 = floating_state();
        let (s1, e1) = reduce(&s0, press(1, 150, 120));
        assert!(matches!(s1.session, InteractionSession::Move(_)));
        assert!(e1.iter().any(|e| matches!(e, FrameEffect::Grab { .. })));
        let (s2, e2) = reduce(&s1, motion(200, 170));
        assert_eq!(s2.placement.current, RootRect::new(150, 150, 400, 300));
        assert!(has_move_frame(&e2, 150, 150));
        assert!(!has_any_frame_root(&e2));
        let (s3, e3) = reduce(&s2, release(200, 170));
        assert!(s3.session.is_idle());
        assert!(e3.contains(&FrameEffect::Ungrab));
        assert_eq!(s3.placement.current, RootRect::new(150, 150, 400, 300));
    }

    #[test]
    fn press_motion_release_resize() {
        let s0 = floating_state();
        let (s1, e1) = reduce(&s0, press(2, 500, 400));
        assert!(matches!(s1.session, InteractionSession::Resize(_)));
        assert!(e1.iter().any(|e| matches!(e, FrameEffect::Grab { .. })));
        let (s2, _) = reduce(&s1, motion(550, 460));
        assert_eq!(s2.placement.current, RootRect::new(100, 100, 450, 360));
        let (s3, e3) = reduce(&s2, release(550, 460));
        assert!(s3.session.is_idle());
        assert!(e3.contains(&FrameEffect::Ungrab));
        assert!(e3.contains(&FrameEffect::ApplyShape));
        assert_eq!(s3.placement.current, RootRect::new(100, 100, 450, 360));
    }

    #[test]
    fn control_click_toggles_max() {
        let s1 = pressed(&floating_state(), 3, 300, 110);
        assert!(matches!(s1.session, InteractionSession::ControlPress(_)));
        let (s2, e2) = reduce(&s1, release(300, 110));
        assert_eq!(s2.placement.mode, PlacementMode::Maximized);
        assert_eq!(s2.placement.current, WA);
        assert!(e2.contains(&FrameEffect::Ungrab));
        assert!(has_frame(&e2, 0, 0));
    }

    fn title_click(
        state: &ControllerState,
        x: i32,
        y: i32,
        time_ms: u32,
    ) -> (ControllerState, Vec<FrameEffect>) {
        let (pressed, _) = reduce(state, press(1, x, y));
        reduce(&pressed, release_at(x, y, time_ms))
    }

    #[test]
    fn title_double_click_within_window_toggles_max() {
        let s0 = floating_state();
        let (s1, e1) = title_click(&s0, 150, 120, 1000);
        assert_eq!(s1.placement.mode, PlacementMode::Floating);
        assert!(s1.session.is_idle());
        assert_eq!(e1, vec![FrameEffect::Ungrab]);
        assert!(s1.last_title_click.is_some());
        let (s2, e2) = title_click(&s1, 150, 120, 1300);
        assert_eq!(s2.placement.mode, PlacementMode::Maximized);
        assert_eq!(s2.placement.current, WA);
        assert!(s2.session.is_idle());
        assert!(s2.last_title_click.is_none());
        assert!(has_frame(&e2, 0, 0));
    }

    #[test]
    fn title_double_click_on_maximized_restores() {
        let (s1, _) = title_click(&maximized_state(), 960, 20, 1000);
        assert_eq!(s1.placement.mode, PlacementMode::Maximized);
        let (s2, _) = title_click(&s1, 960, 20, 1100);
        assert_eq!(s2.placement.mode, PlacementMode::Floating);
        assert_eq!(s2.placement.current, START);
    }

    #[test]
    fn title_second_click_after_timeout_does_not_toggle() {
        let s0 = floating_state();
        let (s1, _) = title_click(&s0, 150, 120, 1000);
        let (slow, _) = title_click(&s1, 150, 120, 1501);
        assert_eq!(slow.placement.mode, PlacementMode::Floating);
        assert!(slow.last_title_click.is_some());
        let (edge, _) = title_click(&s1, 150, 120, 1500);
        assert_eq!(edge.placement.mode, PlacementMode::Maximized);
    }

    #[test]
    fn title_second_click_beyond_slop_does_not_toggle() {
        let s0 = floating_state();
        let (s1, _) = title_click(&s0, 150, 120, 1000);
        let (far, _) = title_click(&s1, 155, 120, 1100);
        assert_eq!(far.placement.mode, PlacementMode::Floating);
        let (diagonal, _) = title_click(&s1, 153, 123, 1100);
        assert_eq!(diagonal.placement.mode, PlacementMode::Floating);
        let (edge, _) = title_click(&s1, 154, 120, 1100);
        assert_eq!(edge.placement.mode, PlacementMode::Maximized);
    }

    #[test]
    fn title_click_from_other_client_does_not_toggle() {
        let (mut s1, _) = title_click(&floating_state(), 150, 120, 1000);
        if let Some(record) = s1.last_title_click.as_mut() {
            record.client = 8;
        }
        let (s2, _) = title_click(&s1, 150, 120, 1100);
        assert_eq!(s2.placement.mode, PlacementMode::Floating);
        assert!(s2.last_title_click.is_some());
    }

    #[test]
    fn title_drag_between_clicks_breaks_pair() {
        let s0 = floating_state();
        let (s1, _) = title_click(&s0, 150, 120, 1000);
        let (s2, _) = reduce(&s1, press(1, 150, 120));
        let (s3, _) = reduce(&s2, motion(200, 170));
        let (s4, _) = reduce(&s3, release_at(200, 170, 1200));
        assert_eq!(s4.placement.mode, PlacementMode::Floating);
        assert!(s4.last_title_click.is_none());
        let (s5, _) = title_click(&s4, 200, 170, 1300);
        assert_eq!(s5.placement.mode, PlacementMode::Floating);
    }

    #[test]
    fn non_title_press_clears_pending_title_click() {
        let s0 = floating_state();
        let (s1, _) = title_click(&s0, 150, 120, 1000);
        let (s2, _) = reduce(&s1, press(2, 500, 400));
        let (s3, _) = reduce(&s2, release_at(500, 400, 1100));
        assert!(s3.last_title_click.is_none());
        let (s4, _) = title_click(&s3, 150, 120, 1200);
        assert_eq!(s4.placement.mode, PlacementMode::Floating);
    }

    #[test]
    fn cancel_aborts_move() {
        let s1 = pressed(&floating_state(), 1, 150, 120);
        let (s2, e2) = reduce(&s1, FrameEvent::Cancel);
        assert!(s2.session.is_idle());
        assert_eq!(s2.placement.current, START);
        assert!(e2.contains(&FrameEffect::Ungrab));
        let (_, e3) = reduce(&s2, FrameEvent::Cancel);
        assert_eq!(e3, vec![FrameEffect::Noop]);
    }

    #[test]
    fn snap_finalizes_left_half() {
        let s0 = floating_state();
        let (s1, e1) = reduce(
            &s0,
            FrameEvent::Snap {
                target: SnapTarget::Left,
            },
        );
        assert_eq!(s1.placement.mode, PlacementMode::Snapped(SnapTarget::Left));
        assert_eq!(s1.placement.current, RootRect::new(0, 0, 960, 1080));
        assert!(e1.contains(&FrameEffect::SnapPreview {
            target: Some(SnapTarget::Left)
        }));
        assert!(has_frame(&e1, 0, 0));
    }

    #[test]
    fn max_restore_returns_to_floating() {
        let s0 = floating_state();
        let (s1, _) = reduce(&s0, FrameEvent::ToggleMax);
        assert_eq!(s1.placement.mode, PlacementMode::Maximized);
        let (s2, e2) = reduce(&s1, FrameEvent::ToggleMax);
        assert_eq!(s2.placement.mode, PlacementMode::Floating);
        assert_eq!(s2.placement.current, START);
        assert!(has_frame(&e2, 100, 100));
    }

    #[test]
    fn noop_geometry_yields_noop() {
        let s0 = floating_state();
        // Motion with zero delta while moving: plan equals start -> Noop.
        let (s1, _) = reduce(&s0, press(1, 150, 120));
        let (s2, e2) = reduce(&s1, motion(150, 120));
        assert_eq!(e2, vec![FrameEffect::Noop]);
        assert_eq!(s2.placement.current, START);
        // Bare configure with no movement on idle session -> Noop path.
        let (_, e3) = reduce(&s0, motion(0, 0));
        assert_eq!(e3, vec![FrameEffect::Noop]);
    }

    #[test]
    fn effect_order_manage_chrome_paint_retarget_first_flush_last() {
        use super::super::chrome::{effects_for, plan_scene};
        use super::super::resources::{CONTROL_ORDER, FrameResources, RESIZE_ORDER};
        let res = FrameResources::new(7, 70, 71, [72, 73, 74], [80, 81, 82, 83, 84, 85, 86, 87]);
        let children = res.children();
        assert_eq!(children.len(), 12);
        assert_eq!(children[8], 71);
        assert_eq!(&children[9..12], &[72, 73, 74]);
        assert_eq!(RESIZE_ORDER.len(), 8);
        assert_eq!(CONTROL_ORDER.len(), 3);
        let scene = plan_scene(
            400,
            300,
            "app",
            true,
            None,
            None,
            false,
            false,
            true,
            &flamewm_skin::recipes::decoration::ChromeTokens::default(),
        );
        let fx = effects_for(70, &scene);
        let kind = |e: &super::super::chrome::ChromeEffect| match e {
            super::super::chrome::ChromeEffect::Retarget { .. } => 0,
            super::super::chrome::ChromeEffect::FillTitlebar { .. } => 1,
            super::super::chrome::ChromeEffect::DrawTitle { .. } => 2,
            super::super::chrome::ChromeEffect::BlitIcon => 3,
            super::super::chrome::ChromeEffect::BlitControls { .. } => 4,
            super::super::chrome::ChromeEffect::ApplyShape { .. } => 5,
            super::super::chrome::ChromeEffect::Flush => 6,
        };
        let order: Vec<i32> = fx.iter().map(kind).collect();
        assert_eq!(*order.first().expect("nonempty"), 0);
        assert_eq!(*order.last().expect("nonempty"), 6);
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(
            order, sorted,
            "chrome effects must be monotonically ordered"
        );
    }

    #[test]
    fn effect_order_move_motion_has_no_layout_or_paint() {
        let s1 = pressed(&floating_state(), 1, 150, 120);
        let (_, e2) = reduce(&s1, motion(200, 170));
        assert_eq!(e2.len(), 3);
        assert!(matches!(
            e2[0],
            FrameEffect::MoveFrameRoot { x: 150, y: 150 }
        ));
        assert!(!has_any_frame_root(&e2));
        // Client local origin is (0,titlebar); synthetic root notify at frame offset.
        assert!(matches!(
            e2[1],
            FrameEffect::NotifyClientRoot {
                x: 150,
                y: 181,
                w: 400,
                h: 269
            }
        ));
        assert_eq!(count_local(&e2), 0);
        assert!(!e2.contains(&FrameEffect::LayoutInput));
        assert!(matches!(e2[2], FrameEffect::PaintChrome));
        assert!(!e2.contains(&FrameEffect::ApplyShape));
        assert!(!e2.contains(&FrameEffect::Ungrab));
    }

    #[test]
    fn effect_order_resize_release_has_frame_client_layout_chrome() {
        let s1 = pressed(&floating_state(), 2, 500, 400);
        let (s2, e_motion) = reduce(&s1, motion(550, 460));
        // Resize motion carries frame + local + root notify.
        assert_eq!(count_local(&e_motion), 1);
        assert!(
            e_motion
                .iter()
                .any(|e| matches!(e, FrameEffect::ConfigureClientLocal { x: 0, y: 31, .. }))
        );
        let (_, e3) = reduce(&s2, release(550, 460));
        assert!(e3.contains(&FrameEffect::LayoutInput));
        assert!(e3.contains(&FrameEffect::PaintChrome));
        assert!(e3.contains(&FrameEffect::ApplyShape));
        assert!(
            e3.iter()
                .any(|e| matches!(e, FrameEffect::ConfigureFrameRoot { .. }))
        );
        assert!(
            e3.iter()
                .any(|e| matches!(e, FrameEffect::ConfigureClientLocal { .. }))
        );
        let ungrab = e3
            .iter()
            .position(|e| *e == FrameEffect::Ungrab)
            .expect("ungrab");
        let layout = e3
            .iter()
            .position(|e| *e == FrameEffect::LayoutInput)
            .expect("layout");
        let paint = e3
            .iter()
            .position(|e| *e == FrameEffect::PaintChrome)
            .expect("paint");
        let frame = e3
            .iter()
            .position(|e| matches!(e, FrameEffect::ConfigureFrameRoot { .. }))
            .expect("frame");
        assert_eq!(ungrab, e3.len() - 1, "geometry precedes Ungrab on release");
        assert!(frame < layout && layout < paint && paint < ungrab);
    }

    #[test]
    fn effect_order_release_geometry_precedes_ungrab() {
        let s1 = pressed(&floating_state(), 1, 150, 120);
        let (s2, _) = reduce(&s1, motion(200, 170));
        let (s3, e3) = reduce(&s2, release(200, 170));
        assert_eq!(s3.placement.current, RootRect::new(150, 150, 400, 300));
        assert_eq!(*e3.last().expect("nonempty"), FrameEffect::Ungrab);
        let tail_move = e3
            .iter()
            .find_map(|e| match *e {
                FrameEffect::MoveFrameRoot { x, y } => Some((x, y)),
                _ => None,
            })
            .expect("final move geometry");
        assert_eq!(tail_move, (150, 150));
        assert!(!has_any_frame_root(&e3));
        let frame_pos = e3
            .iter()
            .position(|e| matches!(e, FrameEffect::MoveFrameRoot { .. }))
            .expect("frame");
        let ungrab_pos = e3
            .iter()
            .position(|e| *e == FrameEffect::Ungrab)
            .expect("ungrab");
        assert!(frame_pos < ungrab_pos);
    }

    #[test]
    fn move_thousand_samples_zero_client_local() {
        let s1 = pressed(&floating_state(), 1, 150, 120);
        let mut total_local = 0;
        let mut total_configure_root = 0;
        let mut total_layout = 0;
        let mut total_paint = 0;
        let mut total_shape = 0;
        let mut total_move_root = 0;
        let mut total_notify = 0;
        let mut total_noop = 0;
        for i in 0..1000 {
            let dx = (i % 100) - 50;
            let dy = ((i * 7) % 100) - 50;
            let (_, e) = reduce(&s1, motion(150 + dx, 120 + dy));
            // Move motion emits either Noop (zero/non-plan delta) or exactly
            // [MoveFrameRoot, NotifyClientRoot, PaintChrome]; nothing else is representable.
            if e == vec![FrameEffect::Noop] {
                total_noop += 1;
                continue;
            }
            assert_eq!(
                e.len(),
                3,
                "move motion must emit move+notify+paint (sample {i})"
            );
            assert!(
                matches!(e[2], FrameEffect::PaintChrome),
                "move[2] must be PaintChrome (sample {i})"
            );
            assert!(
                matches!(e[0], FrameEffect::MoveFrameRoot { .. }),
                "move[0] must be MoveFrameRoot (sample {i})"
            );
            assert!(
                matches!(e[1], FrameEffect::NotifyClientRoot { .. }),
                "move[1] must be NotifyClientRoot (sample {i})"
            );
            for fx in &e {
                match fx {
                    FrameEffect::ConfigureClientLocal { .. } => total_local += 1,
                    FrameEffect::ConfigureFrameRoot { .. } => total_configure_root += 1,
                    FrameEffect::LayoutInput => total_layout += 1,
                    FrameEffect::PaintChrome => total_paint += 1,
                    FrameEffect::ApplyShape => total_shape += 1,
                    FrameEffect::MoveFrameRoot { .. } => total_move_root += 1,
                    FrameEffect::NotifyClientRoot { .. } => total_notify += 1,
                    _ => panic!("unexpected move effect {fx:?} (sample {i})"),
                }
            }
        }
        assert_eq!(total_local, 0);
        assert_eq!(total_configure_root, 0);
        assert_eq!(total_layout, 0);
        assert_eq!(total_paint, total_move_root);
        assert_eq!(total_shape, 0);
        assert_eq!(total_move_root, total_notify);
        assert!(total_move_root > 0, "expected non-noop move samples");
        assert!(total_noop > 0, "expected at least one noop sample");
    }

    #[test]
    fn post_activation_move_repaints_chrome_after_frame_move() {
        let s1 = pressed(&floating_state(), 1, 150, 120);
        let (s2, _) = reduce(&s1, motion(200, 170));
        // Post-activation step whose frame position changes: titlebar must
        // repaint live, ordered after the frame move.
        let (s3, e3) = reduce(&s2, motion(230, 190));
        assert_eq!(s3.placement.current, RootRect::new(180, 170, 400, 300));
        let move_pos = e3
            .iter()
            .position(|e| matches!(e, FrameEffect::MoveFrameRoot { .. }))
            .expect("move geometry");
        let paint_pos = e3
            .iter()
            .position(|e| *e == FrameEffect::PaintChrome)
            .expect("live move must repaint titlebar");
        assert!(
            move_pos < paint_pos,
            "PaintChrome must follow MoveFrameRoot"
        );
        // Repeated pointer position leaves the frame unchanged: no repaint.
        let (_, e4) = reduce(&s3, motion(230, 190));
        assert!(!e4.contains(&FrameEffect::PaintChrome));
    }

    #[test]
    fn resize_motion_paints_after_native_resize_not_at_the_old_size() {
        let s1 = pressed(&floating_state(), 2, 500, 400);
        let (_, effects) = reduce(&s1, motion(550, 460));
        let at = |predicate: fn(&FrameEffect) -> bool| {
            effects.iter().position(predicate).expect("effect")
        };
        let frame = at(|e| matches!(e, FrameEffect::ConfigureFrameRoot { .. }));
        let client = at(|e| matches!(e, FrameEffect::ConfigureClientLocal { .. }));
        let paint = at(|e| matches!(e, FrameEffect::PaintChrome));
        let shape = at(|e| matches!(e, FrameEffect::ApplyShape));
        assert!(frame < client && client < paint && paint < shape);
    }

    #[test]
    fn resize_still_uses_full_configure_frame_root() {
        let s1 = pressed(&floating_state(), 2, 500, 400);
        let (_, e) = reduce(&s1, motion(550, 460));
        assert!(
            e.iter()
                .any(|fx| matches!(fx, FrameEffect::ConfigureFrameRoot { .. })),
            "resize motion must keep full ConfigureFrameRoot"
        );
        assert_eq!(count_local(&e), 1);
    }

    #[test]
    fn move_frame_at_origin_client_local_and_root() {
        // Start (100,100) + delta (300,100) => frame (400,200); client local
        // stays (0,titlebar), synthetic root = (400,231) size (400,269).
        let s1 = pressed(&floating_state(), 1, 150, 120);
        let (s2, e2) = reduce(&s1, motion(450, 220));
        assert_eq!(s2.placement.current, RootRect::new(400, 200, 400, 300));
        assert!(matches!(
            e2[1],
            FrameEffect::NotifyClientRoot {
                x: 400,
                y: 231,
                w: 400,
                h: 269
            }
        ));
    }

    #[test]
    fn resize_right_and_bottom_right_local_origin_constant() {
        let s1 = pressed(&floating_state(), 2, 500, 400);
        let (_, e) = reduce(&s1, motion(550, 460));
        assert!(
            e.iter()
                .any(|fx| matches!(fx, FrameEffect::ConfigureClientLocal { x: 0, y: 31, .. }))
        );
    }

    #[test]
    fn fullscreen_uses_screen_rect_and_restores_exact_snapshot() {
        let mut s0 = floating_state();
        s0.work_area = RootRect::new(0, 24, 1920, 1056);
        s0.screen_rect = RootRect::new(0, 0, 1920, 1080);
        let (s1, e1) = reduce(&s0, FrameEvent::ToggleFullscreen);
        assert_eq!(s1.placement.mode, PlacementMode::Fullscreen);
        assert_eq!(s1.placement.current, RootRect::new(0, 0, 1920, 1080));
        assert!(has_frame(&e1, 0, 0));
        let (s2, _) = reduce(&s0, FrameEvent::ToggleMax);
        assert_eq!(s2.placement.current, RootRect::new(0, 24, 1920, 1056));
        let (s3, _) = reduce(&s1, FrameEvent::ToggleFullscreen);
        assert_eq!(s3.placement.mode, PlacementMode::Floating);
        assert_eq!(s3.placement.current, START);
    }

    #[test]
    fn eight_edges_hold_opposite_anchor() {
        use super::super::geometry::{GeometryReason, GeometryRequest, plan_resize};
        use super::super::model::PlacementSnapshot;
        let edges = [
            ResizeEdges::left(),
            ResizeEdges::right(),
            ResizeEdges::top(),
            ResizeEdges::bottom(),
            ResizeEdges::top_left(),
            ResizeEdges::top_right(),
            ResizeEdges::bottom_left(),
            ResizeEdges::bottom_right(),
        ];
        for e in edges {
            let r = GeometryRequest {
                reason: GeometryReason::InteractiveResize,
                start: PlacementSnapshot {
                    mode: PlacementMode::Floating,
                    rect: START,
                },
                pointer_delta: (40, 30),
                edges: e,
                work_area: WA,
                outputs: &[],
                hints: ClientSizeHints::default(),
                frame_extents: EXT,
            };
            let p = plan_resize(&r);
            if !e.left {
                assert_eq!(p.frame.x, START.x, "left anchor {e:?}");
            }
            if !e.right && !e.left {
                assert_eq!(p.frame.w, START.w, "width {e:?}");
            }
            if !e.top {
                assert_eq!(p.frame.y, START.y, "top anchor {e:?}");
            }
        }
    }

    #[test]
    fn client_configure_request_frame_size_as_intent_grows_frame() {
        // M01 intent: ClientConfigureRequest carries client-window geometry
        // only. A frame-sized observation ((400,300) = outer incl. titlebar)
        // fed as client intent grows the frame by the titlebar: (100,100,
        // 400,300) -> (100,100,400,331). ConfigureNotify observations must
        // therefore never be constructed into this event by the live owner.
        let s0 = floating_state();
        let (s1, _) = reduce(
            &s0,
            FrameEvent::ClientConfigureRequest {
                root_origin: RootPoint::new(100, 131),
                size: Some((400, 300)),
            },
        );
        assert_eq!(s1.placement.current, RootRect::new(100, 100, 400, 331));
    }

    #[test]
    fn border_does_not_change_local_geometry() {
        use super::super::geometry::frame_to_client_local;
        let thin = FrameExtents::new(31, 0);
        let thick = FrameExtents::new(31, 1);
        assert_eq!(
            frame_to_client_local(START, thin),
            frame_to_client_local(START, thick)
        );
    }

    // CONTRACT-REGRESSION T01: threshold keeps a press below activation inert.
    // TRIGGER: title press followed by a three-pixel motion.
    // OBSERVABLE RESULT: placement and motion effects remain unchanged.
    // UNCOVERED GAP: current reducer activates every title press immediately.
    // FAILURE MUTATION: remove the four-pixel activation threshold.
    // REPRESENTATIVE CASE: ordinary floating title drag.
    #[test]
    fn t01_title_motion_below_four_pixels_is_pending() {
        let s0 = floating_state();
        let (s1, _) = reduce(&s0, press(1, 150, 120));
        let (s2, _effects) = reduce(&s1, motion(153, 120));
        assert!(matches!(s1.session, InteractionSession::Move(_)));
        assert_eq!(s1.placement, s0.placement);
        assert_eq!(s2.placement.mode, PlacementMode::Floating);
        assert_eq!(s2.placement.current, s0.placement.current);
        assert_eq!(
            (s2.placement.current.w, s2.placement.current.h),
            (s0.placement.current.w, s0.placement.current.h)
        );
        assert_eq!(s2.placement.floating_restore, s0.placement.floating_restore);
    }

    // CONTRACT-REGRESSION T02: activation occurs at the four-pixel boundary.
    // TRIGGER: title press followed by exactly four pixels of motion.
    // OBSERVABLE RESULT: max drag activates restore on the exact boundary.
    // UNCOVERED GAP: threshold and press snapshot are not represented together.
    // FAILURE MUTATION: change the boundary to three pixels or use incremental state.
    // REPRESENTATIVE CASE: exact threshold maximized drag-away.
    #[test]
    fn t02_title_motion_at_four_pixels_activates_from_snapshot() {
        let s0 = maximized_state();
        let (s1, _) = reduce(&s0, press(1, 960, 20));
        let (s2, effects) = reduce(&s1, motion(964, 20));
        assert_eq!(s2.placement.mode, PlacementMode::Floating);
        assert_eq!(
            (s2.placement.current.w, s2.placement.current.h),
            (
                s0.placement.floating_restore.w,
                s0.placement.floating_restore.h
            )
        );
        assert!(has_full_configure(&effects));
    }

    // CONTRACT-REGRESSION T03: maximized press keeps authoritative placement.
    // TRIGGER: title press while maximized.
    // OBSERVABLE RESULT: current remains work-area/maximized; pending snapshot is Floating.
    // UNCOVERED GAP: current press labels the restore snapshot Maximized.
    // FAILURE MUTATION: store the current mode instead of Floating in MoveSession.
    // REPRESENTATIVE CASE: maximize then drag.
    #[test]
    fn t03_maximized_press_has_floating_pending_snapshot() {
        let s0 = maximized_state();
        let (s1, effects) = reduce(&s0, press(1, 960, 20));
        assert_eq!(s1.placement, s0.placement);
        assert!(effects.contains(&FrameEffect::Grab {
            window: 71,
            cursor: flamewm_render_core::CursorKind::Move,
        }));
        match s1.session {
            InteractionSession::Move(session) => {
                assert_eq!(session.start_placement.mode, PlacementMode::Floating);
                assert_eq!(
                    (
                        session.start_placement.rect.w,
                        session.start_placement.rect.h
                    ),
                    (
                        s0.placement.floating_restore.w,
                        s0.placement.floating_restore.h
                    )
                );
                assert_ne!(session.start_placement.rect, s0.placement.current);
            }
            other => panic!("expected pending move session, got {other:?}"),
        }
    }

    // CONTRACT-REGRESSION T04: snapped press also records a Floating restore.
    // TRIGGER: title press while snapped left.
    // OBSERVABLE RESULT: snapped current rect stays authoritative until activation.
    // UNCOVERED GAP: current snapped branch snapshots the snapped mode/rect.
    // FAILURE MUTATION: treat snapped press as an ordinary move immediately.
    // REPRESENTATIVE CASE: snapped title drag-away.
    #[test]
    fn t04_snapped_press_has_floating_pending_snapshot() {
        let s0 = snapped_state();
        let (s1, _) = reduce(&s0, press(1, 500, 20));
        assert_eq!(s1.placement, s0.placement);
        match s1.session {
            InteractionSession::Move(session) => {
                assert_eq!(session.start_placement.mode, PlacementMode::Floating);
                assert_eq!(
                    (
                        session.start_placement.rect.w,
                        session.start_placement.rect.h
                    ),
                    (
                        s0.placement.floating_restore.w,
                        s0.placement.floating_restore.h
                    )
                );
                assert_ne!(session.start_placement.rect, s0.placement.current);
            }
            other => panic!("expected pending move session, got {other:?}"),
        }
    }

    // CONTRACT-REGRESSION T05: first maximized drag-away is a full restore.
    // TRIGGER: maximized title press, then motion beyond four pixels.
    // OBSERVABLE RESULT: Floating/restore size plus full configure effects.
    // UNCOVERED GAP: current motion keeps Maximized and emits position-only effects.
    // FAILURE MUTATION: replace restore configure with MoveFrameRoot only.
    // REPRESENTATIVE CASE: maximized window first activated motion.
    #[test]
    fn t05_maximized_first_drag_away_restores_with_full_configure() {
        let s0 = maximized_state();
        let (s1, _) = reduce(&s0, press(1, 960, 20));
        let (s2, effects) = reduce(&s1, motion(965, 25));
        assert_eq!(s2.placement.mode, PlacementMode::Floating);
        assert_eq!(
            (s2.placement.current.w, s2.placement.current.h),
            (
                s0.placement.floating_restore.w,
                s0.placement.floating_restore.h
            )
        );
        assert!(has_full_configure(&effects));
        assert!(effects.contains(&FrameEffect::LayoutInput));
        assert!(effects.contains(&FrameEffect::PaintChrome));
        assert!(effects.contains(&FrameEffect::ApplyShape));
        assert!(!effects.contains(&FrameEffect::Ungrab));
    }

    // CONTRACT-REGRESSION T06: snapped drag-away uses the same full restore path.
    // TRIGGER: snapped title press, then motion beyond four pixels.
    // OBSERVABLE RESULT: Floating restore dimensions and full configure effects.
    // UNCOVERED GAP: current snapped motion resizes/moves while retaining Snapped.
    // FAILURE MUTATION: omit the one-time snapped-to-floating transition.
    // REPRESENTATIVE CASE: left-snapped window first activated motion.
    #[test]
    fn t06_snapped_first_drag_away_restores_with_full_configure() {
        let s0 = snapped_state();
        let (s1, _) = reduce(&s0, press(1, 500, 20));
        let (s2, effects) = reduce(&s1, motion(505, 25));
        assert_eq!(s2.placement.mode, PlacementMode::Floating);
        assert_eq!(
            (s2.placement.current.w, s2.placement.current.h),
            (
                s0.placement.floating_restore.w,
                s0.placement.floating_restore.h
            )
        );
        assert!(has_full_configure(&effects));
        assert!(effects.contains(&FrameEffect::LayoutInput));
        assert!(effects.contains(&FrameEffect::PaintChrome));
        assert!(effects.contains(&FrameEffect::ApplyShape));
    }

    // CONTRACT-REGRESSION T07: only the first activated motion restores size.
    // TRIGGER: max drag-away followed by a second motion.
    // OBSERVABLE RESULT: second motion is position-only and preserves Floating size.
    // UNCOVERED GAP: current reducer has no first-activation distinction.
    // FAILURE MUTATION: emit full configure effects on every move motion.
    // REPRESENTATIVE CASE: repeated maximized drag motion.
    #[test]
    fn t07_second_motion_after_restore_is_position_only() {
        let s0 = maximized_state();
        let (s1, _) = reduce(&s0, press(1, 960, 20));
        let (s2, _) = reduce(&s1, motion(965, 25));
        let (s3, effects) = reduce(&s2, motion(985, 45));
        assert_eq!(s3.placement.mode, PlacementMode::Floating);
        assert_eq!(
            (s3.placement.current.w, s3.placement.current.h),
            (
                s0.placement.floating_restore.w,
                s0.placement.floating_restore.h
            )
        );
        assert_position_only(&effects);
    }

    // CONTRACT-REGRESSION T08: WM-owned fullscreen/maximized resize is refused.
    // TRIGGER: resize press and motion in each non-resizable WM-owned mode.
    // OBSERVABLE RESULT: no resize session, geometry, or native effects.
    // UNCOVERED GAP: current reducer starts ResizeSession in both modes.
    // FAILURE MUTATION: allow plan_resize to mutate current while mode-owned.
    // REPRESENTATIVE CASE: bottom-right resize against max/fullscreen.
    #[test]
    fn t08_maximized_and_fullscreen_resize_is_refused() {
        for mode in [PlacementMode::Maximized, PlacementMode::Fullscreen] {
            let mut s0 = floating_state();
            s0.placement.mode = mode;
            s0.placement.current = if mode == PlacementMode::Maximized {
                WA
            } else {
                SCREEN
            };
            let (s1, press_effects) = reduce(&s0, press(2, 1900, 1050));
            assert!(s1.session.is_idle(), "mode {mode:?}");
            assert_eq!(s1.placement, s0.placement, "mode {mode:?}");
            assert_eq!(press_effects, vec![FrameEffect::Noop], "mode {mode:?}");
            let (s2, motion_effects) = reduce(&s1, motion(1950, 1100));
            assert_eq!(s2.placement, s0.placement, "mode {mode:?}");
            assert_eq!(motion_effects, vec![FrameEffect::Noop], "mode {mode:?}");
        }
    }

    // CONTRACT-REGRESSION T09: snapped resize exits snapped mode once.
    // TRIGGER: snapped resize press, first resize motion, then another motion.
    // OBSERVABLE RESULT: one Floating transition; later resize stays Floating.
    // UNCOVERED GAP: current resize retains Snapped while changing its rect.
    // FAILURE MUTATION: apply resize without clearing the snapped mode.
    // REPRESENTATIVE CASE: left-snapped bottom-right resize.
    #[test]
    fn t09_snapped_resize_exits_once_and_stays_floating() {
        let s1 = pressed(&snapped_state(), 2, 960, 1080);
        let (s2, first_effects) = reduce(&s1, motion(965, 1085));
        assert_eq!(s2.placement.mode, PlacementMode::Floating);
        assert!(has_full_configure(&first_effects));
        let (s3, second_effects) = reduce(&s2, motion(985, 1105));
        assert_eq!(s3.placement.mode, PlacementMode::Floating);
        assert!(has_full_configure(&second_effects));
    }

    // CONTRACT-REGRESSION T11: mode/rect invariants survive pending max motion.
    // TRIGGER: maximized title press followed by sub-threshold motion.
    // OBSERVABLE RESULT: Maximized still owns the work-area rect.
    // UNCOVERED GAP: current placement is rewritten from the restore snapshot.
    // FAILURE MUTATION: mutate current before activation is complete.
    // REPRESENTATIVE CASE: three-pixel max drag jitter.
    #[test]
    fn t11_maximized_pending_motion_preserves_mode_rect_invariant() {
        let s1 = pressed(&maximized_state(), 1, 960, 20);
        let (s2, _effects) = reduce(&s1, motion(963, 20));
        assert_eq!(s2.placement.mode, PlacementMode::Maximized);
        assert!(!matches!(s2.placement.mode, PlacementMode::Snapped(_)));
    }

    // CONTRACT-REGRESSION T12: mode/rect invariants survive pending snap motion.
    // TRIGGER: snapped title press followed by sub-threshold motion.
    // OBSERVABLE RESULT: Snapped still owns its snap rect.
    // UNCOVERED GAP: current move planner can change current under Snapped.
    // FAILURE MUTATION: let pending motion update authoritative placement.
    // REPRESENTATIVE CASE: three-pixel left-snap drag jitter.
    #[test]
    fn t12_snapped_pending_motion_preserves_mode_rect_invariant() {
        let s0 = snapped_state();
        let (s1, _) = reduce(&s0, press(1, 500, 20));
        let (s2, _effects) = reduce(&s1, motion(503, 20));
        assert_eq!(s2.placement.mode, PlacementMode::Snapped(SnapTarget::Left));
        assert_eq!(
            (s2.placement.current.w, s2.placement.current.h),
            (s0.placement.current.w, s0.placement.current.h)
        );
        assert_eq!(s2.placement.floating_restore, s0.placement.floating_restore);
        assert_eq!(s2.placement.current, s0.placement.current);
    }

    // CONTRACT-REGRESSION T13: first restore is full configure, not a move alias.
    // TRIGGER: max drag-away at the activation boundary.
    // OBSERVABLE RESULT: frame root, client local, and client root all configure.
    // UNCOVERED GAP: current max path emits only MoveFrameRoot plus notify.
    // FAILURE MUTATION: collapse restore into position-only effects.
    // REPRESENTATIVE CASE: exactly four-pixel max activation.
    #[test]
    fn t13_max_restore_activation_has_all_geometry_domains() {
        let s1 = pressed(&maximized_state(), 1, 960, 20);
        let (s2, effects) = reduce(&s1, motion(964, 24));
        assert_eq!(s2.placement.mode, PlacementMode::Floating);
        assert!(has_full_configure(&effects));
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, FrameEffect::MoveFrameRoot { .. }))
        );
    }

    // CONTRACT-REGRESSION T14: activation and later motion have distinct plans.
    // TRIGGER: max press, first drag-away, then a second drag motion.
    // OBSERVABLE RESULT: full configure once, position-only thereafter.
    // UNCOVERED GAP: current reducer uses one position-only plan for both.
    // FAILURE MUTATION: reuse the first activation plan for subsequent motions.
    // REPRESENTATIVE CASE: two-sample maximized drag.
    #[test]
    fn t14_restore_then_motion_has_one_full_then_position_only_plan() {
        let s1 = pressed(&maximized_state(), 1, 960, 20);
        let (s2, first_effects) = reduce(&s1, motion(965, 25));
        assert!(has_full_configure(&first_effects));
        let (_, second_effects) = reduce(&s2, motion(985, 45));
        assert_position_only(&second_effects);
    }

    const ALL_SNAP_TARGETS: [SnapTarget; 8] = [
        SnapTarget::Left,
        SnapTarget::Right,
        SnapTarget::Top,
        SnapTarget::Bottom,
        SnapTarget::TopLeft,
        SnapTarget::TopRight,
        SnapTarget::BottomLeft,
        SnapTarget::BottomRight,
    ];

    const EVEN_WORK_AREAS: [RootRect; 3] = [
        RootRect::new(0, 0, 1920, 1080),
        RootRect::new(7, 9, 1366, 768),
        RootRect::new(100, 40, 1280, 1024),
    ];

    const ODD_WORK_AREAS: [RootRect; 4] = [
        RootRect::new(0, 0, 1921, 1080),
        RootRect::new(3, 5, 1920, 1081),
        RootRect::new(100, 40, 801, 601),
        RootRect::new(11, 13, 1367, 769),
    ];

    // Preview owner: window-core snap_geometry, reached through the same
    // frame-to-core target mapping that wm.rs uses for release actions.
    fn preview_snap_rect(work_area: RootRect, target: SnapTarget) -> RootRect {
        let core_target = match target {
            SnapTarget::Left => CoreSnapTarget::LeftHalf,
            SnapTarget::Right => CoreSnapTarget::RightHalf,
            SnapTarget::Top => CoreSnapTarget::TopHalf,
            SnapTarget::Bottom => CoreSnapTarget::BottomHalf,
            SnapTarget::TopLeft => CoreSnapTarget::TopLeftQuarter,
            SnapTarget::TopRight => CoreSnapTarget::TopRightQuarter,
            SnapTarget::BottomLeft => CoreSnapTarget::BottomLeftQuarter,
            SnapTarget::BottomRight => CoreSnapTarget::BottomRightQuarter,
        };
        let area = CoreRect::new(work_area.x, work_area.y, work_area.w, work_area.h);
        let size = CoreSize::new(work_area.w, work_area.h);
        let rect = snap_geometry(core_target, area, size);
        RootRect::new(rect.x, rect.y, rect.width, rect.height)
    }

    fn snap_commit_mismatches(work_areas: &[RootRect]) -> Vec<String> {
        let mut out = Vec::new();
        for &work_area in work_areas {
            for target in ALL_SNAP_TARGETS {
                let commit = snap_rect(work_area, target);
                let preview = preview_snap_rect(work_area, target);
                if commit != preview {
                    out.push(format!(
                        "work_area={:?} target={:?} commit={:?} preview={:?}",
                        work_area, target, commit, preview
                    ));
                }
            }
        }
        out
    }

    // GREEN INVARIANT: on even work areas the committed rect equals the preview rect.
    #[test]
    fn snap_commit_matches_preview_on_even_work_areas() {
        let mismatches = snap_commit_mismatches(&EVEN_WORK_AREAS);
        assert!(
            mismatches.is_empty(),
            "commit/preview divergence:\n{}",
            mismatches.join("\n")
        );
    }

    // GREEN INVARIANT: odd widths/heights. snap_rect places the right/bottom half at
    // floor(size/2) extent starting at the ceil offset; snap_geometry mirrors that exactly.
    #[test]
    fn snap_commit_matches_preview_on_odd_work_areas() {
        let all: Vec<RootRect> = EVEN_WORK_AREAS
            .iter()
            .chain(ODD_WORK_AREAS.iter())
            .copied()
            .collect();
        let mismatches = snap_commit_mismatches(&all);
        assert!(
            mismatches.is_empty(),
            "commit/preview divergence:\n{}",
            mismatches.join("\n")
        );
    }
}
