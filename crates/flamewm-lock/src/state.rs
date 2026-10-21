//! Key-event reducer for the lock screen. Pure: no X, PAM, or clock access.
use std::time::Instant;

use crate::backoff::delay_after_failures;
use crate::pam::Outcome;
use crate::secret::PasswordBuffer;

/// Keys the lock screen reacts to; everything else is dropped by the X layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Backspace,
    Escape,
    Enter,
    /// F1..F4: keyboard access to the power row (1 = Sleep .. 4 = Other).
    Function(u8),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Idle,
    Authenticating,
    Failed,
    Unavailable,
}

impl Status {
    pub fn text(self) -> &'static str {
        match self {
            Self::Idle => "",
            Self::Authenticating => "Authenticating\u{2026}",
            Self::Failed => "Authentication failed",
            Self::Unavailable => "Authentication unavailable",
        }
    }
}

/// What the caller must do after a key was applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    None,
    /// The password buffer text changed; repaint the masked field.
    Edited,
    /// Enter on a non-empty buffer: call [`LockState::begin_authentication`].
    Submit,
}

#[derive(Default)]
pub struct LockState {
    password: PasswordBuffer,
    status: Status,
    failures: u32,
    locked_until: Option<Instant>,
}

impl LockState {
    pub fn password(&self) -> &PasswordBuffer {
        &self.password
    }

    pub fn status(&self) -> Status {
        self.status
    }

    /// Input is ignored while authenticating and during a backoff delay.
    pub fn accepts_input(&self, now: Instant) -> bool {
        self.status != Status::Authenticating && self.locked_until.is_none_or(|until| now >= until)
    }

    pub fn key(&mut self, key: Key, now: Instant) -> Effect {
        if !self.accepts_input(now) {
            return Effect::None;
        }
        match key {
            Key::Char(character) => {
                let before = self.password.len();
                self.password.push(character);
                edited_if(self.password.len() != before)
            }
            Key::Backspace => {
                let had_text = !self.password.is_empty();
                self.password.backspace();
                edited_if(had_text)
            }
            Key::Escape => {
                let had_text = !self.password.is_empty();
                self.password.clear();
                edited_if(had_text)
            }
            Key::Enter if self.password.is_empty() => Effect::None,
            Key::Enter => Effect::Submit,
            Key::Function(_) => Effect::None,
        }
    }

    /// Moves the typed secret out for the PAM worker and enters the
    /// authenticating state. The buffer is empty afterwards.
    pub fn begin_authentication(&mut self) -> PasswordBuffer {
        self.status = Status::Authenticating;
        std::mem::take(&mut self.password)
    }

    /// Applies a PAM outcome. Returns `true` only for [`Outcome::Unlocked`],
    /// the single path that may release the lock.
    pub fn finish(&mut self, outcome: Outcome, now: Instant) -> bool {
        match outcome {
            Outcome::Unlocked => true,
            Outcome::Denied => {
                self.failures = self.failures.saturating_add(1);
                let delay = delay_after_failures(self.failures);
                self.locked_until = (!delay.is_zero()).then(|| now + delay);
                self.status = Status::Failed;
                false
            }
            Outcome::Unavailable => {
                self.status = Status::Unavailable;
                false
            }
        }
    }
}

fn edited_if(changed: bool) -> Effect {
    if changed {
        Effect::Edited
    } else {
        Effect::None
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn type_text(state: &mut LockState, text: &str, now: Instant) {
        for character in text.chars() {
            state.key(Key::Char(character), now);
        }
    }

    #[test]
    fn typing_appends_and_backspace_pops() {
        let now = Instant::now();
        let mut state = LockState::default();
        assert_eq!(state.key(Key::Char('a'), now), Effect::Edited);
        assert_eq!(state.key(Key::Char('\n'), now), Effect::None);
        type_text(&mut state, "bc", now);
        assert_eq!(state.password().len(), 3);
        assert_eq!(state.key(Key::Backspace, now), Effect::Edited);
        assert_eq!(state.password().as_str(), "ab");
        state.key(Key::Backspace, now);
        state.key(Key::Backspace, now);
        assert_eq!(state.key(Key::Backspace, now), Effect::None);
    }

    #[test]
    fn escape_clears_and_enter_needs_text() {
        let now = Instant::now();
        let mut state = LockState::default();
        assert_eq!(state.key(Key::Enter, now), Effect::None);
        type_text(&mut state, "secret", now);
        assert_eq!(state.key(Key::Escape, now), Effect::Edited);
        assert!(state.password().is_empty());
        assert_eq!(state.key(Key::Escape, now), Effect::None);
        type_text(&mut state, "x", now);
        assert_eq!(state.key(Key::Enter, now), Effect::Submit);
    }

    #[test]
    fn submit_moves_secret_out_and_blocks_input_while_authenticating() {
        let now = Instant::now();
        let mut state = LockState::default();
        type_text(&mut state, "pw", now);
        let secret = state.begin_authentication();
        assert_eq!(secret.as_str(), "pw");
        assert!(state.password().is_empty());
        assert_eq!(state.status(), Status::Authenticating);
        assert_eq!(state.key(Key::Char('z'), now), Effect::None);
        assert!(!state.accepts_input(now));
    }

    #[test]
    fn denial_backoff_ignores_input_until_delay_passes() {
        let now = Instant::now();
        let mut state = LockState::default();
        for _ in 0..2 {
            type_text(&mut state, "x", now);
            state.begin_authentication();
            assert!(!state.finish(Outcome::Denied, now));
        }
        // Third failure: 1 s backoff, input dropped.
        type_text(&mut state, "x", now);
        state.begin_authentication();
        assert!(!state.finish(Outcome::Denied, now));
        assert_eq!(state.status(), Status::Failed);
        assert_eq!(state.key(Key::Char('a'), now), Effect::None);
        let later = now + Duration::from_millis(1100);
        assert_eq!(state.key(Key::Char('a'), later), Effect::Edited);
        assert_eq!(state.password().as_str(), "a");
    }

    #[test]
    fn unavailable_sets_status_without_backoff() {
        let now = Instant::now();
        let mut state = LockState::default();
        type_text(&mut state, "x", now);
        state.begin_authentication();
        assert!(!state.finish(Outcome::Unavailable, now));
        assert_eq!(state.status().text(), "Authentication unavailable");
        assert!(state.accepts_input(now));
    }

    #[test]
    fn only_an_unlocked_outcome_releases_the_lock() {
        let now = Instant::now();
        let mut state = LockState::default();
        for outcome in [Outcome::Denied, Outcome::Unavailable] {
            assert!(!state.finish(outcome, now));
        }
        assert!(state.finish(Outcome::Unlocked, now));
    }
}
