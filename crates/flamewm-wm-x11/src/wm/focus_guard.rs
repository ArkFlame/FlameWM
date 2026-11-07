//! Basic focus-stealing prevention: a window the user did not ask for does not take focus right
//! after the user interacted with the WM. Pager and user activations always focus.

use std::time::{Duration, Instant};

/// How long after a button or WM key press an unrequested focus change is held back.
pub(super) const RECENT_INPUT: Duration = Duration::from_secs(2);

/// EWMH `_NET_ACTIVE_WINDOW` source indication: 2 is a pager (or user-driven) request.
pub(super) const SOURCE_PAGER: u32 = 2;

#[derive(Debug, Default)]
pub(super) struct FocusGuard {
    last_input: Option<Instant>,
}

impl FocusGuard {
    pub(super) fn note_user_activity(&mut self, now: Instant) {
        self.last_input = Some(now);
    }

    pub(super) fn recent_input(&self, now: Instant) -> bool {
        self.last_input
            .is_some_and(|at| now.saturating_duration_since(at) < RECENT_INPUT)
    }
}

/// A newly mapped window may take focus unless the user just interacted, something else is
/// active, or the window is a transient of the active window.
pub(super) fn map_may_take_focus(recent_input: bool, has_active: bool, transient: bool) -> bool {
    !recent_input || !has_active || transient
}

/// An explicit `_NET_ACTIVE_WINDOW` request is honoured after recent user input only from a pager.
pub(super) fn active_request_may_focus(source: u32, recent_input: bool) -> bool {
    source == SOURCE_PAGER || !recent_input
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_input_window_expires() {
        let t0 = Instant::now();
        let mut guard = FocusGuard::default();
        assert!(!guard.recent_input(t0));
        guard.note_user_activity(t0);
        assert!(guard.recent_input(t0 + RECENT_INPUT / 2));
        assert!(!guard.recent_input(t0 + RECENT_INPUT));
    }

    #[test]
    fn new_map_does_not_steal_focus_after_recent_input() {
        assert!(!map_may_take_focus(true, true, false));
        assert!(map_may_take_focus(false, true, false));
        assert!(map_may_take_focus(true, false, false));
        assert!(map_may_take_focus(true, true, true));
    }

    #[test]
    fn app_activation_is_blocked_only_after_recent_input() {
        assert!(!active_request_may_focus(1, true));
        assert!(!active_request_may_focus(0, true));
        assert!(active_request_may_focus(SOURCE_PAGER, true));
        assert!(active_request_may_focus(1, false));
    }
}
