//! One lock-loop turn: input wake, key and click dispatch, PAM and power results, and repaint.

use super::*;

impl LockLoop {
    /// One loop turn: wake the screen on input (consuming it), apply keys, clicks,
    /// PAM and power results, animate the screensaver, keep the grabs and stacking,
    /// then repaint only when the projected content changed.
    pub(super) fn tick(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), String> {
        if let Some(size) = runtime.take_root_geometry_changed() {
            self.pending_root_geometry = Some(size);
        }
        if let Some((width, height)) = self.pending_root_geometry
            && self.refresh_root_geometry(runtime, width, height)
        {
            self.pending_root_geometry = None;
        }
        self.apply_control_signals();
        if self.pending_root_geometry.is_none()
            && let Some(snapshot) = self.pending_display_snapshot.take()
        {
            self.apply_display_snapshot(runtime, &snapshot);
        }
        self.sync_live_settings(runtime);
        let now = Instant::now();
        let elapsed = now.saturating_duration_since(self.last_tick);
        self.last_tick = now;

        // XQueryPointer reads the socket into Xlib's queue, so the fd no longer signals for
        // those events: drain the queue here, after the pointer poll, every turn.
        let pointer = self.session.borrow().pointer();
        let fresh = self.session.borrow_mut().read_keys();
        let mut batch = std::mem::take(&mut *self.keys.borrow_mut());
        batch.keys.extend(fresh.keys);
        batch.presses += fresh.presses;
        let clicks = std::mem::take(&mut *self.clicks.borrow_mut());
        let moved = matches!((pointer, self.last_pointer),
            (Some(current), Some(previous)) if current.moved_or_buttons_changed(&previous));
        if let Some(state) = pointer {
            self.caps_lock = state.caps_lock();
            self.last_pointer = Some(state);
        }
        let activity = moved || batch.presses > 0 || !clicks.is_empty();
        let consumed = if activity {
            self.wake(runtime)
        } else {
            if let Some(phase) = self.saver.tick(elapsed) {
                self.enter(runtime, phase, now);
            }
            false
        };

        while let Ok(event) = self.power_rx.try_recv() {
            self.power_event(event);
        }
        self.power_event(PowerEvent::Tick(elapsed));
        if !consumed {
            for key in batch.keys {
                self.key(key, now);
            }
            for name in clicks {
                if let Some(event) = power::ui_action(&name) {
                    self.power_event(event);
                }
            }
        }
        while let Ok(outcome) = self.outcome_rx.try_recv() {
            if self.state.finish(outcome, now) {
                if self.pending_root_geometry.is_some() {
                    self.pending_auth_success = true;
                } else {
                    self.unlocked = true;
                }
            }
        }
        if self.pending_root_geometry.is_none() && self.pending_auth_success {
            self.unlocked = true;
        }
        if self.unlocked {
            self.frame_pacer.set_running(false);
            self.destroy_surfaces(runtime)?;
            // The surfaces and their copied pixels are gone; release the decoded source rasters
            // so the lock process does not keep them alive in its static caches after unlock.
            crate::document::release_wallpaper_caches();
            return Ok(());
        }
        self.settle_display();
        self.sync_ui_visibility(runtime);
        // A refused restore keeps the Ribbons frame up; retried here, never while Ribbons runs.
        if self.saver.phase() != Phase::RibbonsActive {
            self.retry_background_restore(runtime);
        }
        self.animate(runtime, now);
        // Stacking and the grab are safety nets, not per-frame work: at most once a second,
        // while the Ribbons animation wakes this loop at frame rate. Focus and keymap events
        // re-grab immediately through `read_keys`, and a shown or rebuilt surface raises itself.
        if now.saturating_duration_since(self.last_housekeeping) >= HOUSEKEEPING {
            self.last_housekeeping = now;
            // A lost grab is retried; the lock is never dropped by a failed retry.
            self.session.borrow_mut().try_grab();
            keep_lock_step("raise lock surfaces", || self.raise_surfaces(runtime));
        }
        self.repaint(runtime);
        Ok(())
    }
}
