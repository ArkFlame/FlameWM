//! Pure fail-safe decision for the lock screen.
//!
//! A failure to paint, a missing PAM backend and an undecodable/missing wallpaper
//! are presentation failures: the session must stay covered by a usable lock,
//! drawn with the solid theme colour, and no failure may release the lock.

/// Which lock subsystems are degraded. A `true` flag means that subsystem failed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Faults {
    pub render: bool,
    pub pam: bool,
    pub background: bool,
}

/// What the lock must present after the subsystems are inspected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fallback {
    /// A usable lock is always shown; no fault removes the cover.
    pub show_lock: bool,
    /// The solid-colour background is selected when any subsystem is degraded.
    pub solid_colour: bool,
    /// A fault never opens the session: only an explicit success verdict does.
    pub unlock: bool,
}

/// The single pure fail-safe decision.
///
/// `show_lock` is unconditional and `unlock` is always false here: this decision
/// only describes the degraded presentation. A successful authentication is the
/// sole path that may release the lock, and it is gated separately by the pure
/// state reducer ([`crate::state::LockState::finish`]).
#[must_use]
pub fn decide(faults: Faults) -> Fallback {
    let degraded = faults.render || faults.pam || faults.background;
    Fallback {
        show_lock: true,
        solid_colour: degraded,
        unlock: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every combination of the three faults must stay covered and stay locked;
    /// the solid colour is selected exactly when at least one subsystem failed.
    #[test]
    fn every_fault_combination_shows_a_lock_and_never_unlocks() {
        for render in [false, true] {
            for pam in [false, true] {
                for background in [false, true] {
                    let faults = Faults {
                        render,
                        pam,
                        background,
                    };
                    let decision = decide(faults);
                    assert!(decision.show_lock, "lost the lock cover for {faults:?}");
                    assert!(
                        !decision.unlock,
                        "a fault unlocked the session for {faults:?}"
                    );
                    assert_eq!(
                        decision.solid_colour,
                        render || pam || background,
                        "wrong background fallback for {faults:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn each_single_failure_uses_the_solid_colour_without_unlocking() {
        for faults in [
            Faults {
                render: true,
                ..Default::default()
            },
            Faults {
                pam: true,
                ..Default::default()
            },
            Faults {
                background: true,
                ..Default::default()
            },
        ] {
            assert_eq!(
                decide(faults),
                Fallback {
                    show_lock: true,
                    solid_colour: true,
                    unlock: false,
                }
            );
        }
    }

    /// Render-failure flag, end to end through the pure reducer: the degraded presentation keeps
    /// a cover, the reducer keeps the session locked on every non-success outcome, and input
    /// still works so the user can retry. Only `Outcome::Unlocked` releases.
    #[test]
    fn render_fault_keeps_a_cover_and_the_reducer_stays_locked() {
        use std::time::Instant;

        use crate::pam::Outcome;
        use crate::state::{Key, LockState, Status};

        let faults = Faults {
            render: true,
            ..Default::default()
        };
        let cover = decide(faults);
        assert!(cover.show_lock && cover.solid_colour && !cover.unlock);

        let now = Instant::now();
        let mut state = LockState::default();
        for outcome in [Outcome::Denied, Outcome::Unavailable] {
            state.key(Key::Char('x'), now);
            state.begin_authentication();
            assert!(!state.finish(outcome, now), "{outcome:?} released the lock");
        }
        assert_eq!(state.status(), Status::Unavailable);
        assert!(state.finish(Outcome::Unlocked, now));
    }

    #[test]
    fn a_healthy_lock_keeps_the_normal_background_and_stays_locked() {
        assert_eq!(
            decide(Faults::default()),
            Fallback {
                show_lock: true,
                solid_colour: false,
                unlock: false,
            }
        );
    }
}
