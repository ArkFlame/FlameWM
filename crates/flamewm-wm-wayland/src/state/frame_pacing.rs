use super::*;

/// What the tty backend should do with one output on the next loop turn.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameDecision {
    /// Nothing changed: no paint, and no timer that would paint later.
    Idle,
    /// Paint now.
    Draw,
    /// A frame is already in flight. The request is kept, and the vblank that clears `queued`
    /// plans this output again, so a request made while queued is deferred, never lost.
    Deferred,
}

/// Frame scheduling rules of the tty backend.
///
/// Free of GPU, loop and session state so the rules are testable on a machine without DRM:
/// * paused wins over everything — a paused session paints nothing at all;
/// * a queued frame defers the request to its vblank;
/// * otherwise a paint happens only for a dirty output or an outstanding client frame callback,
///   so an idle compositor issues zero paints and needs no periodic timer.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
pub fn plan_frame(queued: bool, dirty: bool, has_callback: bool, paused: bool) -> FrameDecision {
    if paused {
        return FrameDecision::Idle;
    }
    let wanted = dirty || has_callback;
    match (queued, wanted) {
        (true, true) => FrameDecision::Deferred,
        (true | false, false) => FrameDecision::Idle,
        (false, true) => FrameDecision::Draw,
    }
}

/// How a paint obtains its scene elements (everything except the cursor).
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneBuild {
    /// Rebuild the window, decoration and desktop elements and recapture the backdrop.
    Full,
    /// Keep the cached elements; only the cursor elements change.
    CursorOnly,
}

/// Cursor-only paints need a cached scene and no scene change since it was built.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
pub fn scene_build(cached: bool, scene_dirty: bool) -> SceneBuild {
    if cached && !scene_dirty {
        SceneBuild::CursorOnly
    } else {
        SceneBuild::Full
    }
}

/// Whether a paint pass should arm the bounded retry timer.
///
/// Only a real sync or buffer error does. An empty frame or a clean output must never arm a timer:
/// that is what keeps an idle compositor at zero periodic paints.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
pub fn arm_retry(needs_retry: bool, dirty: bool, paused: bool) -> bool {
    needs_retry && !dirty && !paused
}

/// Whether one paint pass owes the bounded retry timer.
///
/// A real sync or buffer failure arms it ([`arm_retry`]); so does an output whose GPU fence did not
/// signal this pass ([`OutputFrames::defer_sync`], recognised by `sync_waited_ms > 0`), because a
/// deferred output stays dirty and the bounded retry pass is the only thing that paints it again.
/// Never while paused, and the caller still caps the number of attempts.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
pub fn retry_owed(needs_retry: bool, dirty: bool, sync_waited_ms: u128, paused: bool) -> bool {
    if paused {
        return false;
    }
    sync_waited_ms > 0 || arm_retry(needs_retry, dirty, paused)
}

/// Longest a single paint may block on a GPU fence before the output is deferred instead.
///
/// Smithay returns `RenderFrameResult::needs_sync` when KMS cannot wait for the render itself (no
/// explicit fencing, or a fence that cannot be handed to KMS). The documented contract is that the
/// caller synchronizes before rendering/submitting again; a wait longer than this means the GPU is not
/// going to finish for us, so the compositor must stop blocking the loop.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
pub const SYNC_WAIT_BUDGET: Duration = Duration::from_millis(250);

/// [`SYNC_WAIT_BUDGET`] in milliseconds, for the pure decision function.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
pub const SYNC_WAIT_BUDGET_MS: u128 = SYNC_WAIT_BUDGET.as_millis();

/// Cumulative fence waiting after which an output is treated as wedged and left to the retry bound.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
pub const SYNC_WAIT_LIMIT_MS: u128 = 750;

/// What the tty backend does about the GPU fence of a frame whose render returned `needs_sync`.
///
/// There is no third option on purpose: *never* render again without either having waited for the
/// fence or having deferred this output. Rendering over an unfinished buffer is what produces the
/// repeated buffer/IO errors and finally a wedged pipeline.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncWait {
    /// Nothing outstanding or the fence signaled: the frame may be queued and the next render may run.
    Wait,
    /// The fence did not signal inside the budget. Do not queue over it: leave the output pending and
    /// let a bounded retry re-plan it instead of blocking the compositor thread.
    Defer,
    /// The fence is past the cumulative limit: the pipeline is wedged. Mark the output and move on to
    /// the other outputs; the bounded retry counter is what finally stops the attempts.
    Fail,
}

/// Decides what to do with a fence that has already been waited on for `waited_ms` (cumulative for one
/// output, so repeated deferrals reach [`SyncWait::Fail`] instead of waiting forever).
///
/// Pure on purpose: the whole sync contract is then testable on a machine without a GPU.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
pub fn sync_wait_decision(needs_sync: bool, waited_ms: u128) -> SyncWait {
    if !needs_sync {
        // Nothing to synchronize with: this is the only case where queueing straight away is correct.
        return SyncWait::Wait;
    }
    if waited_ms < SYNC_WAIT_BUDGET_MS {
        SyncWait::Wait
    } else if waited_ms < SYNC_WAIT_LIMIT_MS {
        SyncWait::Defer
    } else {
        SyncWait::Fail
    }
}

/// Whether a connector scan that created `outputs` outputs must fail startup.
///
/// Never for a partial scan: one connector that cannot be set up is skipped and the session runs on
/// the others, exactly like anvil. Only a scan that left zero outputs behind is fatal.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
pub fn scan_failed(outputs: usize) -> bool {
    outputs == 0
}

/// What one seat GPU can drive, as seen by the tty backend's startup probe.
///
/// Plain data on purpose: the choice between several cards is then testable without a GPU.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuProbe {
    /// This card has a connected internal panel (eDP, LVDS, DSI, DPI).
    pub internal: bool,
    /// This card has at least one connected connector, internal or external.
    pub connected: bool,
}

/// Picks the GPU to drive from the cards that opened, in probe order.
///
/// The first card with a connected internal panel wins, else the first with any connected
/// connector, else the first card that opened at all. That last case is deliberate: a machine whose
/// only card reports nothing connected still gets an attempt, and the connector scan is what
/// reports `stage=outputs status=fail detail=no_connector` — a clearer failure than "no GPU".
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
pub fn pick_gpu(probes: &[GpuProbe]) -> Option<usize> {
    probes
        .iter()
        .position(|probe| probe.internal)
        .or_else(|| probes.iter().position(|probe| probe.connected))
        .or((!probes.is_empty()).then_some(0))
}

/// Per-output frame scheduling state. The tty backend keeps one of these per connector; the
/// transitions live here so `plan_frame` is exercised without a GPU. `dirty` is deliberately
/// public: a request that arrives while `queued` survives as `dirty` until the vblank re-plans.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OutputFrames {
    /// A frame is queued and waiting for its vblank.
    pub queued: bool,
    /// This output has to be painted (scene changed, or its state was reset).
    pub dirty: bool,
    /// The last pass failed in a way a bounded retry can recover from.
    pub needs_retry: bool,
    /// Cumulative milliseconds this output has spent blocking on a GPU fence since it last queued a
    /// frame. It is what turns "deferred" into "wedged" after repeated passes, and it is why the wait
    /// never has to be unbounded.
    pub sync_waited_ms: u128,
}

#[cfg_attr(not(feature = "drm"), allow(dead_code))]
impl OutputFrames {
    /// What to do right now.
    pub fn plan(&self, has_callback: bool, paused: bool) -> FrameDecision {
        plan_frame(self.queued, self.dirty, has_callback, paused)
    }

    /// A frame went to the kernel: nothing is stale any more.
    pub fn queue(&mut self) {
        self.queued = true;
        self.dirty = false;
        self.needs_retry = false;
        self.sync_waited_ms = 0;
    }

    /// The renderer drew nothing: no vblank follows, and nothing is in flight to synchronize with, so
    /// no retry may be armed (`empty(false)`). An idle output arms no timer.
    pub fn empty(&mut self, needs_sync: bool) {
        self.queued = false;
        self.dirty = false;
        self.needs_retry = needs_sync;
        self.sync_waited_ms = 0;
    }

    /// The frame failed; a bounded retry is owed.
    pub fn fail(&mut self) {
        self.needs_retry = true;
    }

    /// The GPU fence of this pass did not signal: record the time already spent waiting, keep the
    /// output pending (so a bounded retry re-plans it) and ask for that retry.
    ///
    /// The frame is *not* queued: submitting a buffer the GPU is still writing is what corrupts the
    /// scanout and produces the repeated buffer/IO errors.
    pub fn defer_sync(&mut self, waited_ms: u128) {
        self.sync_waited_ms = self.sync_waited_ms.saturating_add(waited_ms);
        self.queued = false;
        self.dirty = true;
        self.needs_retry = true;
    }

    /// The vblank that presented the queued frame arrived.
    pub fn presented(&mut self) {
        self.queued = false;
    }

    /// Everything stopped (pause, device loss): no frame in flight, nothing owed.
    pub fn park(&mut self) {
        self.queued = false;
        self.dirty = false;
        self.needs_retry = false;
        self.sync_waited_ms = 0;
    }

    /// Coming back from a session pause: scanout was reset, so this output is stale and has to be
    /// painted in full.
    pub fn wake(&mut self) {
        self.queued = false;
        self.dirty = true;
        self.needs_retry = false;
        self.sync_waited_ms = 0;
    }
}

/// What the nested winit path should do with one host frame opportunity.
///
/// The tty backend already decides zero-frame versus paint through [`FrameDecision`]. The nested
/// path had the same rule spelled out as an inline condition in `render_frame`; keeping the rule
/// here makes it testable without a window system and gives both backends one typed facade.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NestedFrame {
    /// No pixels are owed: do not bind, draw, submit or send frame callbacks.
    Idle,
    /// Paint this frame.
    Paint,
}

/// The nested frame gate, pure on its four inputs.
///
/// A scene change, a pending dump, a locked output or a compositor-drawn client cursor all owe a
/// frame. With none of them and a host-drawn pointer, pointer motion needs no paint at all, so the
/// opportunity is spent without touching the backend. This is the same rule `render_frame` applied
/// inline; expressing it as data is step 1 of the damage-tracking design and does not change what
/// any frame draws.
pub const fn nested_frame(
    scene_dirty: bool,
    dump_pending: bool,
    locked: bool,
    host_draws_pointer: bool,
) -> NestedFrame {
    if scene_dirty || dump_pending || locked || !host_draws_pointer {
        NestedFrame::Paint
    } else {
        NestedFrame::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A host-drawn pointer with nothing else pending is the one case that spends no frame.
    #[test]
    fn idle_nested_opportunity_paints_nothing() {
        assert_eq!(nested_frame(false, false, false, true), NestedFrame::Idle);
    }

    /// Each of the four inputs on its own forces a paint, including a client cursor surface.
    #[test]
    fn every_owed_change_forces_a_nested_paint() {
        assert_eq!(nested_frame(true, false, false, true), NestedFrame::Paint);
        assert_eq!(nested_frame(false, true, false, true), NestedFrame::Paint);
        assert_eq!(nested_frame(false, false, true, true), NestedFrame::Paint);
        assert_eq!(nested_frame(false, false, false, false), NestedFrame::Paint);
    }

    /// Several owed changes collapse to one paint, never to an idle frame.
    #[test]
    fn combined_nested_changes_still_paint_once() {
        assert_eq!(nested_frame(true, true, true, false), NestedFrame::Paint);
    }
}
