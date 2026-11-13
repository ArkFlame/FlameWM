//! Lock projection paint: widget visibility sync and the repaint of the full lock frame.

use super::*;

impl LockLoop {
    /// Pushes the wanted widget visibility to the document while it is dirty.
    pub(super) fn sync_ui_visibility(&mut self, runtime: &mut SurfaceRuntime) {
        if !self.ui_visibility_dirty {
            return;
        }
        let shown = self.ui_shown_wanted;
        settle(&mut self.ui_visibility_dirty, "lock UI visibility", || {
            for surface in &self.surfaces {
                runtime
                    .with_document(surface.handle, |view| {
                        document::set_lock_ui_shown(view, shown)
                    })
                    .map_err(|error| format!("set lock UI shown: {error:?}"))?;
            }
            if let Some(fallback) = self.fallback.as_ref() {
                runtime
                    .with_document(fallback.handle, |view| {
                        document::set_lock_ui_shown(view, shown)
                    })
                    .map_err(|error| format!("set fallback lock UI shown: {error:?}"))?;
            }
            Ok(())
        });
    }

    /// A failed paint keeps the previous frame: `shown` moves only after a full paint, so the
    /// next turn sees the content still differs and retries.
    pub(super) fn repaint(&mut self, runtime: &mut SurfaceRuntime) {
        // While the screensaver runs the lock UI stays hidden: no content is projected.
        if self.saver.phase() != Phase::Visible {
            return;
        }
        let key = PaintKey {
            time: local_now(),
            password_len: self.state.password().len(),
            status: self.state.status(),
            caps_lock: self.caps_lock,
            settings_revision: self.settings_revision,
            power_revision: self.power_revision,
        };
        if self.shown.is_some() && self.painted_key == Some(key) {
            return;
        }
        let view = LockView::project(
            &self.settings,
            key.time,
            &self.username,
            self.state.password(),
            self.state.status(),
            self.caps_lock,
        );
        let power = self.power.view();
        let view_changed = self.shown.as_ref() != Some(&view);
        let power_changed = self.shown_power.as_ref() != Some(&power);
        if !view_changed && !power_changed {
            self.painted_key = Some(key);
            return;
        }
        let painted = keep_lock_step("repaint lock surface", || {
            for surface in &self.surfaces {
                runtime
                    .with_document(surface.handle, |document_view| {
                        if view_changed {
                            document::apply_view(document_view, &view)?;
                        }
                        if power_changed {
                            document::apply_power(
                                document_view,
                                surface.rect.width.max(1) as u32,
                                surface.rect.height.max(1) as u32,
                                &power,
                            )?;
                        }
                        Ok(())
                    })
                    .map_err(|error| format!("update lock surface: {error:?}"))?;
                runtime
                    .redraw(surface.handle)
                    .map_err(|error| format!("redraw lock surface: {error:?}"))?;
            }
            if let Some(surface) = self.fallback.as_ref() {
                runtime
                    .with_document(surface.handle, |document_view| {
                        if view_changed {
                            document::apply_view(document_view, &view)?;
                        }
                        if power_changed {
                            document::apply_power(
                                document_view,
                                surface.rect.width.max(1) as u32,
                                surface.rect.height.max(1) as u32,
                                &power,
                            )?;
                        }
                        Ok(())
                    })
                    .map_err(|error| format!("update fallback lock surface: {error:?}"))?;
                runtime
                    .redraw(surface.handle)
                    .map_err(|error| format!("redraw fallback lock surface: {error:?}"))?;
            }
            Ok(())
        });
        if painted {
            self.shown = Some(view);
            self.shown_power = Some(power);
            self.painted_key = Some(key);
        }
    }
}
