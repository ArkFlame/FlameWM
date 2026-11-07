use super::*;

impl X11App {
    /// Mark damage without painting: merged (max coverage) latch consumed
    /// by `redraw_if_dirty`. Returns false when damage is empty.
    pub(crate) fn mark_dirty(&mut self, damage: SurfaceDamage) {
        self.pending_damage = self.pending_damage.merge(damage);
    }

    pub(crate) fn mark_full(&mut self) {
        self.pending_damage = SurfaceDamage::Full;
    }

    /// Input handled: repaint through the planner. Unlike `mark_full`, this does not latch a
    /// Full repaint, because the command diff bounds what the input changed.
    pub(crate) fn mark_input_repaint(&mut self) {
        self.input_repaint = true;
    }

    /// Mirrors the document's keyboard focus into the interaction state. A change repaints
    /// through the input path, so only the focus ring's damage region is redrawn.
    pub(crate) fn sync_keyboard_focus(&mut self, document: &RuntimeDocument) {
        if document.focus() != self.interaction.focus {
            self.interaction.focus = document.focus();
            self.mark_input_repaint();
        }
    }

    /// Paint only when damage is latched or the document revision moved
    /// past the retained cache; otherwise skip the frame.
    pub(crate) unsafe fn redraw_if_dirty(
        &mut self,
        document: &mut RuntimeDocument,
    ) -> Result<bool, String> {
        let doc_damage = document.take_damage();
        // Only an explicit full request (mark_full: Expose, resize, backdrop deadline) latches a
        // Full repaint. Document edits report Full damage too, but the command diff in the
        // planner bounds them, so they must not disable the region path.
        let explicit_full = matches!(self.pending_damage, SurfaceDamage::Full);
        self.pending_damage = self.pending_damage.merge(doc_damage);
        let input = std::mem::take(&mut self.input_repaint);
        // Paint staleness follows the paint revision: color edits must repaint even when
        // retained layout is reused.
        let stale = self
            .cached_paint
            .as_ref()
            .is_none_or(|cached| cached.revision != document.revision());
        if self.pending_damage.is_empty() && !stale && !input {
            return Ok(false);
        }
        if explicit_full {
            self.region_force_full = true;
        }
        // Skipping an unchanged scene is only allowed when input was the sole cause. A document
        // revision bump or latched damage always repaints: the command list can compare equal while
        // the pixels behind a retained asset changed.
        self.skip_eligible = input && !stale && !explicit_full && self.pending_damage.is_empty();
        self.pending_damage = SurfaceDamage::None;
        unsafe { self.redraw(document)? };
        Ok(true)
    }

    /// Transparent ARGB scene clear: 0x00000000, never root_background/BLACK.
    pub(crate) fn clear_scene(&mut self) -> Result<(), String> {
        // Depth-32 ARGB scene: pixel 0 == 0x00000000 transparent clear.
        // Depth-24 has no alpha: 0x00000000 would be opaque black under the panel corners.
        let clear = scene_clear_pixel(self.depth);
        unsafe { XSetForeground(self.display, self.gc, clear) };
        unsafe { XSetClipMask(self.display, self.gc, 0) };
        self.gc_clip = None;
        unsafe {
            XFillRectangle(
                self.display,
                self.backbuffer,
                self.gc,
                0,
                0,
                self.width,
                self.height,
            )
        };
        Ok(())
    }
}
