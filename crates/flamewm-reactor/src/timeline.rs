//! One canonical, event-driven animation timeline.
//!
//! [`Timeline`] is pure data: every method takes an injected `now`, so no hidden clock is read.
//! It never touches the reactor. The owning event loop decides when to sample it; the WM loop
//! wakes every [`FRAME_INTERVAL`] while a transition is active.

use std::time::{Duration, Instant};

/// Caller-chosen key for a transition.
pub type TransitionId = u64;

/// Maximum number of concurrently active transitions.
pub const MAX_ACTIVE: usize = 16;
/// Frame interval (60 Hz) for the owning loop while transitions are active.
pub const FRAME_INTERVAL: Duration = Duration::from_millis(16);
/// Lowest accepted animation speed, in hundredths (0.25x).
pub const MIN_SPEED: u16 = 25;
/// Highest accepted animation speed, in hundredths (2.0x).
pub const MAX_SPEED: u16 = 200;

/// Easing curve applied to normalized time in `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Easing {
    #[default]
    EaseOutCubic,
    Smoothstep,
}

impl Easing {
    fn apply(self, t: f32) -> f32 {
        match self {
            Self::EaseOutCubic => 1.0 - (1.0 - t).powi(3),
            Self::Smoothstep => t * t * (3.0 - 2.0 * t),
        }
    }
}

/// Outcome of [`Timeline::start`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    /// No entry was created (animations disabled or capacity full); the caller applies the end
    /// state immediately.
    Immediate,
    /// A transition is active; the owning loop samples it with [`Timeline::progress`].
    Started,
}

#[derive(Debug, Clone, Copy)]
struct Transition<K> {
    kind: K,
    start: Instant,
    duration: Duration,
    easing: Easing,
    from: f32,
    to: f32,
}

impl<K> Transition<K> {
    fn value(&self, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.start);
        if self.duration.is_zero() || elapsed >= self.duration {
            return self.to;
        }
        let t = elapsed.as_secs_f32() / self.duration.as_secs_f32();
        (self.from + (self.to - self.from) * self.easing.apply(t)).clamp(0.0, 1.0)
    }

    fn remaining(&self, now: Instant) -> Duration {
        self.duration
            .saturating_sub(now.saturating_duration_since(self.start))
    }
}

/// Bounded set of keyed transitions with a global enable switch and speed factor.
#[derive(Debug, Clone)]
pub struct Timeline<K> {
    transitions: Vec<(TransitionId, Transition<K>)>,
    enabled: bool,
    speed: u16,
}

impl<K: Copy> Default for Timeline<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Copy> Timeline<K> {
    /// Creates an enabled timeline at 1.0x speed.
    #[must_use]
    pub fn new() -> Self {
        Self {
            transitions: Vec::with_capacity(MAX_ACTIVE),
            enabled: true,
            speed: 100,
        }
    }

    /// Enables or disables all animation. Disabling drops every active transition.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.transitions.clear();
        }
    }

    /// Sets the speed in hundredths (`100` = 1.0x), clamped to `MIN_SPEED..=MAX_SPEED`.
    pub fn set_speed(&mut self, hundredths: u16) {
        self.speed = hundredths.clamp(MIN_SPEED, MAX_SPEED);
    }

    /// Effective duration of a `base_ms` transition at the current speed.
    #[must_use]
    pub fn duration_for(&self, base_ms: u32) -> Duration {
        let micros = u64::from(base_ms) * 1_000 * 100 / u64::from(self.speed);
        Duration::from_micros(micros)
    }

    /// Starts a 0 -> 1 transition. If `id` is already active it restarts from its current
    /// progress, so a rapid retrigger never jumps.
    pub fn start(
        &mut self,
        id: TransitionId,
        kind: K,
        now: Instant,
        base_ms: u32,
        easing: Easing,
    ) -> Start {
        self.start_towards(id, kind, now, base_ms, easing, (0.0, 1.0))
    }

    /// Starts a transition toward `span.1`. A new `id` begins at `span.0`; an active `id`
    /// restarts from its current progress, so a retarget never jumps.
    pub fn start_towards(
        &mut self,
        id: TransitionId,
        kind: K,
        now: Instant,
        base_ms: u32,
        easing: Easing,
        span: (f32, f32),
    ) -> Start {
        if !self.enabled {
            return Start::Immediate;
        }
        let index = self.index_of(id);
        if index.is_none() && self.transitions.len() >= MAX_ACTIVE {
            return Start::Immediate;
        }
        let from = index.map_or(span.0, |i| self.transitions[i].1.value(now));
        let transition = Transition {
            kind,
            start: now,
            duration: self.duration_for(base_ms),
            easing,
            from,
            to: span.1,
        };
        match index {
            Some(i) => self.transitions[i].1 = transition,
            None => self.transitions.push((id, transition)),
        }
        Start::Started
    }

    /// Reverses an active transition from its current progress without a jump.
    /// Returns `false` when `id` is not active.
    pub fn reverse(&mut self, id: TransitionId, now: Instant) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };
        let transition = &mut self.transitions[index].1;
        transition.from = transition.value(now);
        transition.to = 1.0 - transition.to;
        transition.start = now;
        true
    }

    /// Current progress in `[0, 1]`, or `None` when `id` is not active.
    #[must_use]
    pub fn progress(&self, id: TransitionId, now: Instant) -> Option<f32> {
        self.index_of(id)
            .map(|i| self.transitions[i].1.value(now))
    }

    /// Kind of the active transition `id`, or `None` when it is not active.
    #[must_use]
    pub fn kind(&self, id: TransitionId) -> Option<K> {
        self.index_of(id).map(|i| self.transitions[i].1.kind)
    }

    /// Removes `id` without applying its end state. Returns whether it was active.
    pub fn cancel(&mut self, id: TransitionId) -> bool {
        self.index_of(id)
            .map(|i| self.transitions.swap_remove(i))
            .is_some()
    }

    /// Removes `id` and returns its target value (`1.0` for forward, `0.0` after a reverse), so
    /// the caller can settle the end state. Returns `None` when `id` is not active.
    pub fn finish(&mut self, id: TransitionId) -> Option<f32> {
        self.index_of(id)
            .map(|i| self.transitions.swap_remove(i).1.to)
    }

    /// Whether `id` has reached its end at `now`; `None` when `id` is not active.
    #[must_use]
    pub fn finished(&self, id: TransitionId, now: Instant) -> Option<bool> {
        self.index_of(id)
            .map(|i| self.transitions[i].1.remaining(now).is_zero())
    }

    /// Drops transitions that have reached their end at `now`.
    pub fn prune(&mut self, now: Instant) {
        self.transitions
            .retain(|(_, t)| now.saturating_duration_since(t.start) < t.duration);
    }

    /// Time until the earliest active transition completes, or `None` when empty.
    #[must_use]
    pub fn next_deadline(&self, now: Instant) -> Option<Duration> {
        self.transitions
            .iter()
            .map(|(_, t)| t.remaining(now))
            .min()
    }

    /// Number of active transitions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.transitions.len()
    }

    /// Whether no transitions are active.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.transitions.is_empty()
    }

    fn index_of(&self, id: TransitionId) -> Option<usize> {
        self.transitions.iter().position(|(key, _)| *key == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Kind {
        Open,
        Close,
    }

    fn base() -> Instant {
        Instant::now()
    }

    #[test]
    fn progress_endpoints_are_exact() {
        let t0 = base();
        let mut tl = Timeline::new();
        assert_eq!(
            tl.start(1, Kind::Open, t0, 200, Easing::EaseOutCubic),
            Start::Started
        );
        assert_eq!(tl.progress(1, t0), Some(0.0));
        assert_eq!(tl.progress(1, t0 + Duration::from_millis(200)), Some(1.0));
        assert_eq!(tl.progress(1, t0 + Duration::from_secs(5)), Some(1.0));
        let mid = tl.progress(1, t0 + Duration::from_millis(100)).unwrap();
        assert!(mid > 0.0 && mid < 1.0);
        // Ease-out cubic is front-loaded: halfway in time is already above half progress.
        assert!(mid > 0.5);
    }

    #[test]
    fn smoothstep_is_symmetric_at_midpoint() {
        assert_eq!(Easing::Smoothstep.apply(0.0), 0.0);
        assert_eq!(Easing::Smoothstep.apply(1.0), 1.0);
        assert!((Easing::Smoothstep.apply(0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn speed_two_x_halves_duration_and_clamps() {
        let t0 = base();
        let mut tl = Timeline::new();
        tl.set_speed(200);
        assert_eq!(tl.duration_for(200), Duration::from_millis(100));
        tl.start(1, Kind::Open, t0, 200, Easing::Smoothstep);
        assert_eq!(tl.progress(1, t0 + Duration::from_millis(100)), Some(1.0));
        assert!(tl.progress(1, t0 + Duration::from_millis(99)).unwrap() < 1.0);

        tl.set_speed(10);
        assert_eq!(tl.duration_for(100), Duration::from_millis(400));
        tl.set_speed(500);
        assert_eq!(tl.duration_for(100), Duration::from_millis(50));
    }

    #[test]
    fn disabled_creates_no_entry_and_no_deadline() {
        let t0 = base();
        let mut tl: Timeline<Kind> = Timeline::new();
        tl.set_enabled(false);
        assert_eq!(
            tl.start(1, Kind::Open, t0, 200, Easing::EaseOutCubic),
            Start::Immediate
        );
        assert!(tl.is_empty());
        assert_eq!(tl.next_deadline(t0), None);
    }

    #[test]
    fn disabling_drops_active_transitions() {
        let t0 = base();
        let mut tl = Timeline::new();
        tl.start(1, Kind::Open, t0, 200, Easing::EaseOutCubic);
        tl.set_enabled(false);
        assert!(tl.is_empty());
    }

    #[test]
    fn reverse_is_continuous_from_current_progress() {
        let t0 = base();
        let mut tl = Timeline::new();
        tl.start(7, Kind::Open, t0, 200, Easing::EaseOutCubic);
        let at = t0 + Duration::from_millis(60);
        let before = tl.progress(7, at).unwrap();
        assert!(tl.reverse(7, at));
        assert_eq!(tl.progress(7, at), Some(before));
        assert_eq!(tl.progress(7, at + Duration::from_millis(200)), Some(0.0));
        assert!(!tl.reverse(99, at));
    }

    #[test]
    fn restart_on_same_id_does_not_jump() {
        let t0 = base();
        let mut tl = Timeline::new();
        tl.start(3, Kind::Open, t0, 200, Easing::EaseOutCubic);
        let at = t0 + Duration::from_millis(100);
        let before = tl.progress(3, at).unwrap();
        assert_eq!(
            tl.start(3, Kind::Close, at, 200, Easing::EaseOutCubic),
            Start::Started
        );
        assert_eq!(tl.len(), 1);
        assert_eq!(tl.progress(3, at), Some(before));
        assert_eq!(tl.kind(3), Some(Kind::Close));
    }

    #[test]
    fn cancel_and_finish_remove_entries() {
        let t0 = base();
        let mut tl = Timeline::new();
        tl.start(1, Kind::Open, t0, 200, Easing::EaseOutCubic);
        tl.start(2, Kind::Close, t0, 200, Easing::EaseOutCubic);
        assert!(tl.cancel(1));
        assert!(!tl.cancel(1));
        assert_eq!(tl.finish(2), Some(1.0));
        assert_eq!(tl.finish(2), None);
        assert!(tl.is_empty());
    }

    #[test]
    fn capacity_is_bounded_and_replacement_still_works() {
        let t0 = base();
        let mut tl = Timeline::new();
        for id in 0..MAX_ACTIVE as u64 {
            assert_eq!(
                tl.start(id, Kind::Open, t0, 200, Easing::EaseOutCubic),
                Start::Started
            );
        }
        assert_eq!(
            tl.start(999, Kind::Open, t0, 200, Easing::EaseOutCubic),
            Start::Immediate
        );
        assert_eq!(tl.len(), MAX_ACTIVE);
        assert_eq!(
            tl.start(0, Kind::Close, t0, 200, Easing::EaseOutCubic),
            Start::Started
        );
        assert_eq!(tl.len(), MAX_ACTIVE);
    }

    #[test]
    fn deadline_is_none_when_empty_and_shrinks_with_time() {
        let t0 = base();
        let mut tl = Timeline::new();
        assert_eq!(tl.next_deadline(t0), None);
        tl.start(1, Kind::Open, t0, 200, Easing::EaseOutCubic);
        assert_eq!(
            tl.next_deadline(t0 + Duration::from_millis(50)),
            Some(Duration::from_millis(150))
        );
        tl.prune(t0 + Duration::from_millis(200));
        assert_eq!(tl.next_deadline(t0 + Duration::from_millis(200)), None);
    }

    #[test]
    fn start_towards_reverses_from_current_progress_without_jump() {
        let t0 = base();
        let mut tl = Timeline::new();
        tl.start_towards(7, Kind::Open, t0, 200, Easing::EaseOutCubic, (0.0, 1.0));
        let mid = t0 + Duration::from_millis(100);
        let before = tl.progress(7, mid).unwrap();
        assert_eq!(
            tl.start_towards(7, Kind::Close, mid, 120, Easing::EaseOutCubic, (1.0, 0.0)),
            Start::Started
        );
        assert!((tl.progress(7, mid).unwrap() - before).abs() < 1e-6, "no jump");
        assert_eq!(tl.kind(7), Some(Kind::Close));
        assert_eq!(tl.progress(7, mid + Duration::from_millis(120)), Some(0.0));
    }

    #[test]
    fn new_start_towards_begins_at_absent_from_value() {
        let t0 = base();
        let mut tl = Timeline::new();
        tl.start_towards(3, Kind::Close, t0, 120, Easing::EaseOutCubic, (1.0, 0.0));
        assert_eq!(tl.progress(3, t0), Some(1.0));
        assert_eq!(tl.progress(3, t0 + Duration::from_millis(120)), Some(0.0));
    }

    #[test]
    fn finished_reports_end_only_after_duration() {
        let t0 = base();
        let mut tl = Timeline::new();
        assert_eq!(tl.finished(1, t0), None);
        tl.start(1, Kind::Open, t0, 180, Easing::EaseOutCubic);
        assert_eq!(tl.finished(1, t0 + Duration::from_millis(179)), Some(false));
        assert_eq!(tl.finished(1, t0 + Duration::from_millis(180)), Some(true));
    }

    #[test]
    fn disabled_or_full_start_towards_is_immediate() {
        let t0 = base();
        let mut tl = Timeline::<Kind>::new();
        tl.set_enabled(false);
        assert_eq!(
            tl.start_towards(1, Kind::Open, t0, 180, Easing::EaseOutCubic, (0.0, 1.0)),
            Start::Immediate
        );
        assert!(tl.is_empty());
    }
}
