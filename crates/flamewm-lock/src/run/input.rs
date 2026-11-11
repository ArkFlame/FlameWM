//! Lock-loop input handling: screensaver wake, key and pointer submission, and power requests.

use super::*;

impl LockLoop {
    /// Records input for the screensaver. `true` when it woke the screen: the input
    /// of this turn is consumed and must not reach the password or the power row.
    pub(super) fn wake(&mut self, runtime: &mut SurfaceRuntime) -> bool {
        let before = self.saver.phase();
        if !self.saver.input() {
            return false;
        }
        self.frame_pacer.set_running(false);
        self.frames = None;
        self.fallback_frames = None;
        if before == Phase::RibbonsActive || before == Phase::DisplayOff {
            self.restore_background(runtime);
        }
        if before == Phase::DisplayOff {
            self.display_on_pending = true;
        }
        if before != Phase::Visible {
            // Showing the widgets again; the repaint this turn re-applies their content.
            self.ui_shown_wanted = true;
            self.ui_visibility_dirty = true;
            self.shown = None;
            self.shown_power = None;
        }
        true
    }

    pub(super) fn key(&mut self, key: Key, now: Instant) {
        match key {
            Key::Enter if self.power.view().confirm.is_some() => {
                self.power_event(PowerEvent::Confirm);
            }
            Key::Function(number) => {
                if let Some(button) = usize::from(number)
                    .checked_sub(1)
                    .and_then(|i| POWER_ORDER.get(i))
                {
                    self.power_event(PowerEvent::Click(*button));
                }
            }
            _ => {
                if key == Key::Escape {
                    self.power_event(PowerEvent::Escape);
                }
                if self.state.key(key, now) == Effect::Submit {
                    self.submit();
                }
            }
        }
    }

    /// Feeds one event to the power reducer; a requested action runs on a worker thread.
    pub(super) fn power_event(&mut self, event: PowerEvent) {
        // A tick only changes the power view while an error message is on screen (TTL expiry).
        let visible_change = !matches!(event, PowerEvent::Tick(_))
            || self
                .shown_power
                .as_ref()
                .is_some_and(|shown| !shown.message.is_empty());
        if visible_change {
            self.power_revision += 1;
        }
        if let PowerEffect::Perform(action) = self.power.handle(event) {
            power::perform_on_worker(action, self.power_tx.clone());
        }
    }

    pub(super) fn submit(&mut self) {
        let secret = self.state.begin_authentication();
        let user = self.username.clone();
        let outcome_tx = self.outcome_tx.clone();
        let spawned = thread::Builder::new()
            .name("flamewm-lock-pam".to_string())
            .spawn(move || {
                let outcome = pam::authenticate(&user, secret.as_str());
                drop(secret);
                // The receiver lives as long as the loop; a send error means it is gone.
                let _ = outcome_tx.send(outcome);
            });
        if spawned.is_err() {
            let _ = self.outcome_tx.send(Outcome::Unavailable);
        }
    }
}
