use super::*;

impl X11App {
    pub(super) unsafe fn paint_full(
        &mut self,
        document: &RuntimeDocument,
        scale: f32,
    ) -> Result<u64, String> {
        // Counts every whole-frame attempt; the `render.clear` count is the XRender frames.
        let gpu = {
            let _gpu_guard = flamewm_profiler::start("render.gpu_frame");
            unsafe { self.paint_full_gpu(document, scale) }
        };
        if let Some(count) = gpu {
            unsafe { self.present_scene()? };
            return Ok(count);
        }
        {
            let _clear_guard = flamewm_profiler::start("render.clear");
            self.clear_scene()?;
            unsafe { self.paint_surface_backdrop()? };
        }
        // Borrow retained commands by index: build fresh only when cache
        // invalid; single coverage/shape per frame; no clone path.
        let count = self
            .cached_paint
            .as_ref()
            .expect("paint cached above")
            .commands
            .len();
        {
            let _draw_guard = flamewm_profiler::start("render.draw");
            unsafe { self.prefetch_text_shadow_masks(count, document, scale) };
            for index in 0..count {
                // SAFETY: index is below the cached count; `paint` does not replace the cache.
                let command = unsafe { &*self.retained_command(index) };
                // SAFETY: redraw is entered only with the app's initialized X11 resources.
                unsafe { self.paint(command, document, scale)? };
            }
        }
        self.finish_node_backdrops();
        unsafe { self.present_scene()? };
        Ok(count as u64)
    }

    /// Pointer to retained command `index`, so painting can borrow `self` mutably. Valid
    /// while the paint cache is not replaced, which `paint` never does.
    pub(super) fn retained_command(&self, index: usize) -> *const PaintCommand {
        let base = self
            .cached_paint
            .as_ref()
            .expect("paint cached above")
            .commands
            .as_ptr();
        // SAFETY: callers pass `index` below the cached command count.
        unsafe { base.add(index) }
    }

    /// Full relayout and repaint for a window size change or Expose. A retained scene can
    /// predate the new size or the document state the resize hand-off just applied.
    pub(crate) unsafe fn repaint_full(
        &mut self,
        document: &mut RuntimeDocument,
    ) -> Result<(), String> {
        self.mark_full();
        unsafe { self.redraw_if_dirty(document)? };
        Ok(())
    }
}
