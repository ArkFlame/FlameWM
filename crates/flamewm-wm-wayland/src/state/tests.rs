use super::frame_pacing::{
    FrameDecision, GpuProbe, OutputFrames, SYNC_WAIT_BUDGET_MS, SYNC_WAIT_LIMIT_MS, SceneBuild,
    SyncWait, arm_retry, pick_gpu, plan_frame, retry_owed, scan_failed, scene_build,
    sync_wait_decision,
};
use super::{Rect, focus_needs_restack, reserve_struts};

/// Named and hidden pointers are drawn by the host window (no software frame per motion);
/// client cursor surfaces stay software-drawn.
#[test]
fn host_draws_named_and_hidden_pointers_only() {
    use super::State;
    use smithay::input::pointer::{CursorIcon, CursorImageStatus};
    assert!(State::host_draws_pointer_status(
        &CursorImageStatus::default_named()
    ));
    assert!(State::host_draws_pointer_status(&CursorImageStatus::Named(
        CursorIcon::Pointer
    )));
    assert!(State::host_draws_pointer_status(&CursorImageStatus::Hidden));
}

/// Pointer motion over an unchanged scene reuses the cached elements; any scene change, or no
/// cache yet, rebuilds them.
#[test]
fn cursor_only_paint_reuses_scene_only_when_clean_and_cached() {
    assert_eq!(scene_build(true, false), SceneBuild::CursorOnly);
    assert_eq!(scene_build(true, true), SceneBuild::Full);
    assert_eq!(scene_build(false, false), SceneBuild::Full);
    assert_eq!(scene_build(false, true), SceneBuild::Full);
}

/// Clearing keyboard focus is a restack transition even when the model order is unchanged.
#[test]
fn clearing_focus_requests_xwayland_restack() {
    assert!(focus_needs_restack(Some(7), None, false));
    assert!(focus_needs_restack(None, None, true));
    assert!(!focus_needs_restack(None, None, false));
}

/// One connector that cannot be set up is skipped; only zero outputs is fatal.
#[test]
fn zero_outputs_is_fatal_and_one_of_two_is_not() {
    assert!(scan_failed(0));
    assert!(!scan_failed(1));
    assert!(!scan_failed(2));
}

/// The GPU pick: internal panel first, then any connected connector, then the first card.
#[test]
fn gpu_pick_prefers_internal_then_connected_then_first() {
    let nothing = GpuProbe::default();
    let external = GpuProbe {
        internal: false,
        connected: true,
    };
    let panel = GpuProbe {
        internal: true,
        connected: true,
    };
    assert_eq!(pick_gpu(&[]), None);
    // Nothing connected anywhere: the first card is still tried, so the failure is reported by
    // the connector scan instead of as "no GPU".
    assert_eq!(pick_gpu(&[nothing]), Some(0));
    assert_eq!(pick_gpu(&[nothing, nothing]), Some(0));
    assert_eq!(pick_gpu(&[nothing, external]), Some(1));
    assert_eq!(pick_gpu(&[external, nothing]), Some(0));
    // A hybrid laptop whose boot GPU has nothing connected falls through to the panel card…
    assert_eq!(pick_gpu(&[nothing, external, panel]), Some(2));
    // …and a connected panel beats an earlier card that only has external connectors.
    assert_eq!(pick_gpu(&[external, panel]), Some(1));
    assert_eq!(pick_gpu(&[panel, external]), Some(0));
}

#[test]
fn struts_shrink_each_edge_by_the_largest_reservation() {
    let area = Rect::new(0, 0, 1000, 800);
    let work = reserve_struts(
        area,
        [[0, 0, 0, 30], [0, 0, 0, 50], [20, 0, 0, 0]].into_iter(),
    );
    assert_eq!(work, Rect::new(20, 0, 980, 750));
}

#[test]
fn no_struts_keeps_the_area() {
    let area = Rect::new(10, 20, 300, 200);
    assert_eq!(reserve_struts(area, std::iter::empty()), area);
}

/// The first frame after init is painted: startup marks every output dirty.
#[test]
fn initial_frame_paints() {
    let frames = OutputFrames {
        dirty: true,
        ..OutputFrames::default()
    };
    assert_eq!(frames.plan(false, false), FrameDecision::Draw);
    // Even with no output state at all, the dirty flag alone is enough.
    assert_eq!(plan_frame(false, true, false, false), FrameDecision::Draw);
}

/// An idle compositor paints nothing; one change paints exactly one frame.
#[test]
fn dirty_after_idle_renders_once() {
    let mut frames = OutputFrames::default();
    assert_eq!(frames.plan(false, false), FrameDecision::Idle);
    assert_eq!(frames.plan(false, false), FrameDecision::Idle);
    frames.dirty = true;
    assert_eq!(frames.plan(false, false), FrameDecision::Draw);
    // The paint consumes the request, so the next plan is idle again.
    frames.queue();
    frames.presented();
    assert_eq!(frames.plan(false, false), FrameDecision::Idle);
}

/// A request that arrives while a frame is in flight survives to its vblank.
#[test]
fn request_while_queued_is_deferred_not_lost() {
    let mut frames = OutputFrames::default();
    frames.queue();
    frames.dirty = true;
    assert_eq!(frames.plan(false, false), FrameDecision::Deferred);
    assert!(
        frames.dirty,
        "a deferred request must not be cleared by planning"
    );
    // The vblank clears `queued`; the surviving request paints on the very next plan.
    frames.presented();
    assert_eq!(frames.plan(false, false), FrameDecision::Draw);
}

/// Paused and idle both mean zero paints, whatever else is pending.
#[test]
fn idle_and_paused_never_paint() {
    let idle = OutputFrames::default();
    assert_eq!(idle.plan(false, false), FrameDecision::Idle);
    let paused = OutputFrames {
        dirty: true,
        queued: false,
        needs_retry: true,
        sync_waited_ms: 0,
    };
    assert_eq!(paused.plan(true, true), FrameDecision::Idle);
    assert_eq!(paused.plan(false, true), FrameDecision::Idle);
    assert_eq!(plan_frame(true, true, true, true), FrameDecision::Idle);
    // A callback alone is enough to want a frame when the session is ours.
    assert_eq!(plan_frame(false, false, true, false), FrameDecision::Draw);
}

/// A resume resets scanout, so every output has to be repainted in full.
#[test]
fn resume_forces_a_full_redraw() {
    let mut frames = OutputFrames {
        queued: true,
        dirty: false,
        needs_retry: true,
        sync_waited_ms: 0,
    };
    frames.park();
    assert_eq!(frames, OutputFrames::default());
    assert_eq!(frames.plan(false, true), FrameDecision::Idle);
    frames.wake();
    assert_eq!(frames.plan(false, false), FrameDecision::Draw);
}

/// The sync contract, as a pure decision: only "no fence" and "inside the budget" may render, and
/// repeated deferrals end in `Fail` instead of an unbounded wait.
#[test]
fn sync_wait_decision_waits_within_the_budget_then_defers_then_fails() {
    // No fence to synchronize with: queue straight away.
    assert_eq!(sync_wait_decision(false, 0), SyncWait::Wait);
    assert_eq!(
        sync_wait_decision(false, SYNC_WAIT_LIMIT_MS * 10),
        SyncWait::Wait
    );
    // Inside the budget a bounded wait is still the right answer (it normally signals in <1 ms).
    assert_eq!(sync_wait_decision(true, 0), SyncWait::Wait);
    assert_eq!(
        sync_wait_decision(true, SYNC_WAIT_BUDGET_MS - 1),
        SyncWait::Wait
    );
    // The budget is spent: stop blocking and defer this output, never render over the fence.
    assert_eq!(
        sync_wait_decision(true, SYNC_WAIT_BUDGET_MS),
        SyncWait::Defer
    );
    assert_eq!(
        sync_wait_decision(true, SYNC_WAIT_LIMIT_MS - 1),
        SyncWait::Defer
    );
    // Past the cumulative limit the pipeline is wedged: fail the output, keep the loop alive.
    assert_eq!(sync_wait_decision(true, SYNC_WAIT_LIMIT_MS), SyncWait::Fail);
    assert_eq!(sync_wait_decision(true, u128::MAX), SyncWait::Fail);
    // The thresholds are ordered, or the three outcomes would be unreachable in this order.
    const { assert!(SYNC_WAIT_BUDGET_MS < SYNC_WAIT_LIMIT_MS) };
}

/// A deferred fence keeps the output pending and asks for the bounded retry, and the waiting
/// accumulates so it can never loop forever; the next queued frame clears the accumulation.
#[test]
fn deferred_fence_keeps_the_output_pending_and_accumulates() {
    let mut frames = OutputFrames {
        dirty: true,
        ..OutputFrames::default()
    };
    assert_eq!(frames.plan(false, false), FrameDecision::Draw);
    frames.defer_sync(SYNC_WAIT_BUDGET_MS);
    // Not queued (the GPU is still writing that buffer), still wanting a frame, retry owed.
    assert!(!frames.queued);
    assert!(frames.dirty);
    assert!(frames.needs_retry);
    // `arm_retry` alone would not arm it: the output is dirty, so the pass owes the retry itself.
    assert!(!arm_retry(frames.needs_retry, frames.dirty, false));
    assert!(retry_owed(
        frames.needs_retry,
        frames.dirty,
        frames.sync_waited_ms,
        false
    ));
    assert!(!retry_owed(
        frames.needs_retry,
        frames.dirty,
        frames.sync_waited_ms,
        true
    ));
    assert_eq!(frames.sync_waited_ms, SYNC_WAIT_BUDGET_MS);
    assert_eq!(frames.plan(false, false), FrameDecision::Draw);
    assert_eq!(
        sync_wait_decision(true, frames.sync_waited_ms),
        SyncWait::Defer
    );
    // Waiting accumulates across passes until the decision becomes Fail.
    frames.defer_sync(SYNC_WAIT_BUDGET_MS);
    frames.defer_sync(SYNC_WAIT_BUDGET_MS);
    assert!(frames.sync_waited_ms >= SYNC_WAIT_LIMIT_MS);
    assert_eq!(
        sync_wait_decision(true, frames.sync_waited_ms),
        SyncWait::Fail
    );
    // A frame that did queue clears the accumulation for the next cycle.
    frames.queue();
    assert_eq!(frames.sync_waited_ms, 0);
    assert_eq!(
        sync_wait_decision(true, frames.sync_waited_ms),
        SyncWait::Wait
    );
}

/// An empty frame leaves the output idle: no vblank follows and no timer may be armed.
#[test]
fn empty_render_without_dirty_arms_no_timer() {
    let mut frames = OutputFrames {
        dirty: true,
        ..OutputFrames::default()
    };
    assert_eq!(frames.plan(false, false), FrameDecision::Draw);
    frames.empty(false);
    assert!(!frames.needs_retry);
    assert!(!arm_retry(frames.needs_retry, frames.dirty, false));
    // Only a real sync or buffer error earns the bounded retry, and never while paused.
    assert!(arm_retry(true, false, false));
    assert!(!arm_retry(true, false, true));
    assert!(!arm_retry(true, true, false));
}
