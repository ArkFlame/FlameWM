//! Shake-to-locate: detects a fast left-right shake of the pointer from horizontal motion samples.
//!
//! Pure detector. No timers and no allocation: the caller feeds each motion sample with its timestamp
//! and reads the boolean result. A shake is [`SHAKE_REVERSALS`] direction reversals, each preceded by
//! a swing of at least [`MIN_SWING_PX`] pixels, completed within [`WINDOW_MS`] milliseconds. The
//! detector then resets, so one shake reports once.

/// Direction reversals needed for a shake.
pub const SHAKE_REVERSALS: u8 = 4;
/// Minimum accumulated horizontal travel in one direction for that swing to count.
pub const MIN_SWING_PX: u32 = 40;
/// Window, in milliseconds, from the first counted swing to the last reversal.
pub const WINDOW_MS: u32 = 500;

/// Horizontal shake detector. Default is empty.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShakeDetector {
    /// Sign of the current swing: `1`, `-1`, or `0` before the first non-zero sample.
    dir: i8,
    /// Time of the first sample in the current swing.
    swing_start: u32,
    /// Accumulated travel in the current swing.
    swing_px: u32,
    /// Counted reversals in the current window.
    reversals: u8,
    /// Swing start that opened the current window.
    window_start: u32,
}

impl ShakeDetector {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            dir: 0,
            swing_start: 0,
            swing_px: 0,
            reversals: 0,
            window_start: 0,
        }
    }

    /// Feeds one horizontal motion sample. Returns `true` exactly when this sample completes a shake.
    /// Zero-distance samples are ignored.
    pub fn feed(&mut self, t_ms: u32, dx: i32) -> bool {
        if dx == 0 {
            return false;
        }
        let dir: i8 = if dx > 0 { 1 } else { -1 };
        if self.dir == 0 {
            self.start_swing(t_ms, dir, dx);
            return false;
        }
        if dir == self.dir {
            self.swing_px = self.swing_px.saturating_add(dx.unsigned_abs());
            return false;
        }

        // Direction flipped: the finished swing may count as a reversal.
        let swing_start = self.swing_start;
        let swing_px = self.swing_px;
        self.start_swing(t_ms, dir, dx);
        if swing_px < MIN_SWING_PX {
            return false;
        }
        if self.reversals == 0 || t_ms.wrapping_sub(self.window_start) > WINDOW_MS {
            self.window_start = swing_start;
            self.reversals = 0;
        }
        self.reversals += 1;
        if self.reversals >= SHAKE_REVERSALS {
            *self = Self::new();
            return true;
        }
        false
    }

    fn start_swing(&mut self, t_ms: u32, dir: i8, dx: i32) {
        self.dir = dir;
        self.swing_start = t_ms;
        self.swing_px = dx.unsigned_abs();
    }
}

/// How long the cursor stays enlarged after a shake, in milliseconds.
pub const ENLARGE_MS: u64 = 1000;
/// Upper bound for the enlarged nominal cursor size, in pixels.
pub const MAX_ENLARGED_CURSOR_PX: u32 = 128;

/// Result of one motion sample on [`ShakeLocate`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShakeEvent {
    /// No shake completed, or the sample was ignored.
    Idle,
    /// A shake started an enlarge: the caller applies the enlarged cursor size once.
    Enlarged,
    /// A shake while already enlarged: the caller does nothing, the window restarts.
    Extended,
}

/// Enlarge/restore state for shake-to-locate. Pure: the caller supplies the motion sample and a
/// monotonic clock in milliseconds. It owns no timer; [`ShakeLocate::deadline_ms`] tells the
/// caller when to wake, and the restore happens only when [`ShakeLocate::restore_due`] says so.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShakeLocate {
    detector: ShakeDetector,
    /// Restore deadline while enlarged; `None` while the nominal size is in effect.
    restore_at_ms: Option<u64>,
}

impl ShakeLocate {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            detector: ShakeDetector::new(),
            restore_at_ms: None,
        }
    }

    /// Feeds one horizontal motion sample at server time `t_ms`, with `now_ms` on the caller's clock.
    pub fn feed_motion(&mut self, t_ms: u32, dx: i32, now_ms: u64) -> ShakeEvent {
        if !self.detector.feed(t_ms, dx) {
            return ShakeEvent::Idle;
        }
        let was_enlarged = self.restore_at_ms.is_some();
        self.restore_at_ms = Some(now_ms.saturating_add(ENLARGE_MS));
        if was_enlarged {
            ShakeEvent::Extended
        } else {
            ShakeEvent::Enlarged
        }
    }

    /// Wake-up for the pending restore, on the caller's clock. `None` while nominal.
    #[must_use]
    pub const fn deadline_ms(&self) -> Option<u64> {
        self.restore_at_ms
    }

    /// True when the caller must restore the nominal cursor size now. Also true when the feature
    /// was switched off while enlarged. Clears the state; a disabled feature keeps no detector
    /// history.
    pub fn restore_due(&mut self, now_ms: u64, enabled: bool) -> bool {
        if !enabled {
            let was_enlarged = self.restore_at_ms.is_some();
            *self = Self::new();
            return was_enlarged;
        }
        match self.restore_at_ms {
            Some(at) if now_ms >= at => {
                self.restore_at_ms = None;
                true
            }
            _ => false,
        }
    }
}

/// Enlarged cursor size for a nominal size: twice nominal, capped, never below nominal.
#[must_use]
pub fn enlarged_cursor_size(nominal: u32) -> u32 {
    nominal
        .saturating_mul(2)
        .min(MAX_ENLARGED_CURSOR_PX)
        .max(nominal)
}

#[cfg(test)]
mod locate_tests {
    use super::*;

    /// Shake samples from the detector tests: four reversals of 40 px inside 300 ms.
    const SHAKE: [(u32, i32); 5] = [(0, 40), (75, -40), (150, 40), (225, -40), (300, 40)];

    fn shake_at(state: &mut ShakeLocate, base_ms: u64) -> ShakeEvent {
        let mut last = ShakeEvent::Idle;
        for (t, dx) in SHAKE {
            last = state.feed_motion(t, dx, base_ms + u64::from(t));
        }
        last
    }

    #[test]
    fn shake_enlarges_once_then_extends_while_enlarged() {
        let mut state = ShakeLocate::new();
        assert_eq!(shake_at(&mut state, 0), ShakeEvent::Enlarged);
        assert_eq!(state.deadline_ms(), Some(300 + ENLARGE_MS));
        assert_eq!(shake_at(&mut state, 400), ShakeEvent::Extended);
        assert_eq!(state.deadline_ms(), Some(700 + ENLARGE_MS));
    }

    #[test]
    fn restore_fires_exactly_at_the_deadline() {
        let mut state = ShakeLocate::new();
        shake_at(&mut state, 0);
        let deadline = state.deadline_ms().expect("enlarged");
        assert!(!state.restore_due(deadline - 1, true));
        assert!(state.restore_due(deadline, true));
        assert_eq!(state.deadline_ms(), None, "no timer after restore");
        assert!(!state.restore_due(deadline + 5_000, true), "restores once");
    }

    #[test]
    fn idle_state_has_no_deadline_and_never_restores() {
        let mut state = ShakeLocate::new();
        assert_eq!(state.deadline_ms(), None);
        assert!(!state.restore_due(u64::MAX, true));
        assert_eq!(state.feed_motion(0, 3, 0), ShakeEvent::Idle);
    }

    #[test]
    fn switching_off_while_enlarged_requests_restore_and_clears_history() {
        let mut state = ShakeLocate::new();
        shake_at(&mut state, 0);
        assert!(state.restore_due(10, false), "restore on disable");
        assert_eq!(state, ShakeLocate::new());
        assert!(!state.restore_due(10, false), "nothing left to restore");
    }

    #[test]
    fn disabled_feature_keeps_no_detector_history() {
        let mut state = ShakeLocate::new();
        for &(t, dx) in &SHAKE[..4] {
            state.feed_motion(t, dx, u64::from(t));
        }
        assert!(!state.restore_due(0, false));
        assert_eq!(state, ShakeLocate::new(), "partial shake forgotten");
    }

    #[test]
    fn enlarged_size_doubles_nominal_and_caps() {
        assert_eq!(enlarged_cursor_size(24), 48);
        assert_eq!(enlarged_cursor_size(32), 64);
        assert_eq!(enlarged_cursor_size(64), 128);
        assert_eq!(enlarged_cursor_size(100), 128);
        assert_eq!(enlarged_cursor_size(0), 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds `(t, dx)` samples and returns each result, so tests can assert the exact true/false pattern.
    fn run(samples: &[(u32, i32)]) -> Vec<bool> {
        let mut detector = ShakeDetector::new();
        samples
            .iter()
            .map(|&(t, dx)| detector.feed(t, dx))
            .collect()
    }

    #[test]
    fn shake_four_reversals_of_40px_within_500ms_fires_once() {
        let samples = [(0, 40), (75, -40), (150, 40), (225, -40), (300, 40)];
        let fired = run(&samples);
        assert_eq!(fired.iter().filter(|&&hit| hit).count(), 1, "{fired:?}");
        assert_eq!(fired, [false, false, false, false, true]);
    }

    #[test]
    fn shake_swings_of_39px_do_not_fire() {
        let samples = [(0, 39), (120, -39), (240, 39), (360, -39), (480, 39)];
        assert!(run(&samples).iter().all(|&hit| !hit));
    }

    #[test]
    fn shake_window_boundary_is_500ms_inclusive() {
        let inside = [(0, 40), (125, -40), (250, 40), (375, -40), (500, 40)];
        assert!(run(&inside)[4], "span of exactly 500 ms fires");
        let outside = [(0, 40), (125, -40), (250, 40), (375, -40), (501, 40)];
        assert!(
            run(&outside).iter().all(|&hit| !hit),
            "501 ms does not fire"
        );
    }

    #[test]
    fn shake_slow_back_and_forth_does_not_fire() {
        let samples: Vec<(u32, i32)> = (0..10)
            .map(|i| (i * 800, if i % 2 == 0 { 60 } else { -60 }))
            .collect();
        assert!(run(&samples).iter().all(|&hit| !hit));
    }

    #[test]
    fn shake_small_jitter_does_not_fire() {
        let samples: Vec<(u32, i32)> = (0..40)
            .map(|i| (i * 10, if i % 2 == 0 { 5 } else { -5 }))
            .collect();
        assert!(run(&samples).iter().all(|&hit| !hit));
    }

    #[test]
    fn shake_zero_motion_is_ignored() {
        let mut detector = ShakeDetector::new();
        assert!(!detector.feed(0, 0));
        assert_eq!(detector, ShakeDetector::new());
    }

    #[test]
    fn shake_resets_after_firing_so_next_shake_needs_fresh_reversals() {
        let mut detector = ShakeDetector::new();
        for (t, dx) in [(0, 40), (75, -40), (150, 40), (225, -40), (300, 40)] {
            detector.feed(t, dx);
        }
        assert!(
            !detector.feed(400, -60),
            "one reversal after a fire does not fire"
        );
    }

    #[test]
    fn shake_window_expires_across_slow_reversals() {
        // Four reversals spanning 800 ms: the first lies outside the 500 ms window of the last.
        let samples = [(0, 40), (200, -40), (400, 40), (600, -40), (800, 40)];
        assert!(run(&samples).iter().all(|&hit| !hit));
    }
}
