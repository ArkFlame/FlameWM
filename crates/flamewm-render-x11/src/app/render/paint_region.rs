use super::*;

impl X11App {
    /// Repaints only `region` of the retained backbuffer: clear it, paint the commands that
    /// touch it under a picture clip, reset the clip, then copy that rect to the window.
    /// Returns the number of commands painted.
    pub(super) unsafe fn paint_region(
        &mut self,
        region: PixelRegion,
        document: &RuntimeDocument,
        scale: f32,
    ) -> Result<u64, String> {
        let _region_guard = flamewm_profiler::start("render.region");
        // Validate before touching the backbuffer: bailing out after the clear below would leave
        // the region blank, so an unsupported command must fall back with the pixels intact.
        {
            let viewport = (self.width, self.height);
            let cached = self
                .cached_paint
                .as_ref()
                .ok_or("region paint has no cached scene")?;
            if cached.commands.iter().any(|command| {
                !super::damage_plan::no_pixel_command(command)
                    && command_bounds(command, viewport).is_none()
            }) {
                return Err("unsupported command in region paint".to_string());
            }
        }
        // Same depth-aware clear as `clear_scene`: a depth-24 backbuffer has no alpha, so 0 would
        // be opaque black in the repainted corners.
        unsafe { XSetForeground(self.display, self.gc, scene_clear_pixel(self.depth)) };
        unsafe { XSetClipMask(self.display, self.gc, 0) };
        self.gc_clip = None;
        unsafe {
            XFillRectangle(
                self.display,
                self.backbuffer,
                self.gc,
                region.x,
                region.y,
                region.width,
                region.height,
            )
        };
        let xrender = self
            .xrender
            .as_mut()
            .ok_or("region paint needs the XRender backend")?;
        let clip = XRectangle {
            x: region.x as i16,
            y: region.y as i16,
            width: region.width as u16,
            height: region.height as u16,
        };
        unsafe { xrender.set_destination_clip(clip)? };
        // The clear above erased the window backdrop here; lay it back (clipped to the region).
        unsafe { self.paint_surface_backdrop()? };
        self.region_text_clip = Some(region);
        let painted = unsafe { self.paint_region_commands(region, document, scale) };
        // Reset on every path, error or not: the clips must never leak into a later frame.
        self.region_text_clip = None;
        let (width, height) = (self.width, self.height);
        let reset = match self.xrender.as_mut() {
            Some(xrender) => unsafe {
                xrender.set_destination_clip(XRectangle {
                    x: 0,
                    y: 0,
                    width: width as u16,
                    height: height as u16,
                })
            },
            None => Ok(()),
        };
        let visits = painted?;
        reset?;
        unsafe { XSetClipMask(self.display, self.gc, 0) };
        self.gc_clip = None;
        unsafe { XSetClipOrigin(self.display, self.gc, 0, 0) };
        unsafe {
            XCopyArea(
                self.display,
                self.backbuffer,
                self.window,
                self.gc,
                region.x,
                region.y,
                region.width,
                region.height,
                region.x,
                region.y,
            )
        };
        unsafe { XFlush(self.display) };
        Ok(visits)
    }

    pub(super) unsafe fn paint_region_commands(
        &mut self,
        region: PixelRegion,
        document: &RuntimeDocument,
        scale: f32,
    ) -> Result<u64, String> {
        let count = self
            .cached_paint
            .as_ref()
            .expect("paint cached above")
            .commands
            .len();
        let viewport = (self.width, self.height);
        let mut visits = 0u64;
        for index in 0..count {
            // SAFETY: index is below the cached count; `paint` does not replace the cache.
            let command = unsafe { &*self.retained_command(index) };
            // Commands that write no pixels (clips, the scrollbar track) always replay so the clip
            // stack and ordering match the Full path.
            let always = super::damage_plan::no_pixel_command(command);
            if !always {
                let Some(bounds) = command_bounds(command, viewport) else {
                    return Err("unsupported command in region paint".to_string());
                };
                if !bounds.intersects(region) {
                    continue;
                }
            }
            visits += 1;
            // The GC fallbacks have no picture clip, so they could write outside the region.
            let needs_xft = matches!(
                command,
                PaintCommand::Text { .. } | PaintCommand::TextShadow { .. }
            );
            if needs_xft {
                self.ensure_xft();
            }
            if needs_xft && self.xft.is_none() {
                return Err("region text needs the Xft backend".to_string());
            }
            if matches!(
                command,
                PaintCommand::Image { .. } | PaintCommand::TextShadow { .. }
            ) && self.xrender.is_none()
            {
                return Err("region image needs the XRender backend".to_string());
            }
            // SAFETY: same initialized resources as the Full path.
            unsafe { self.paint(command, document, scale)? };
        }
        Ok(visits)
    }
}
