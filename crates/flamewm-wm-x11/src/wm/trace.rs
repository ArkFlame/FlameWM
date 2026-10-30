//! Pointer, resize, and frame-dispatch trace emitters plus workspace counters.
use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_pointer_trace(
    id: flamewm_debug::DebugEventId,
    phase: &str,
    client: Window,
    source: Window,
    region: Option<FrameRegion>,
    event: FrameEvent,
    pre: InteractionSession,
    post: Option<InteractionSession>,
    commit: Option<&interaction::PointerCommit>,
    placement: Option<PlacementMode>,
    post_placement: Option<PlacementMode>,
    snap_target: Option<&dyn std::fmt::Debug>,
    preview: Option<bool>,
) {
    let pointer = match event {
        FrameEvent::Press { pointer, .. }
        | FrameEvent::Motion { pointer }
        | FrameEvent::Release { pointer, .. } => Some(pointer),
        _ => None,
    };
    let delta = pointer.and_then(|point| {
        let start = match pre {
            InteractionSession::Move(session) => session.start_pointer,
            InteractionSession::Resize(session) => session.start_pointer,
            _ => return None,
        };
        Some((
            point.x.saturating_sub(start.x),
            point.y.saturating_sub(start.y),
        ))
    });
    let cooldown = if matches!(event, FrameEvent::Motion { .. }) {
        std::time::Duration::ZERO
    } else {
        std::time::Duration::from_millis(100)
    };
    flamewm_debug::emit(id, cooldown, || {
        // Formatted only when tracing is enabled; keeps the `Some("Target")` text shape.
        let commit = commit.map(|commit| {
            let effects = join_effects(&commit.effects);
            format!(
                "grab={:?} ungrab={} abort_on_grab_fail={} effects=[{effects}]",
                commit.grab, commit.ungrab, commit.abort_on_grab_fail
            )
        });
        let snap_target = snap_target.map(|target| format!("{target:?}"));
        format!(
            "phase={phase} client={client} source={source} region={region:?} event={event:?} pointer={pointer:?} delta={delta:?} pre={pre:?} post={post:?} placement={placement:?} post_placement={post_placement:?} snap_target={snap_target:?} preview={preview:?} commit={commit:?}"
        )
    });
}

pub(super) fn emit_native_configure<T, E: std::fmt::Debug>(
    id: flamewm_debug::DebugEventId,
    client: Window,
    xid: Window,
    detail: impl FnOnce() -> String,
    result: &Result<T, E>,
) {
    // Built lazily: this runs on every drag motion, and the strings are only
    // needed when debug emission is enabled.
    flamewm_debug::emit(id, std::time::Duration::from_millis(100), || {
        let outcome = match result {
            Ok(_) => "ok".to_owned(),
            Err(error) => format!("error={error:?}"),
        };
        format!("client={client} xid={xid} {} result={outcome}", detail())
    });
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PointerIngress {
    pub(super) kind: &'static str,
    pub(super) event: Window,
    pub(super) child: Window,
    pub(super) root_x: i16,
    pub(super) root_y: i16,
}

#[derive(Default)]
pub(super) struct PointerIngressProbe {
    pub(super) motion_dispatched: bool,
    pub(super) motion_handled: bool,
    pub(super) last_motion: Option<PointerIngress>,
}

#[derive(Debug)]
pub(super) struct ResizeProbe {
    pub(super) client: Window,
    pub(super) source: Window,
    pub(super) region: FrameRegion,
    pub(super) start_pointer: RootPoint,
    pub(super) last_pointer: RootPoint,
    pub(super) effect_seen: bool,
    pub(super) native_first: Option<String>,
    pub(super) native_last: Option<String>,
}

pub(super) fn resize_edges(session: InteractionSession) -> Option<ResizeEdges> {
    match session {
        InteractionSession::Resize(session) => Some(session.edges),
        _ => None,
    }
}

pub(super) fn emit_resize_probe(
    stage: &str,
    boundary: &str,
    probe: &ResizeProbe,
    pointer: Option<RootPoint>,
    session: InteractionSession,
    commit: Option<&interaction::PointerCommit>,
) {
    let delta = pointer.map(|point| {
        (
            point.x.saturating_sub(probe.start_pointer.x),
            point.y.saturating_sub(probe.start_pointer.y),
        )
    });
    let session_edges = resize_edges(session).or(match probe.region {
        FrameRegion::Resize(edges) => Some(edges),
        _ => None,
    });
    let (plan_noop, plan_rect, effects) = commit.map_or_else(
        || (None, None, String::from("[]")),
        |commit| {
            let rect = commit.effects.iter().find_map(|effect| match effect {
                FrameEffect::ConfigureFrameRoot { x, y, w, h } => Some((*x, *y, *w, *h)),
                _ => None,
            });
            (
                Some(
                    commit
                        .effects
                        .iter()
                        .all(|effect| matches!(effect, FrameEffect::Noop)),
                ),
                rect,
                join_effects(&commit.effects),
            )
        },
    );
    let id = if stage == "effect" {
        flamewm_debug::WM_FRAME_GEOMETRY_RESIZE_NATIVE
    } else {
        flamewm_debug::WM_FRAME_GEOMETRY_PLAN
    };
    flamewm_debug::emit(id, std::time::Duration::ZERO, || {
        format!(
            "probe=resize_edge stage={stage} boundary={boundary} client={} source={} region={:?} session_edges={session_edges:?} pointer={pointer:?} delta={delta:?} plan_noop={plan_noop:?} plan_rect={plan_rect:?} effects=[{effects}] native_first={:?} native_last={:?}",
            probe.client, probe.source, probe.region, probe.native_first, probe.native_last,
        )
    });
}

pub(super) fn join_effects(effects: &[FrameEffect]) -> String {
    effects
        .iter()
        .map(|effect| format!("{effect:?}"))
        .collect::<Vec<_>>()
        .join(",")
}

pub(super) fn emit_frame_dispatch(detail: impl FnOnce() -> String) {
    flamewm_debug::emit(
        flamewm_debug::WM_FRAME_EVENT_DISPATCH,
        std::time::Duration::ZERO,
        detail,
    );
}

pub(super) fn pointer_ingress(event: &Event) -> Option<PointerIngress> {
    let ingress = |kind, event, child, root_x, root_y| PointerIngress {
        kind,
        event,
        child,
        root_x,
        root_y,
    };
    match event {
        Event::ButtonPress(e) => Some(ingress("ButtonPress", e.event, e.child, e.root_x, e.root_y)),
        Event::ButtonRelease(e) => Some(ingress(
            "ButtonRelease",
            e.event,
            e.child,
            e.root_x,
            e.root_y,
        )),
        Event::MotionNotify(e) => Some(ingress(
            "MotionNotify",
            e.event,
            e.child,
            e.root_x,
            e.root_y,
        )),
        _ => None,
    }
}

pub(super) struct WorkspaceCounterSet {
    pub(super) switch_total: flamewm_profiler::CounterPoint,
    pub(super) visibility: flamewm_profiler::CounterPoint,
    pub(super) ewmh_current: flamewm_profiler::CounterPoint,
    pub(super) shell_publish: flamewm_profiler::CounterPoint,
    pub(super) query_tree: flamewm_profiler::CounterPoint,
}

pub(super) fn workspace_counters() -> &'static WorkspaceCounterSet {
    use std::sync::OnceLock;
    static SET: OnceLock<WorkspaceCounterSet> = OnceLock::new();
    SET.get_or_init(|| WorkspaceCounterSet {
        switch_total: flamewm_profiler::CounterPoint::new("wm.workspace.switch.total"),
        visibility: flamewm_profiler::CounterPoint::new("wm.workspace.switch.visibility"),
        ewmh_current: flamewm_profiler::CounterPoint::new("wm.workspace.switch.ewmh_current"),
        shell_publish: flamewm_profiler::CounterPoint::new("wm.workspace.switch.shell_publish"),
        query_tree: flamewm_profiler::CounterPoint::new("wm.workspace.query_tree.count"),
    })
}
