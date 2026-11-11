//! Applies live control snapshots (settings and displays) to the running lock loop.

use super::*;

impl LockLoop {
    pub(super) fn apply_control_signals(&mut self) {
        let signals = std::mem::take(&mut *self.control_signals.borrow_mut());
        let (latest_settings, latest_displays) = latest_control_snapshots(signals);
        if let Some(snapshot) = latest_settings {
            self.update_live_settings(crate::session::lock_settings_from_snapshot(&snapshot));
        }
        if let Some(snapshot) = latest_displays {
            self.pending_display_snapshot = Some(snapshot);
        }
    }

    pub(super) fn update_live_settings(&mut self, settings: LockSettings) {
        // This path is presentation-only; authentication state and PAM channels stay owned by
        // this lock loop and are never rebuilt for a settings revision.
        self.live_typography_dirty |= typography_changed(&self.settings, &settings);
        self.live_background_dirty |= background_changed(&self.settings, &settings);
        self.settings = settings;
        self.settings_revision += 1;
    }

    pub(super) fn sync_live_settings(&mut self, runtime: &mut SurfaceRuntime) {
        if !self.live_typography_dirty && !self.live_background_dirty {
            return;
        }
        // Borrow the settings field directly: `surfaces` and `fallback` are disjoint fields.
        let settings = &self.settings;
        let root = self.root;
        let typography_dirty = self.live_typography_dirty;
        let background_dirty = self.live_background_dirty;
        let update_visible_background = self.saver.phase() != Phase::RibbonsActive;
        let canvas = if background_dirty {
            document::shared_root_background(settings, root)
        } else {
            None
        };
        let mut typography_ok = true;
        let mut background_ok = true;
        for surface in self.surfaces.iter_mut().chain(self.fallback.iter_mut()) {
            if typography_dirty {
                let result = runtime
                    .with_document(surface.handle, |view| {
                        document::apply_typography(view, settings)
                    })
                    .and_then(|()| runtime.redraw(surface.handle));
                typography_ok &= result.is_ok();
            }
            if background_dirty {
                let background =
                    document::background_for_output(settings, root, surface.rect, canvas.as_ref());
                surface.base_background = background.clone();
                if update_visible_background {
                    let result = runtime
                        .with_document(surface.handle, |view| {
                            document::set_background(view, background)
                        })
                        .and_then(|()| runtime.redraw(surface.handle));
                    background_ok &= result.is_ok();
                }
            }
        }
        if typography_ok {
            self.live_typography_dirty = false;
        }
        if background_ok {
            self.live_background_dirty = false;
        }
    }
}
