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
