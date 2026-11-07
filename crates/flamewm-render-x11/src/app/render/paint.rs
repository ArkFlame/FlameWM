use super::*;

impl X11App {
    pub(crate) unsafe fn paint(
        &mut self,
        command: &PaintCommand,
        document: &RuntimeDocument,
        scale: f32,
    ) -> Result<(), String> {
        match command {
            PaintCommand::NodeBackdrop {
                rect,
                radius,
                blur_percent,
                opacity,
                node,
            } => {
                let device = super::surface_backdrop::node_device_rect(*rect, scale);
                let radius = (*radius * scale).round().max(0.0) as u32;
                // SAFETY: redraw is entered only with the app's initialized X11 resources.
                unsafe {
                    self.paint_node_backdrop(*node, device, radius, *blur_percent, *opacity)?
                };
            }
            PaintCommand::FillRect {
                rect,
                color,
                radius,
            } => {
                let rect = scale_rect(*rect, scale);
                let radius = *radius * scale;
                // Canonical scene target: XRender handles all alpha; the GC
                // path only runs in explicit OpaqueFallback (flattened pixel).
                // XRender ignores the GC clip stack: cut the fill to it. A cut fill keeps
                // straight edges (its corner radius is dropped at the clip edge).
                let visible = visible_under(rect, &self.clip_stack);
                // A partly clipped fill keeps its corner radius through a picture clip. Region
                // paints own the destination clip, so the clip is the part intersected with the
                // region and the restore goes back to the region, never the whole window.
                let region_rect = self.region_text_clip.map(|region| Rect {
                    x: region.x as f32,
                    y: region.y as f32,
                    width: region.width as f32,
                    height: region.height as f32,
                });
                let (full_width, full_height) = (self.width, self.height);
                let restore_rect = region_rect.unwrap_or(Rect {
                    x: 0.0,
                    y: 0.0,
                    width: full_width as f32,
                    height: full_height as f32,
                });
                if let Some(xrender) = self.xrender.as_mut() {
                    let (rect, radius, picture_clip) = match visible {
                        Visible::Hidden => return Ok(()),
                        Visible::Whole => (rect, radius, None),
                        Visible::Part(part) => match partial_fill_clip(part, region_rect) {
                            None => return Ok(()),
                            Some(clip) => (rect, radius, Some(clip)),
                        },
                    };
                    if let Some(part) = picture_clip {
                        let to_clip = |r: Rect| XRectangle {
                            x: r.x.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16,
                            y: r.y.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16,
                            width: r.width.round().max(0.0).min(u16::MAX as f32) as u16,
                            height: r.height.round().max(0.0).min(u16::MAX as f32) as u16,
                        };
                        // SAFETY: same initialized backbuffer/display contract as `paint`.
                        unsafe { xrender.set_destination_clip(to_clip(part))? };
                        // SAFETY: same contract; the clip is restored below on every path.
                        let filled = unsafe {
                            crate::xrender_aa::fill_rounded_rect(xrender, rect, radius, *color)
                        };
                        let reset = unsafe { xrender.set_destination_clip(to_clip(restore_rect)) };
                        filled?;
                        reset?;
                        return Ok(());
                    }
                    // SAFETY: same initialized backbuffer/display contract as `paint`.
                    unsafe { crate::xrender_aa::fill_rounded_rect(xrender, rect, radius, *color)? };
                    return Ok(());
                }
                if self.alpha_mode != SurfaceAlphaMode::OpaqueFallback && color.a < 255 {
                    return Err(
                        "translucent fill without XRender outside OpaqueFallback".to_string()
                    );
                }
                {
                    // SAFETY: the app's display and colormap are initialized for this draw.
                    let pixel = unsafe { self.pixel(*color)? };
                    // SAFETY: display and GC are valid X11 resources owned by this app.
                    unsafe { XSetForeground(self.display, self.gc, pixel) };
                    // SAFETY: the app's display, GC, and backbuffer are valid for this draw.
                    unsafe { self.fill_rounded_rect(rect, radius) };
                }
            }
            PaintCommand::StrokeRect {
                rect,
                color,
                width,
                radius,
            } => {
                let rect = scale_rect(*rect, scale);
                let width = *width * scale;
                let radius = *radius * scale;
                // XRender ignores the GC clip stack; the AA painter clips rounded
                // edge runs explicitly without replacing their arc with square bands.
                let visible = visible_under(rect, &self.clip_stack);
                if let Some(xrender) = self.xrender.as_mut() {
                    match visible {
                        Visible::Hidden => {}
                        Visible::Whole => {
                            // SAFETY: same initialized backbuffer/display contract as `paint`.
                            unsafe {
                                crate::xrender_aa::stroke_rounded_rect(
                                    xrender, rect, radius, width, *color,
                                )?
                            };
                        }
                        Visible::Part(part) => {
                            // Clip the native AA ring rather than replacing its curved
                            // corners with square rectangular border bands.
                            unsafe {
                                crate::xrender_aa::stroke_rounded_rect_clipped(
                                    xrender,
                                    rect,
                                    radius,
                                    width,
                                    *color,
                                    Some(part),
                                )?
                            };
                        }
                    }
                    return Ok(());
                }
                if self.alpha_mode != SurfaceAlphaMode::OpaqueFallback && color.a < 255 {
                    return Err(
                        "translucent stroke without XRender outside OpaqueFallback".to_string()
                    );
                }
                {
                    // SAFETY: the app's display and colormap are initialized for this draw.
                    let pixel = unsafe { self.pixel(*color)? };
                    // SAFETY: display and GC are valid X11 resources owned by this app.
                    unsafe { XSetForeground(self.display, self.gc, pixel) };
                    let repeats = width.round().clamp(1.0, 8.0) as i32;
                    for inset in 0..repeats {
                        // SAFETY: the app's display, GC, and backbuffer are valid for this draw.
                        unsafe { self.stroke_rounded_rect(rect, radius, inset) };
                    }
                }
            }
            PaintCommand::Image {
                rect,
                asset,
                node,
                revision,
                treatment,
                tint,
            } => {
                let (dx, dy, width, height) = snap_device_rect(*rect, scale);
                let Some((compiled_asset, src_w, src_h, _, pixels)) =
                    document.image_for_node(*node)
                else {
                    return Err(format!("paint references missing image for node {node}"));
                };
                let asset_id = compiled_asset.unwrap_or(*asset);
                let expected_revision = document.image_revision_for_node(*node);
                debug_assert_eq!(
                    *revision, expected_revision,
                    "stale image paint command for node {node}"
                );
                // SAFETY: image_pixmap uses the app's initialized X11 resources and cache.
                let cached = unsafe {
                    self.image_pixmap(
                        *node,
                        asset_id,
                        pixels,
                        src_w,
                        src_h,
                        expected_revision,
                        width,
                        height,
                        *treatment,
                        *tint,
                    )?
                };
                // Canonical scene target: XRender/Xft paint onto the scene
                // backbuffer; composited path uses PictOpOver with true alpha.
                // XRender ignores the GC clip stack. A blit cannot be cropped (no source
                // offset in the XRender backend), so an image cut by a clip is skipped.
                let visible = visible_under(
                    Rect {
                        x: dx as f32,
                        y: dy as f32,
                        width: width as f32,
                        height: height as f32,
                    },
                    &self.clip_stack,
                );
                // A region repaint copies only the image rows and columns inside the region.
                let crop = self.region_text_clip.map(|region| {
                    let x0 = dx.max(region.x);
                    let y0 = dy.max(region.y);
                    let x1 = (dx + width as i32).min(region.x + region.width as i32);
                    let y1 = (dy + height as i32).min(region.y + region.height as i32);
                    (x0, y0, x1, y1)
                });
                if let Some(xrender) = self.xrender.as_mut() {
                    if visible == Visible::Whole {
                        match crop {
                            None => {
                                // SAFETY: same initialized backbuffer/display contract as `paint`.
                                unsafe {
                                    xrender.blit_argb32_over(
                                        cached.pixmap,
                                        dx,
                                        dy,
                                        width,
                                        height,
                                    )?
                                };
                            }
                            Some((x0, y0, x1, y1)) if x1 > x0 && y1 > y0 => {
                                // SAFETY: same initialized backbuffer/display contract as `paint`.
                                unsafe {
                                    xrender.blit_argb32_over_from(
                                        cached.pixmap,
                                        (x0 - dx, y0 - dy),
                                        (x0, y0),
                                        (x1 - x0) as u32,
                                        (y1 - y0) as u32,
                                    )?
                                };
                            }
                            Some(_) => {}
                        }
                    }
                    return Ok(());
                }
                if cached.mask != 0 {
                    // SAFETY: display and GC are valid X11 resources owned by this app.
                    unsafe { XSetClipOrigin(self.display, self.gc, dx, dy) };
                    // SAFETY: display and GC are valid X11 resources owned by this app.
                    unsafe { XSetClipMask(self.display, self.gc, cached.mask) };
                    self.gc_clip = None;
                }
                // SAFETY: cached pixmap, backbuffer, display, and GC are valid X11 resources.
                unsafe {
                    XCopyArea(
                        self.display,
                        cached.pixmap,
                        self.backbuffer,
                        self.gc,
                        0,
                        0,
                        width,
                        height,
                        dx,
                        dy,
                    )
                };
                if cached.mask != 0 {
                    // SAFETY: display and GC are valid X11 resources owned by this app.
                    unsafe { XSetClipMask(self.display, self.gc, 0) };
                    self.gc_clip = None;
                    // SAFETY: display and GC are valid X11 resources owned by this app.
                    unsafe { XSetClipOrigin(self.display, self.gc, 0, 0) };
                }
            }
            PaintCommand::PushClip { rect } => {
                let rect = scale_rect(*rect, scale);
                self.clip_stack.push(rect);
                unsafe { self.apply_clip_stack() };
            }
            PaintCommand::PopClip => {
                self.clip_stack.pop();
                unsafe { self.apply_clip_stack() };
            }
            // Overlay scrollbar, same look as the explorer's file-list thumb: no reserved track,
            // a slim translucent rounded thumb 2px in from the edge, drawn over the entries.
            PaintCommand::ScrollbarTrack { .. } => {}
            PaintCommand::ScrollbarThumb { rect, horizontal } => {
                let mut thumb = *rect;
                if *horizontal {
                    thumb.y += thumb.height - 8.0;
                    thumb.height = 6.0;
                } else {
                    thumb.x += thumb.width - 8.0;
                    thumb.width = 6.0;
                }
                let rect = scale_rect(thumb, scale);
                let radius = 3.0 * scale;
                let color = Color {
                    r: 255,
                    g: 255,
                    b: 255,
                    a: 0x48,
                };
                if let Some(xrender) = self.xrender.as_mut() {
                    let (rect, radius) = match visible_under(rect, &self.clip_stack) {
                        Visible::Hidden => return Ok(()),
                        Visible::Whole => (rect, radius),
                        Visible::Part(part) => (part, 0.0),
                    };
                    // SAFETY: same initialized backbuffer/display contract as `paint`.
                    unsafe { crate::xrender_aa::fill_rounded_rect(xrender, rect, radius, color)? };
                }
            }
            PaintCommand::TextShadow {
                x,
                y,
                color,
                size,
                weight,
                blur_radius,
                text,
            } => {
                let x = *x * scale;
                let y = *y * scale;
                let size = *size * scale;
                let radius = (*blur_radius * scale).clamp(0.0, 8.0);
                // Xft/XRender may not be available on minimal X servers; an offset
                // glyph is the compatibility fallback, not the primary blur path.
                if !self.paint_text_shadow(
                    x,
                    y,
                    *color,
                    size,
                    *weight,
                    radius,
                    text,
                    document.ui_font_family(),
                )? {
                    // Preserve the legacy offset glyph if Xft A8 or XRender is unavailable.
                    let fallback = PaintCommand::Text {
                        x: x / scale,
                        y: y / scale,
                        color: *color,
                        size: size / scale,
                        weight: *weight,
                        text: text.clone(),
                    };
                    unsafe { self.paint(&fallback, document, scale)? };
                }
            }
            PaintCommand::Text {
                x,
                y,
                color,
                size,
                weight,
                text,
            } => {
                let x = *x * scale;
                let y = *y * scale;
                let size = *size * scale;
                // Xft keeps its own draw clip, so the GC clip stack never reaches glyphs.
                // Cut to the stack bounds; a stack with no common area draws nothing.
                let stack_clip = match super::clip::stack_bounds(&self.clip_stack) {
                    _ if self.clip_stack.is_empty() => None,
                    None => return Ok(()),
                    Some(b) => Some((
                        b.x.round() as i32,
                        b.y.round() as i32,
                        b.width.round().max(0.0) as u32,
                        b.height.round().max(0.0) as u32,
                    )),
                };
                // Region repaint: glyphs outside the region are left to the rest of the frame.
                let text_clip = match (stack_clip, self.region_text_clip) {
                    (clip, None) => clip,
                    (None, Some(region)) => Some((region.x, region.y, region.width, region.height)),
                    (Some((x, y, w, h)), Some(region)) => {
                        let x0 = x.max(region.x);
                        let y0 = y.max(region.y);
                        let x1 = (x + w as i32).min(region.x + region.width as i32);
                        let y1 = (y + h as i32).min(region.y + region.height as i32);
                        if x1 <= x0 || y1 <= y0 {
                            return Ok(());
                        }
                        Some((x0, y0, (x1 - x0) as u32, (y1 - y0) as u32))
                    }
                };
                self.ensure_xft();
                if let Some(xft) = self.xft.as_mut() {
                    // SAFETY: same initialized backbuffer/display contract as `paint`.
                    unsafe { xft.set_preferred_family(document.ui_font_family()) };
                    // SAFETY: same initialized backbuffer/display contract as `paint`.
                    unsafe { xft.set_clip(text_clip) };
                    // SAFETY: same initialized backbuffer/display contract as `paint`.
                    let drawn = unsafe { xft.draw_text(x, y, *color, size, *weight, text) };
                    // SAFETY: same initialized backbuffer/display contract as `paint`.
                    unsafe { xft.set_clip(None) };
                    drawn?;
                } else {
                    // SAFETY: the app's display and colormap are initialized for this draw.
                    let pixel = unsafe { self.pixel(*color)? };
                    // SAFETY: display and GC are valid X11 resources owned by this app.
                    unsafe { XSetForeground(self.display, self.gc, pixel) };
                    // SAFETY: font pointers are loaded and retained by X11App for its lifetime.
                    if let Some(font) = unsafe { self.font_for_size(size) } {
                        // SAFETY: display and GC are valid X11 resources owned by this app.
                        unsafe { XSetFont(self.display, self.gc, font) };
                    }
                    let string = if text.is_ascii() {
                        CString::new(text.as_bytes())
                    } else {
                        CString::new(
                            text.chars()
                                .map(|ch| if ch.is_ascii() { ch } else { '?' })
                                .collect::<String>(),
                        )
                    }
                    .map_err(|_| "text contains NUL".to_string())?;
                    let len = string.as_bytes().len().min(i32::MAX as usize) as i32;
                    // SAFETY: display, backbuffer, and GC are valid; string remains alive for the call.
                    unsafe {
                        XDrawString(
                            self.display,
                            self.backbuffer,
                            self.gc,
                            x.round() as i32,
                            y.round() as i32,
                            string.as_ptr(),
                            len,
                        )
                    };
                }
            }
        }
        Ok(())
    }
}
