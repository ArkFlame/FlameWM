//! Debounce for udev change events on the primary GPU.
//!
//! One connector hotplug makes the kernel send several `change` uevents in a burst. The first event
//! of a burst arms one settle timer; the rest are absorbed until it fires, and that single timer
//! rescans the connectors once. A later event (after the timer fired) starts a new burst.

use std::time::Duration;

/// How long a hotplug burst must stay quiet before the connectors are rescanned.
pub const HOTPLUG_SETTLE: Duration = Duration::from_millis(250);

/// The burst state: `true` while a settle timer is armed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct HotplugDebounce {
    armed: bool,
}

impl HotplugDebounce {
    /// Records one change event. Returns true only for the first event of a burst, which is the
    /// one that must arm the settle timer.
    pub fn event(&mut self) -> bool {
        !std::mem::replace(&mut self.armed, true)
    }

    /// The settle timer fired: the burst is over and the next event starts a new one.
    pub fn fire(&mut self) {
        self.armed = false;
    }

    /// Whether a settle timer is armed.
    pub fn is_armed(&self) -> bool {
        self.armed
    }
}

#[cfg(test)]
mod tests {
    use super::HotplugDebounce;

    #[test]
    fn a_burst_arms_exactly_one_timer() {
        let mut burst = HotplugDebounce::default();
        assert!(burst.event(), "first event arms the timer");
        assert!(!burst.event(), "events inside the burst are absorbed");
        assert!(!burst.event());
        assert!(burst.is_armed());
    }

    #[test]
    fn a_fired_timer_starts_a_new_burst() {
        let mut burst = HotplugDebounce::default();
        assert!(burst.event());
        burst.fire();
        assert!(!burst.is_armed());
        assert!(burst.event(), "an event after the settle starts a new burst");
    }

    #[test]
    fn a_quiet_device_stays_unarmed() {
        let burst = HotplugDebounce::default();
        assert!(!burst.is_armed());
    }
}
