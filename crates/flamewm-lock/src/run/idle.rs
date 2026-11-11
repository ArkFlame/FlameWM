//! Idle screensaver phases, Ribbons background animation, background restore and DPMS settling.

use super::*;

impl LockLoop {
    /// Applies a phase the screensaver entered on idle time.
    pub(super) fn enter(&mut self, runtime: &mut SurfaceRuntime, phase: Phase, now: Instant) {
        if phase != Phase::Visible {
            self.ui_shown_wanted = false;
            self.ui_visibility_dirty = true;
        }
        match phase {
            Phase::RibbonsActive => {
                self.frames = Some(
                    self.surfaces
                        .iter()
                        .map(|surface| {
                            RibbonFrames::new(
                                surface.rect.width.max(1) as u32,
                                surface.rect.height.max(1) as u32,
                                &self.settings.saver,
                                now,
                            )
                        })
                        .collect(),
                );
                self.fallback_frames = self.fallback.as_ref().map(|surface| {
                    RibbonFrames::new(
                        surface.rect.width.max(1) as u32,
                        surface.rect.height.max(1) as u32,
                        &self.settings.saver,
                        now,
                    )
                });
                self.next_frame = now;
                self.frame_pacer.set_running(true);
            }
            Phase::DisplayOff => {
                self.frame_pacer.set_running(false);
                if self.frames.take().is_some() {
                    self.restore_background(runtime);
                }
                // Fail open on the display only: a refused DPMS call leaves the screen on, still locked.
                self.session.borrow_mut().set_display_power(false);
            }
            Phase::Visible => {}
        }
    }

    /// Retries DPMS on until the server accepts it; a refused call is logged, never fatal.
    pub(super) fn settle_display(&mut self) {
        if !self.display_on_pending {
            return;
        }
        if self.session.borrow_mut().set_display_power(true) {
            self.display_on_pending = false;
        } else {
            eprintln!("flamewm-lock: display power on refused, retrying next turn");
        }
    }

    /// Renders the next Ribbons frame into the background when one is due.
    /// A failed frame keeps the previous background; `next_frame` already moved on, so the
    /// next due frame retries.
    pub(super) fn animate(&mut self, runtime: &mut SurfaceRuntime, now: Instant) {
        if self.saver.phase() != Phase::RibbonsActive || now < self.next_frame {
            return;
        }
        let Some(frames) = self.frames.as_mut() else {
            return;
        };
        self.next_frame = now + FRAME_INTERVAL;
        keep_lock_step("animate lock background", || {
            update_animated_backgrounds(
                runtime,
                &self.surfaces,
                self.fallback.as_ref(),
                frames,
                self.fallback_frames.as_mut(),
                now,
            )
        });
    }

    pub(super) fn restore_background(&mut self, runtime: &mut SurfaceRuntime) {
        settle(
            &mut self.background_stale,
            "lock background restore",
            || restore_surface_backgrounds(runtime, &self.surfaces, self.fallback.as_ref()),
        );
    }

    /// Re-applies the wallpaper while a failed restore is pending. Not called during Ribbons.
    pub(super) fn retry_background_restore(&mut self, runtime: &mut SurfaceRuntime) {
        if self.background_stale {
            self.restore_background(runtime);
        }
    }
}
