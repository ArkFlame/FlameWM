//! Pure timing for the terminal scrollback glide. The wheel starts one; the UI loop steps it once per frame while
//! the animation runs. No clock read, no allocation: callers pass `Instant`s in.
use std::time::{Duration, Instant};

/// Length of one glide. Every wheel step restarts it from the displayed offset, so a burst settles this long after
/// the last step.
pub const DURATION: Duration = Duration::from_millis(70);
/// Interval between glide frames. The ticker sleeps only while a frame is requested, so an idle terminal has none.
pub const FRAME: Duration = Duration::from_millis(16);

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// Integer scrollback offset easing toward a target. Copy-only state, so a frame can read it before borrowing a pane.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScrollGlide {
    pane: u64,
    from: f32,
    to: f32,
    applied: usize,
    start: Option<Instant>,
}

impl ScrollGlide {
    /// Starts a glide of `pane` from `from` to `to`. A same-value glide settles without moving anything.
    pub fn begin(&mut self, pane: u64, from: usize, to: usize, now: Instant) {
        self.pane = pane;
        self.from = from as f32;
        self.to = to as f32;
        self.applied = from;
        self.start = Some(now);
    }

    /// Whether a glide has been started and has not settled yet.
    #[must_use]
    pub fn is_animating(&self, now: Instant) -> bool {
        self.start
            .is_some_and(|start| now.saturating_duration_since(start) < DURATION)
    }

    /// Whether a glide has a start time, finished or not. A finished glide still has to be settled once.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.start.is_some()
    }

    #[must_use]
    pub fn applied(&self) -> usize {
        self.applied
    }

    #[must_use]
    pub fn pane(&self) -> u64 {
        self.pane
    }

    #[must_use]
    pub fn target(&self) -> usize {
        self.to as usize
    }

    /// Offset at `now`, eased. Exactly `to` once the duration has passed and `applied` when no glide runs.
    #[must_use]
    pub fn value(&self, now: Instant) -> f32 {
        let Some(start) = self.start else {
            return self.applied as f32;
        };
        let t = now.saturating_duration_since(start).as_secs_f32() / DURATION.as_secs_f32();
        if t >= 1.0 {
            self.to
        } else {
            self.from + (self.to - self.from) * ease_out_cubic(t)
        }
    }

    /// Steps the applied integer offset. Returns the new offset when it changed, so the caller writes the pane only
    /// when a frame visibly moved. A finished glide snaps exactly to the target and clears its start.
    pub fn advance(&mut self, now: Instant) -> Option<usize> {
        let Some(start) = self.start else {
            return None;
        };
        let value = self.value(now);
        let lo = self.from.min(self.to) as i64;
        let hi = self.from.max(self.to) as i64;
        // Rounding cannot overshoot the target: the eased value stays inside the endpoints.
        let next = (value.round() as i64).clamp(lo, hi) as usize;
        if now.saturating_duration_since(start) >= DURATION {
            self.start = None;
        }
        if next != self.applied {
            self.applied = next;
            Some(next)
        } else {
            None
        }
    }

    pub fn cancel(&mut self) {
        self.start = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn a_glide_is_monotonic_and_settles_exactly_on_the_target() {
        let start = t0();
        let mut glide = ScrollGlide::default();
        glide.begin(7, 10, 20, start);
        let mut last = glide.applied();
        for step in 0..=2u32 {
            let now = start + FRAME * step;
            if let Some(value) = glide.advance(now) {
                assert!(value >= last, "the offset never moves backwards");
                last = value;
            }
        }
        assert_eq!(glide.advance(start + DURATION), Some(20));
        assert_eq!(glide.applied(), 20);
        assert_eq!(glide.value(start + DURATION), 20.0);
        assert!(!glide.is_animating(start + DURATION));
        assert!(!glide.is_active());
    }

    #[test]
    fn a_reverse_glide_is_monotonic_downwards() {
        let start = t0();
        let mut glide = ScrollGlide::default();
        glide.begin(1, 20, 10, start);
        let mut last = glide.applied();
        for step in 0..=2u32 {
            if let Some(value) = glide.advance(start + FRAME * step) {
                assert!(value <= last, "the offset never moves forwards");
                last = value;
            }
        }
        assert_eq!(glide.advance(start + DURATION), Some(10));
    }

    #[test]
    fn an_equal_target_neither_moves_nor_panics() {
        let start = t0();
        let mut glide = ScrollGlide::default();
        glide.begin(3, 5, 5, start);
        assert_eq!(glide.advance(start + FRAME), None);
        assert_eq!(glide.value(start + FRAME), 5.0);
        assert_eq!(glide.advance(start + DURATION), None);
        assert!(!glide.is_animating(start + DURATION));
        assert!(!glide.is_active());
    }
}
