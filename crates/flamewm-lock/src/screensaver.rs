//! Lock-screen idle state machine: Visible, then Ribbons (animated) or straight to
//! `DisplayOff` (mode `none`), then `DisplayOff`. Pure: no clock, X, or DPMS access.
//!
//! Timing: a `0` value means "never". `screensaverAfterLockMinutes` counts idle time
//! since the last input; `screensaverDisplayOffMinutes` counts time spent in Ribbons.
//! The first input while the screen is not Visible wakes it and is consumed.
use std::time::Duration;

use crate::settings::SaverConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Visible,
    RibbonsActive,
    DisplayOff,
}

#[derive(Debug, Clone)]
pub struct Screensaver {
    config: SaverConfig,
    phase: Phase,
    idle: Duration,
    active: Duration,
}

impl Screensaver {
    #[must_use]
    pub fn new(config: SaverConfig) -> Self {
        Self {
            config,
            phase: Phase::Visible,
            idle: Duration::ZERO,
            active: Duration::ZERO,
        }
    }

    #[must_use]
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Advances the idle clock by `elapsed`. Returns the new phase when it changed.
    pub fn tick(&mut self, elapsed: Duration) -> Option<Phase> {
        match self.phase {
            Phase::Visible => {
                self.idle += elapsed;
                let after = minutes(self.config.after_lock_minutes)?;
                if self.idle < after {
                    return None;
                }
                self.active = Duration::ZERO;
                self.enter(if self.config.ribbons() {
                    Phase::RibbonsActive
                } else {
                    Phase::DisplayOff
                })
            }
            Phase::RibbonsActive => {
                self.active += elapsed;
                let off = minutes(self.config.display_off_minutes)?;
                if self.active < off {
                    return None;
                }
                self.enter(Phase::DisplayOff)
            }
            Phase::DisplayOff => None,
        }
    }

    /// Records one input. Returns `true` when the input woke the screen and must
    /// not reach the lock UI; `false` when the input is an ordinary input.
    pub fn input(&mut self) -> bool {
        let woke = self.phase != Phase::Visible;
        self.idle = Duration::ZERO;
        self.active = Duration::ZERO;
        self.phase = Phase::Visible;
        woke
    }

    fn enter(&mut self, phase: Phase) -> Option<Phase> {
        self.phase = phase;
        Some(phase)
    }
}

/// `None` for the "never" value 0.
fn minutes(value: u32) -> Option<Duration> {
    (value > 0).then(|| Duration::from_secs(u64::from(value) * 60))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::SaverMode;

    fn config(mode: SaverMode, after: u32, off: u32) -> SaverConfig {
        SaverConfig {
            mode,
            after_lock_minutes: after,
            display_off_minutes: off,
            ..SaverConfig::default()
        }
    }

    fn minute() -> Duration {
        Duration::from_secs(60)
    }

    #[test]
    fn ribbons_start_after_idle_minutes_not_before() {
        let mut saver = Screensaver::new(config(SaverMode::Ribbons, 2, 0));
        assert_eq!(saver.tick(minute()), None);
        assert_eq!(
            saver.tick(Duration::from_secs(59)),
            None,
            "119 s is still under two minutes"
        );
        assert_eq!(
            saver.tick(Duration::from_secs(1)),
            Some(Phase::RibbonsActive)
        );
        assert_eq!(saver.phase(), Phase::RibbonsActive);
    }

    #[test]
    fn ribbons_then_display_off_after_its_own_timer() {
        let mut saver = Screensaver::new(config(SaverMode::Ribbons, 1, 3));
        assert_eq!(saver.tick(minute()), Some(Phase::RibbonsActive));
        assert_eq!(saver.tick(minute() * 2), None);
        assert_eq!(saver.tick(minute()), Some(Phase::DisplayOff));
        assert_eq!(
            saver.tick(minute() * 10),
            None,
            "DisplayOff is terminal until input"
        );
    }

    #[test]
    fn mode_none_goes_straight_to_display_off() {
        let mut saver = Screensaver::new(config(SaverMode::None, 1, 5));
        assert_eq!(saver.tick(minute()), Some(Phase::DisplayOff));
    }

    #[test]
    fn zero_after_lock_means_never() {
        for mode in [SaverMode::Ribbons, SaverMode::None] {
            let mut saver = Screensaver::new(config(mode, 0, 1));
            assert_eq!(saver.tick(Duration::from_secs(10_000)), None);
            assert_eq!(saver.phase(), Phase::Visible);
        }
    }

    #[test]
    fn zero_display_off_means_ribbons_run_forever() {
        let mut saver = Screensaver::new(config(SaverMode::Ribbons, 1, 0));
        assert_eq!(saver.tick(minute()), Some(Phase::RibbonsActive));
        assert_eq!(saver.tick(minute() * 1000), None);
        assert_eq!(saver.phase(), Phase::RibbonsActive);
    }

    #[test]
    fn first_wake_input_is_consumed_and_returns_to_visible() {
        let mut saver = Screensaver::new(config(SaverMode::Ribbons, 1, 0));
        saver.tick(minute());
        assert!(saver.input(), "wake input must be consumed");
        assert_eq!(saver.phase(), Phase::Visible);
        assert!(!saver.input(), "the next input is an ordinary one");
    }

    #[test]
    fn wake_from_display_off_is_consumed() {
        let mut saver = Screensaver::new(config(SaverMode::None, 1, 0));
        saver.tick(minute());
        assert!(saver.input());
        assert_eq!(saver.phase(), Phase::Visible);
    }

    #[test]
    fn visible_input_passes_and_resets_idle() {
        let mut saver = Screensaver::new(config(SaverMode::Ribbons, 2, 0));
        saver.tick(minute());
        assert!(!saver.input());
        assert_eq!(saver.tick(minute()), None, "idle restarted by the input");
        assert_eq!(saver.tick(minute()), Some(Phase::RibbonsActive));
    }

    #[test]
    fn wake_restarts_the_ribbons_timer() {
        let mut saver = Screensaver::new(config(SaverMode::Ribbons, 1, 2));
        saver.tick(minute());
        saver.input();
        assert_eq!(saver.tick(minute()), Some(Phase::RibbonsActive));
        assert_eq!(saver.tick(Duration::from_secs(119)), None);
        assert_eq!(saver.tick(Duration::from_secs(1)), Some(Phase::DisplayOff));
    }
}
