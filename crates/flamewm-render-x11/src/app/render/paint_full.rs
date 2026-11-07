use super::*;

/// Destination clip covering the whole window at `width` x `height` device pixels.
pub(crate) fn window_clip_rect(width: u32, height: u32) -> XRectangle {
    XRectangle {
        x: 0,
        y: 0,
        width: width as u16,
        height: height as u16,
    }
}

impl X11App {
    /// Restores the XRender destination clip to the whole current window. A region paint narrows the
    /// clip to its own rect and resets it to the window extent of that moment; after a grow that extent
    /// is stale, and XRender fills stop at it while Xft text does not. Every frame that fills through
    /// XRender must start from the current window.
    pub(crate) fn reset_destination_clip(&mut self) -> Result<(), String> {
        let clip = window_clip_rect(self.width, self.height);
        match self.xrender.as_mut() {
            Some(xrender) => unsafe { xrender.set_destination_clip(clip) },
            None => Ok(()),
        }
    }

    pub(super) unsafe fn paint_full(
        &mut self,
        document: &RuntimeDocument,
        scale: f32,
    ) -> Result<u64, String> {
        self.reset_destination_clip()?;
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

#[cfg(test)]
mod tests {
    use super::window_clip_rect;

    #[test]
    fn destination_clip_covers_the_grown_window_not_the_region_extent() {
        // Regression: a region paint at the old 1250x726 left the XRender clip there; a grown
        // 1594x820 window must still be filled to its full extent.
        let grown = window_clip_rect(1594, 820);
        assert_eq!((grown.x, grown.y), (0, 0));
        assert_eq!((grown.width, grown.height), (1594, 820));
    }

    #[test]
    fn full_paint_resets_the_destination_clip_before_any_fill() {
        let source = include_str!("paint_full.rs");
        let body = &source[source
            .find("pub(super) unsafe fn paint_full(")
            .expect("full paint entry remains")..];
        let reset = body
            .find("self.reset_destination_clip()?;")
            .expect("full paint resets the clip");
        let first_fill = body
            .find("paint_full_gpu")
            .expect("full paint has a GPU/XRender fill path");
        assert!(
            reset < first_fill,
            "the clip must be restored before the first XRender fill of the frame"
        );
        assert!(
            include_str!("paint_region.rs").contains("self.reset_destination_clip()"),
            "region paint and full paint share one clip reset owner"
        );
    }
}
