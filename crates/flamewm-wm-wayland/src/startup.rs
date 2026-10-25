//! The tty backend's first-frame gate: **a queued page flip is not a presented frame**.
//!
//! `DrmCompositor::queue_frame` only hands the atomic commit to KMS; the flip completes at the first
//! vblank that `DrmCompositor::frame_submitted()` answers with `Some(..)`
//! (`smithay-0.7.0/src/backend/drm/compositor/mod.rs:2433` and `:2604`). Between those two points the
//! panel can stay black forever — a link that never trains, a buffer the display engine never latches,
//! the repeated `Buffer I/O error` of the hardware report — while the compositor already holds DRM
//! master. If the seat were grabbed at queue time, that combination would be unrecoverable: no frame
//! on screen, `EVIOCGRAB` on the keyboard, and no way to reach a console with Ctrl+Alt+Fn.
//!
//! So the backend has two separate boundaries and only the second one is readiness:
//!
//! * **queue** — `stage=firstframe status=ok code=7`, diagnostic only. Nothing is started, no deadline
//!   is disarmed, no keyboard is grabbed;
//! * **present** — `stage=presented status=ok code=8`, the first flip that actually completed. This is
//!   where libinput is started and where the bounded deadline is cancelled.
//!
//! One deadline, armed when the backend initialises and never cancelled by a queue, bounds the wait.
//! When it fires [`FirstFrameGate::failure`] distinguishes the two ways a native start can be dead:
//! `no_first_queue` (nothing ever reached KMS: the pipeline never got that far) and `pageflip_timeout`
//! (a flip was queued and never completed: exactly the black-panel case).
//!
//! Everything here is pure — `Instant`, `Duration` and integers, no DRM or GPU type — so the whole
//! boundary is unit-tested on a machine without a seat.

use std::time::{Duration, Instant};

/// One bounded deadline for the first *presented* frame, armed by the tty backend at init.
///
/// Deliberately not configurable: 8 s is already far longer than a working modeset plus one flip, and
/// a longer budget only keeps a black VT on screen. A session that needs to survive a really slow
/// panel is the session wrapper's problem (its own budget is wider than this one).
pub const FIRST_FRAME_DEADLINE: Duration = Duration::from_secs(8);

/// [`FirstFrameGate::failure`]: inside the deadline, or the gate is already satisfied.
pub const NO_FAILURE: &str = "none";

/// [`FirstFrameGate::failure`]: the deadline fired and no flip was ever queued to KMS.
pub const NO_FIRST_QUEUE: &str = "no_first_queue";

/// [`FirstFrameGate::failure`]: a flip was queued and never completed — the black-panel case.
pub const PAGEFLIP_TIMEOUT: &str = "pageflip_timeout";

/// Tracks the two boundaries of the first frame: what was queued and what was actually presented.
///
/// A CRTC is recorded when one of its flips is queued and dropped again when that flip completes, so
/// [`FirstFrameGate::presented`] can only ever be satisfied by a vblank of an output that really has a
/// flip in flight. That is what stops an unrelated vblank (another CRTC, a leftover commit of the
/// previous DRM master, a spurious event) from being reported as the first presented frame.
#[derive(Clone, Debug)]
pub struct FirstFrameGate {
    /// When the backend started, so the queue/present ages are comparable with the stage lines.
    started_at: Instant,
    /// The budget the one-shot deadline was armed with.
    deadline: Duration,
    /// Flips queued to KMS, successful `queue_frame` calls.
    queued: u64,
    /// Flips that completed, i.e. vblanks that answered `frame_submitted()` with `Some(..)`.
    presented: u64,
    /// CRTCs (as `u32::from(crtc::Handle)`) with a queued flip that has not completed yet.
    queued_crtcs: Vec<u32>,
    queued_at: Option<Instant>,
    presented_at: Option<Instant>,
    /// Last render or queue error, named when the deadline fires.
    last_error: Option<String>,
    /// The session is not ours: vblanks cannot be attributed to our flips while it is paused.
    paused: bool,
}

impl FirstFrameGate {
    /// A gate with the default [`FIRST_FRAME_DEADLINE`].
    pub fn new() -> Self {
        Self::with_deadline(FIRST_FRAME_DEADLINE)
    }

    pub fn with_deadline(deadline: Duration) -> Self {
        Self {
            started_at: Instant::now(),
            deadline,
            queued: 0,
            presented: 0,
            queued_crtcs: Vec::new(),
            queued_at: None,
            presented_at: None,
            last_error: None,
            paused: false,
        }
    }

    /// A flip of `crtc` was queued to KMS.
    ///
    /// Returns true for the first queue of the session, which is when the caller emits
    /// `stage=firstframe status=ok code=7`. Diagnostic only: this starts nothing, grabs nothing and
    /// deliberately does not touch the deadline — queueing a flip must never buy a session more time
    /// to present one.
    pub fn queued(&mut self, crtc: u32) -> bool {
        self.queued_crtcs.push(crtc);
        self.queued = self.queued.saturating_add(1);
        if self.queued_at.is_none() {
            self.queued_at = Some(Instant::now());
        }
        self.queued == 1
    }

    /// The flip of `crtc` completed: KMS presented the frame we queued.
    ///
    /// Returns true for the first presented frame of the session, which is when the caller emits
    /// `stage=presented status=ok code=8`, starts libinput and cancels the deadline. A vblank for a
    /// CRTC with no queued flip, or while the session is paused (the CRTC belongs to somebody else
    /// then), is not evidence of anything and is ignored.
    pub fn presented(&mut self, crtc: u32) -> bool {
        if self.paused {
            return false;
        }
        let Some(index) = self.queued_crtcs.iter().position(|queued| *queued == crtc) else {
            return false;
        };
        self.queued_crtcs.remove(index);
        self.presented = self.presented.saturating_add(1);
        if self.presented_at.is_none() {
            self.presented_at = Some(Instant::now());
        }
        self.presented == 1
    }

    /// The guard [`FirstFrameGate::presented`] applies, exposed for tests and callers that need to
    /// ask *before* acting: only the CRTC of an output with a queued flip can present a frame.
    pub fn presented_for(&self, crtc: u32) -> bool {
        !self.paused && self.queued_crtcs.contains(&crtc)
    }

    /// The first frame of this session is on screen. This is the only readiness signal there is.
    pub fn ready(&self) -> bool {
        self.presented > 0
    }

    pub fn queued_count(&self) -> u64 {
        self.queued
    }

    pub fn presented_count(&self) -> u64 {
        self.presented
    }

    /// The first queued CRTC, for the `crtc=` token of a failure line.
    pub fn queued_crtc(&self) -> Option<u32> {
        self.queued_crtcs.first().copied()
    }

    /// Milliseconds from backend init to the first queued flip.
    pub fn queued_ms(&self) -> Option<u128> {
        self.queued_at
            .map(|at| at.saturating_duration_since(self.started_at).as_millis())
    }

    /// Milliseconds from backend init to the first presented flip.
    pub fn presented_ms(&self) -> Option<u128> {
        self.presented_at
            .map(|at| at.saturating_duration_since(self.started_at).as_millis())
    }

    /// Why the deadline fired — `none` while the gate is satisfied or still inside its budget.
    ///
    /// `no_first_queue` and `pageflip_timeout` are different failures on purpose: the first says the
    /// pipeline never reached KMS, the second says KMS took the commit and never completed it. A log
    /// reader can tell a broken modeset from a black panel without a trace tool.
    pub fn failure(&self, elapsed: Duration) -> &'static str {
        if self.ready() || self.paused || elapsed < self.deadline {
            return NO_FAILURE;
        }
        if self.queued == 0 {
            NO_FIRST_QUEUE
        } else {
            PAGEFLIP_TIMEOUT
        }
    }

    /// Records the last render or queue error; the deadline line names it.
    pub fn note_error(&mut self, error: impl Into<String>) {
        self.last_error = Some(error.into());
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// The session was taken away (logind pause): any flip in flight is gone with it.
    pub fn pause(&mut self) {
        self.paused = true;
        self.queued_crtcs.clear();
    }

    /// The session is ours again: a resume needs a fresh queue *and* a fresh deadline, because the
    /// old one says nothing about the CRTC that was just handed back.
    pub fn resume(&mut self) {
        self.paused = false;
    }

    /// The `detail` of the failure line: short tokens, reason and counters first so a long error
    /// message can only truncate itself (the field is capped at 64 tokens by `main::detail_tokens`).
    pub fn failure_detail(&self, reason: &str, elapsed: Duration) -> String {
        let crtc = match self.queued_crtc() {
            Some(crtc) => crtc.to_string(),
            None => "none".to_owned(),
        };
        let mut detail = format!(
            "{reason} queued={} presented={} crtc={crtc} elapsed={}ms",
            self.queued,
            self.presented,
            elapsed.as_millis()
        );
        if let Some(error) = &self.last_error {
            detail.push_str(" last_render_error=");
            detail.push_str(error);
        }
        detail
    }
}

impl Default for FirstFrameGate {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FIRST_FRAME_DEADLINE, FirstFrameGate, NO_FAILURE, NO_FIRST_QUEUE, PAGEFLIP_TIMEOUT,
    };
    use std::time::Duration;

    /// The whole point of this module: `queue_frame` returning `Ok(())` is not a frame on screen.
    /// A gate that ever says "ready" on a queue alone would grab the keyboard for a black panel.
    #[test]
    fn queue_without_vblank_is_not_ready() {
        let mut gate = FirstFrameGate::new();
        assert!(gate.queued(63), "the first queue is reported once");
        assert_eq!(gate.queued_count(), 1);
        assert_eq!(gate.presented_count(), 0);
        assert!(!gate.ready(), "a queued flip must never be readiness");
        // A second queue changes nothing about readiness either.
        assert!(!gate.queued(63));
        assert!(!gate.ready());
        assert_eq!(gate.queued_count(), 2);
    }

    /// The first vblank of a queued flip is the readiness transition, and it is the only thing that
    /// disarms the bounded deadline.
    #[test]
    fn first_vblank_disarms_watchdog() {
        let mut gate = FirstFrameGate::new();
        gate.queued(63);
        assert!(
            gate.presented(63),
            "the first presented frame is reported once"
        );
        assert!(gate.ready());
        // A vblank with no flip queued for that CRTC is not a presented frame: it is not counted.
        assert!(!gate.presented(63), "later vblanks are not a transition");
        assert_eq!(gate.presented_count(), 1);
        // Even long past the budget a satisfied gate reports no failure: the deadline is off.
        assert_eq!(gate.failure(FIRST_FRAME_DEADLINE * 10), NO_FAILURE);
    }

    /// A vblank for a CRTC that has no flip in flight — another output, or a leftover commit of the
    /// previous DRM master — must not be mistaken for our first presented frame.
    #[test]
    fn wrong_crtc_vblank_does_not_satisfy_ready() {
        let mut gate = FirstFrameGate::new();
        gate.queued(63);
        assert!(!gate.presented_for(99));
        assert!(
            !gate.presented(99),
            "a foreign CRTC cannot present our frame"
        );
        assert!(!gate.ready());
        assert_eq!(gate.presented_count(), 0);
        // The queued CRTC is still the one that must answer, and the deadline still says so.
        assert_eq!(gate.queued_crtc(), Some(63));
        assert_eq!(gate.failure(FIRST_FRAME_DEADLINE), PAGEFLIP_TIMEOUT);
        // A vblank of the right CRTC, once it is queued, does satisfy it.
        assert!(gate.presented(63));
        assert!(gate.ready());
    }

    /// Nothing was ever queued: the deadline names that case instead of blaming a page flip that
    /// never existed. Inside the budget nothing is reported.
    #[test]
    fn no_first_queue_times_out() {
        let gate = FirstFrameGate::new();
        assert_eq!(gate.failure(Duration::ZERO), NO_FAILURE);
        assert_eq!(
            gate.failure(FIRST_FRAME_DEADLINE - Duration::from_millis(1)),
            NO_FAILURE
        );
        assert_eq!(gate.failure(FIRST_FRAME_DEADLINE), NO_FIRST_QUEUE);
        assert_eq!(gate.failure(Duration::from_secs(60)), NO_FIRST_QUEUE);
        let detail = gate.failure_detail(NO_FIRST_QUEUE, Duration::from_secs(8));
        assert!(
            detail.starts_with("no_first_queue queued=0 presented=0 crtc=none elapsed=8000ms"),
            "{detail}"
        );
    }

    /// The reported hardware case: a flip was queued and KMS never completed it. The deadline must
    /// name `pageflip_timeout` and carry the counters plus the last error, so the exit is diagnosable.
    #[test]
    fn missing_pageflip_timeout_and_exits() {
        let mut gate = FirstFrameGate::new();
        gate.queued(63);
        gate.note_error("queue_frame eDP-1: Buffer I/O error");
        assert_eq!(gate.failure(Duration::from_secs(8)), PAGEFLIP_TIMEOUT);
        assert!(!gate.ready(), "a timed-out gate is never ready");
        let detail = gate.failure_detail(PAGEFLIP_TIMEOUT, Duration::from_secs(8));
        // Reason and counters come first: the token field is capped, so the error truncates itself.
        assert!(
            detail.starts_with("pageflip_timeout queued=1 presented=0 crtc=63 elapsed=8000ms"),
            "{detail}"
        );
        assert!(
            detail.contains("last_render_error=queue_frame eDP-1: Buffer I/O error"),
            "{detail}"
        );
    }

    /// A pause between queue and vblank: the session — and with it the pending flip — is gone, so a
    /// vblank arriving while paused must not satisfy readiness, and the deadline it consumed says
    /// nothing about the resumed session.
    #[test]
    fn queued_pending_during_pause_does_not_false_ready() {
        let mut gate = FirstFrameGate::new();
        gate.queued(63);
        gate.pause();
        assert!(!gate.presented_for(63), "a paused session owns no CRTC");
        assert!(
            !gate.presented(63),
            "a paused session cannot present a frame"
        );
        assert!(!gate.ready());
        // Not a startup failure either: the session was taken away, not the GPU wedged.
        assert_eq!(gate.failure(Duration::from_secs(60)), NO_FAILURE);
        // After the resume the old flip is gone: a new queue and a new vblank are both required.
        gate.resume();
        assert_eq!(gate.queued_crtc(), None);
        assert!(!gate.presented(63));
        assert!(!gate.ready());
        assert_eq!(gate.failure(FIRST_FRAME_DEADLINE), PAGEFLIP_TIMEOUT);
        gate.queued(63);
        assert!(gate.presented(63));
        assert!(gate.ready());
    }

    /// The boundary ages are what the timing line reports; both are relative to backend init and must
    /// stay in that order.
    #[test]
    fn queue_and_present_ages_track_the_boundaries() {
        let mut gate = FirstFrameGate::new();
        assert_eq!(gate.queued_ms(), None);
        assert_eq!(gate.presented_ms(), None);
        gate.queued(63);
        let queued = gate.queued_ms().expect("the queue was recorded");
        assert_eq!(gate.presented_ms(), None);
        gate.presented(63);
        let presented = gate.presented_ms().expect("the presentation was recorded");
        assert!(presented >= queued, "presented={presented} queued={queued}");
    }

    /// The one-shot line a hardware log is read with: the reason and the counters must be the leading
    /// tokens of the detail, and a gate that was never queued says so with `crtc=none`.
    #[test]
    fn failure_detail_is_ordered_for_a_log_reader() {
        let gate = FirstFrameGate::with_deadline(Duration::from_millis(5));
        let detail = gate.failure_detail(NO_FIRST_QUEUE, Duration::from_millis(5));
        assert_eq!(
            detail,
            "no_first_queue queued=0 presented=0 crtc=none elapsed=5ms"
        );
        // Without an error the token is simply absent, never an empty `last_render_error=`.
        assert!(!detail.contains("last_render_error"));
    }
}
