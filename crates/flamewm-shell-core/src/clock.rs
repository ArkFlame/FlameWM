#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockFormat {
    pub time: &'static str,
    pub date: &'static str,
}
impl Default for ClockFormat {
    fn default() -> Self {
        Self {
            time: "%H:%M",
            date: "%-d/%-m/%y",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockTimerPlan {
    pub interval_ms: u64,
    pub initial_delay_ms: u64,
}
#[must_use]
pub fn clock_timer_plan(format: &str, now_epoch_ms: u64) -> ClockTimerPlan {
    let with_seconds = clock_shows_seconds(format);
    let interval_ms = if with_seconds { 1_000 } else { 60_000 };
    ClockTimerPlan {
        interval_ms,
        initial_delay_ms: wall_clock_delay_ms(now_epoch_ms, with_seconds),
    }
}
#[must_use]
pub fn clock_shows_seconds(format: &str) -> bool {
    ["%S", "%T", "%X", "%r"]
        .iter()
        .any(|token| format.contains(token))
}
/// Milliseconds until the next second (`with_seconds == true`) or minute
/// boundary after `now_epoch_ms`. Mirrors the `flamewm-reactor` wall-clock
/// primitive so shell-core stays dependency-free; recompute after every tick
/// so suspend/resume jumps realign instead of drifting. Returns the full
/// interval when exactly on a boundary so the timer always advances.
#[must_use]
pub fn wall_clock_delay_ms(now_epoch_ms: u64, with_seconds: bool) -> u64 {
    let interval_ms = if with_seconds { 1_000 } else { 60_000 };
    let remainder = now_epoch_ms % interval_ms;
    if remainder == 0 {
        interval_ms
    } else {
        interval_ms - remainder
    }
}
/// Minimal wall-clock consumer: remembers the last projected local date and
/// reports when the calendar must be rebuilt. The reactor fires the tick;
/// this struct decides date/calendar refresh on local-date change only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockDateTracker {
    last_date: Option<(i32, u8, u8)>,
}
impl ClockDateTracker {
    #[must_use]
    pub fn new(initial_date: (i32, u8, u8)) -> Self {
        Self {
            last_date: Some(initial_date),
        }
    }
    /// Returns `true` when `today` differs from the last seen local date and
    /// stores it. Midnight/DST/suspend jumps surface as a single change.
    pub fn date_changed(&mut self, today: (i32, u8, u8)) -> bool {
        if self.last_date == Some(today) {
            false
        } else {
            self.last_date = Some(today);
            true
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    struct FakeClock {
        now_ms: std::cell::Cell<u64>,
    }

    impl FakeClock {
        fn new(now_ms: u64) -> Self {
            Self {
                now_ms: std::cell::Cell::new(now_ms),
            }
        }

        fn advance(&self, delta_ms: u64) {
            self.now_ms.set(self.now_ms.get() + delta_ms);
        }

        fn now_epoch_ms(&self) -> u64 {
            self.now_ms.get()
        }
    }

    #[test]
    fn wall_clock_delay_aligns_to_live_boundaries() {
        assert_eq!(wall_clock_delay_ms(61_234, false), 58_766);
        assert_eq!(wall_clock_delay_ms(1_250, true), 750);
        assert_eq!(wall_clock_delay_ms(60_000, false), 60_000);
        assert_eq!(wall_clock_delay_ms(1_000, true), 1_000);
    }

    #[test]
    fn wall_clock_delay_realigns_after_suspend_jump_without_sleep() {
        // No `Thread::sleep` anywhere: the fake clock jumps instantly.
        let clock = FakeClock::new(61_234);
        assert_eq!(wall_clock_delay_ms(clock.now_epoch_ms(), false), 58_766);
        clock.advance(3_630_000);
        assert_eq!(wall_clock_delay_ms(clock.now_epoch_ms(), false), 28_766);
    }

    #[test]
    fn date_tracker_fires_once_per_local_date_change() {
        let mut tracker = ClockDateTracker::new((2026, 9, 5));
        assert!(!tracker.date_changed((2026, 9, 5)));
        assert!(tracker.date_changed((2026, 9, 6)));
        assert!(!tracker.date_changed((2026, 9, 6)));
        // Suspend jump across several days surfaces as a single change.
        assert!(tracker.date_changed((2026, 9, 9)));
        assert!(!tracker.date_changed((2026, 9, 9)));
    }
}
