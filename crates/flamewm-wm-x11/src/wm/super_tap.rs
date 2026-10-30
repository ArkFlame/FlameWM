//! Bare Super key tap detection for the start-menu toggle.

/// Result of a `SuperTap` transition. `Toggle` asks the WM to flip Start once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SuperTapOutcome {
    Idle,
    Toggle,
}

/// Bare-Super tap detector with KDE Meta semantics: a tap is a bare Super press
/// whose release is not followed by any other key while Super is held.
///
/// X autorepeat on a held key emits a `KeyRelease` and `KeyPress` pair that share
/// one server timestamp. A release is therefore held as `candidate` and only
/// completes when the event batch ends (`commit`) or another key arrives; a
/// same-timestamp Super press cancels it, so repeat pairs never toggle.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct SuperTap {
    /// Keycode of the Super key currently held.
    pub(super) held: Option<u8>,
    /// Another key was pressed while Super was held, so its release is not a tap.
    pub(super) chorded: bool,
    /// Keycode and timestamp of a completed bare-Super release awaiting commit.
    pub(super) candidate: Option<(u8, u32)>,
}

impl SuperTap {
    /// Bare Start-bound Super press.
    pub(super) fn press_super(&mut self, keycode: u8, time: u32) -> SuperTapOutcome {
        let mut outcome = SuperTapOutcome::Idle;
        if let Some((key, stamp)) = self.candidate.take() {
            if key == keycode && stamp == time {
                // Autorepeat pair: this press shares the release's timestamp.
                self.held = Some(keycode);
                return SuperTapOutcome::Idle;
            }
            // A new press after a completed tap: the previous tap completes now.
            outcome = SuperTapOutcome::Toggle;
        }
        if self.held == Some(keycode) {
            // Repeat press with no intervening release: still the same hold.
            return outcome;
        }
        self.held = Some(keycode);
        self.chorded = false;
        outcome
    }

    /// Any release. Only the release of the held Super key can complete a tap.
    pub(super) fn release_super(&mut self, keycode: u8, time: u32) {
        if self.held != Some(keycode) {
            return;
        }
        self.held = None;
        if !self.chorded {
            self.candidate = Some((keycode, time));
        }
        self.chorded = false;
    }

    /// Any key press that is not a bare Start-bound Super press.
    pub(super) fn press_other(&mut self) -> SuperTapOutcome {
        if self.held.is_some() {
            self.chorded = true;
            return SuperTapOutcome::Idle;
        }
        // A completed tap followed by another key still counts as a tap.
        self.commit()
    }

    /// One raw key press. `start_binding` is true only for a press of the Start-bound Super key
    /// with no other modifier held; every other press completes or cancels a pending tap.
    pub(super) fn raw_press(
        &mut self,
        start_binding: bool,
        keycode: u8,
        time: u32,
    ) -> SuperTapOutcome {
        if start_binding {
            self.press_super(keycode, time)
        } else {
            self.press_other()
        }
    }

    /// Completes a pending tap at the end of an event batch.
    pub(super) fn commit(&mut self) -> SuperTapOutcome {
        if self.candidate.take().is_some() {
            SuperTapOutcome::Toggle
        } else {
            SuperTapOutcome::Idle
        }
    }

    /// Forgets all Super state so a later release cannot toggle Start.
    pub(super) fn focus_lost(&mut self) {
        *self = Self::default();
    }
}

/// True when the modifier state has no bits besides the ignored lock keys.
pub(super) fn is_bare_state(state: u16) -> bool {
    use flamewm_x11_desktop::shortcuts::{MOD_LOCK, MOD_NUM_LOCK};
    state & !(MOD_LOCK | MOD_NUM_LOCK) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUPER: u8 = 133;
    const KEY_A: u8 = 38;

    #[test]
    fn bare_super_tap_toggles_once_at_batch_end() {
        let mut tap = SuperTap::default();
        assert_eq!(tap.raw_press(true, SUPER, 10), SuperTapOutcome::Idle);
        tap.release_super(SUPER, 20);
        assert_eq!(tap.commit(), SuperTapOutcome::Toggle);
        assert_eq!(tap.commit(), SuperTapOutcome::Idle);
    }

    #[test]
    fn super_chord_with_another_key_is_not_a_tap() {
        let mut tap = SuperTap::default();
        tap.raw_press(true, SUPER, 10);
        // Super+a: the key press marks the hold as chorded and the Super release is no tap.
        assert_eq!(tap.raw_press(false, KEY_A, 11), SuperTapOutcome::Idle);
        tap.release_super(SUPER, 12);
        assert_eq!(tap.commit(), SuperTapOutcome::Idle);
    }

    #[test]
    fn autorepeat_pair_sharing_release_timestamp_never_toggles() {
        let mut tap = SuperTap::default();
        tap.raw_press(true, SUPER, 10);
        tap.release_super(SUPER, 20);
        assert_eq!(tap.raw_press(true, SUPER, 20), SuperTapOutcome::Idle);
        assert_eq!(tap.commit(), SuperTapOutcome::Idle);
    }

    #[test]
    fn super_press_that_is_not_the_start_binding_never_toggles() {
        let mut tap = SuperTap::default();
        assert_eq!(tap.raw_press(false, SUPER, 10), SuperTapOutcome::Idle);
        tap.release_super(SUPER, 20);
        assert_eq!(tap.commit(), SuperTapOutcome::Idle);
    }

    #[test]
    fn tap_followed_by_another_key_still_toggles() {
        let mut tap = SuperTap::default();
        tap.raw_press(true, SUPER, 10);
        tap.release_super(SUPER, 20);
        assert_eq!(tap.raw_press(false, KEY_A, 30), SuperTapOutcome::Toggle);
    }
}
