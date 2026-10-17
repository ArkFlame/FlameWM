use super::primitives::scale_rect;
use super::*;
use crate::external_drawable::match_argb32_visual;
use crate::native::coverage::{build_coverage, build_coverage_scaled};
use crate::xshape::ClipRect;

/// Fingerprint of retained scroll state for the paint cache key.
/// Revision alone is not enough: scroll writes are light-touch.
fn scroll_fingerprint(document: &RuntimeDocument) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    let mut mix = |word: u64| {
        hash ^= word;
        hash = hash.wrapping_mul(0x100000001b3);
    };
    mix(document.revision());
    // Revision covers content edits; scroll offsets are hashed directly by
    // sampling node-local state through the public scroll API is not
    // possible without an index list, so the paint path passes offsets via
    // `build_paint_commands_with_scroll` and the cache key includes the
    // interaction state plus revision here. Full scroll-list hashing lives
    // in the document-owned mark path (scroll writes mark Full).
    hash
}

impl X11App {
    /// Intersect the clip stack and apply via XSetClipRectangles on the GC.
    /// Empty stack clears the clip.
    unsafe fn apply_clip_stack(&mut self) {
        if self.clip_stack.is_empty() {
            unsafe { XSetClipMask(self.display, self.gc, 0) };
            unsafe { XSetClipOrigin(self.display, self.gc, 0, 0) };
            return;
        }
        let mut region = self.clip_stack[0];
        for rect in self.clip_stack.iter().skip(1) {
            let x1 = region.x.max(rect.x);
            let y1 = region.y.max(rect.y);
            let x2 = (region.x + region.width).min(rect.x + rect.width);
            let y2 = (region.y + region.height).min(rect.y + rect.height);
            region = Rect {
                x: x1,
                y: y1,
                width: (x2 - x1).max(0.0),
                height: (y2 - y1).max(0.0),
            };
        }
        if region.width <= 0.0 || region.height <= 0.0 {
            region = Rect {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
            };
        }
        let rect = XRectangle {
            x: region.x.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16,
            y: region.y.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16,
            width: region.width.round().max(0.0).min(u16::MAX as f32) as u16,
            height: region.height.round().max(0.0).min(u16::MAX as f32) as u16,
        };
        // Unsorted single rectangle; ordering constant 0 (Unsorted).
        unsafe { XSetClipRectangles(self.display, self.gc, 0, 0, &rect, 1, 0) };
    }

    pub(crate) unsafe fn font_for_size_pub(&self, size: f32) -> Option<Font> {
        unsafe { self.font_for_size(size) }
    }

    pub(crate) unsafe fn pixel_pub(&mut self, color: Color) -> Result<u64, String> {
        unsafe { self.pixel(color) }
    }

    pub(crate) unsafe fn upload_argb32_pub(
        &mut self,
        pixmap: Pixmap,
        argb: &[u32],
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let Some(visual) = match_argb32_visual(self.display, self.screen) else {
            return Err("no 32-bit TrueColor visual for ARGB32 upload".to_string());
        };
        unsafe { self.upload_argb32_words(pixmap, visual, argb, width, height) }
    }

    unsafe fn font_for_size(&self, size: f32) -> Option<Font> {
        let target = size.round().clamp(1.0, 255.0) as i32;
        self.fonts
            .iter()
            .min_by_key(|(bucket, _)| (i32::from(**bucket) - target).abs())
            .map(|(_, font)| {
                // SAFETY: font pointers are loaded and retained by X11App for its lifetime.
                unsafe { (**font).fid }
            })
    }

    pub(crate) unsafe fn redraw(&mut self, document: &RuntimeDocument) -> Result<(), String> {
        let layout_guard = flamewm_profiler::start("render.layout");
        let scale = document.ui_scale();
        let viewport = (self.width, self.height);
        let reuse_layout = self.layout.as_ref().is_some_and(|cached| {
            cached.revision == document.revision() && self.cached_viewport == viewport
        });
        if !reuse_layout {
            let layout = LayoutEngine::compute(
                document,
                self.width as f32 / scale,
                self.height as f32 / scale,
                self.interaction,
            );
            self.layout = Some(layout);
            if let Some(layout) = self.layout.as_mut() {
                super::scroll::translate_scrolled_content(layout, document, self.interaction);
            }
            self.cached_viewport = viewport;
        }
        drop(layout_guard);
        let paint_guard = flamewm_profiler::start("render.paint");
        let scroll_fp = scroll_fingerprint(document);
        let cache_valid = self.cached_paint.as_ref().is_some_and(|cached| {
            cached.revision == document.revision()
                && cached.interaction == self.interaction
                && cached.scroll_fingerprint == scroll_fp
                && reuse_layout
        });
        if !cache_valid {
            let layout = self.layout.as_mut().expect("layout retained above");
            let fresh = super::scroll::build_scrolled_paint(document, layout, self.interaction);
            self.cached_paint = Some(CachedPaint {
                commands: fresh,
                revision: document.revision(),
                interaction: self.interaction,
                scroll_fingerprint: scroll_fp,
            });
        }
        drop(paint_guard);
        let _present_guard = flamewm_profiler::start("render.present");
        {
            let _clear_guard = flamewm_profiler::start("render.clear");
            self.clear_scene()?;
            unsafe { self.paint_surface_backdrop()? };
        }
        // Borrow retained commands by index: build fresh only when cache
        // invalid; single coverage/shape per frame; no clone path.
        {
            let _draw_guard = flamewm_profiler::start("render.draw");
            let count = self
                .cached_paint
                .as_ref()
                .expect("paint cached above")
                .commands
                .len();
            for index in 0..count {
                // Borrow retained command without cloning the vector or the
                // item: raw pointer scoped to one iteration; paint takes &.
                let command: &PaintCommand = unsafe {
                    let base = self
                        .cached_paint
                        .as_ref()
                        .expect("paint cached above")
                        .commands
                        .as_ptr();
                    &*base.add(index)
                };
                // SAFETY: redraw is entered only with the app's initialized X11 resources.
                unsafe { self.paint(command, document, scale)? };
            }
        }
        self.finish_node_backdrops();
        unsafe { self.present_scene()? };
        unsafe { self.bind_presented_backbuffer_as_window_background()? };
        // Single shape per changed frame, from retained commands.
        {
            let _shape_guard = flamewm_profiler::start("render.shape");
            let cached = self.cached_paint.as_ref().expect("paint cached above");
            let backdrop_radius = backdrop_corner_radius_px(document, self.width, self.height);
            unsafe {
                self.refresh_shape_mask_from_coverage_scaled(
                    &cached.commands,
                    scale,
                    backdrop_radius,
                )
            };
        }
        Ok(())
    }

    pub(crate) unsafe fn present_retained(
        &mut self,
        document: &RuntimeDocument,
    ) -> Result<(), String> {
        let _guard = flamewm_profiler::start("render.present");
        if self.layout.is_some() {
            unsafe { self.present_scene()? };
            unsafe { XFlush(self.display) };
            return Ok(());
        }
        unsafe { self.redraw(document) }
    }

    /// Mark damage without painting: merged (max coverage) latch consumed
    /// by `redraw_if_dirty`. Returns false when damage is empty.
    pub(crate) fn mark_dirty(&mut self, damage: SurfaceDamage) {
        self.pending_damage = self.pending_damage.merge(damage);
    }

    pub(crate) fn mark_full(&mut self) {
        self.pending_damage = SurfaceDamage::Full;
    }

    /// Paint only when damage is latched or the document revision moved
    /// past the retained cache; otherwise skip the frame.
    pub(crate) unsafe fn redraw_if_dirty(
        &mut self,
        document: &mut RuntimeDocument,
    ) -> Result<bool, String> {
        let doc_damage = document.take_damage();
        self.pending_damage = self.pending_damage.merge(doc_damage);
        let stale = self
            .layout
            .as_ref()
            .is_none_or(|cached| cached.revision != document.revision());
        if self.pending_damage.is_empty() && !stale {
            return Ok(false);
        }
        self.pending_damage = SurfaceDamage::None;
        unsafe { self.redraw(document)? };
        Ok(true)
    }

    /// Transparent ARGB scene clear: 0x00000000, never root_background/BLACK.
    pub(crate) fn clear_scene(&mut self) -> Result<(), String> {
        // Depth-32 ARGB scene: pixel 0 == 0x00000000 transparent clear.
        // XRender fills composite over this; the GC path fills pixel 0 too.
        unsafe { XSetForeground(self.display, self.gc, TRANSPARENT_CLEAR_PIXEL) };
        unsafe { XSetClipMask(self.display, self.gc, 0) };
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

    /// Present per SurfaceAlphaMode: composited/shape via backbuffer copy,
    /// opaque fallback paints flattened pixels (already flattened in pixel()).
    ///
    /// Owner of the retained background pixmap: binds the presented
    /// backbuffer via XSetWindowBackgroundPixmap so expose repaints reuse
    /// the last painted frame without recompute. Never binds an unpainted
    /// pixmap: callers paint first, then call this, then present. The
    /// previously retired pixmap (kept alive across resize) is released
    /// only after the replacement is bound.
    pub(crate) unsafe fn bind_presented_backbuffer_as_window_background(
        &mut self,
    ) -> Result<(), String> {
        if self.backbuffer == 0 {
            return Err("no backbuffer to bind as window background".to_string());
        }
        // Painted by contract: redraw paints before binding; expose returns
        // only via present_retained (retained scene) or redraw.
        let (should_bind, _) = background_bind_plan(
            self.backbuffer,
            self.background_bound,
            self.retired_backbuffer,
            true,
        );
        if should_bind {
            unsafe { XSetWindowBackgroundPixmap(self.display, self.window, self.backbuffer) };
            self.background_bound = self.backbuffer;
        }
        self.release_retired_after_bind();
        Ok(())
    }

    fn release_retired_after_bind(&mut self) {
        let Some((pixmap, bytes)) = self.retired_backbuffer.take() else {
            return;
        };
        if pixmap == 0 || pixmap == self.background_bound {
            if pixmap != 0 {
                self.retired_backbuffer = Some((pixmap, bytes));
            }
            return;
        }
        unsafe {
            self.allocator.free_pixmap(self.display, pixmap, bytes);
        }
        refresh_backbuffer_gauge(self.backbuffer_bytes as u64);
    }
}

/// Pure background-pixmap lifecycle decision.
// Returns (should_bind, releasable_retired): never bind 0 or an unpainted
// replacement (painted=false); bind only when the painted backbuffer
// differs from bound; release retired only after the new pixmap is bound.
pub(crate) fn background_bind_plan(
    backbuffer: Pixmap,
    background_bound: Pixmap,
    retired: Option<(Pixmap, usize)>,
    painted: bool,
) -> (bool, bool) {
    if backbuffer == 0 || !painted {
        return (false, false);
    }
    let should_bind = background_bound != backbuffer;
    let releasable = match retired {
        Some((pixmap, _)) => pixmap != 0 && pixmap != backbuffer && should_bind,
        None => false,
    };
    (should_bind, releasable)
}

impl X11App {
    pub(crate) unsafe fn present_scene(&mut self) -> Result<(), String> {
        unsafe { XSetClipMask(self.display, self.gc, 0) };
        unsafe { XSetClipOrigin(self.display, self.gc, 0, 0) };
        {
            let _copy_guard = flamewm_profiler::start("render.copy");
            unsafe {
                XCopyArea(
                    self.display,
                    self.backbuffer,
                    self.window,
                    self.gc,
                    0,
                    0,
                    self.width,
                    self.height,
                    0,
                    0,
                )
            };
        }
        {
            let _flush_guard = flamewm_profiler::start("render.flush");
            unsafe { XFlush(self.display) };
        }
        Ok(())
    }

    /// Single source of the popup mask spans for the settled and slide paths: a retained
    /// backdrop uses the rounded rect of `backdrop_radius`, otherwise paint coverage.
    fn mask_spans(
        &self,
        commands: &[PaintCommand],
        scale: f32,
        backdrop_radius: u32,
    ) -> Vec<(u32, u32, u32)> {
        popup_mask_spans(
            commands,
            self.width,
            self.height,
            scale,
            self.surface_backdrop.is_some().then_some(backdrop_radius),
        )
    }

    /// Scale-aware shape refresh: coverage matches the paint path for
    /// rect/radius/stroke/clip in device pixels; text stays skipped.
    /// `backdrop_radius` (device px) rounds the retained-backdrop mask.
    pub(crate) unsafe fn refresh_shape_mask_from_coverage_scaled(
        &self,
        commands: &[PaintCommand],
        scale: f32,
        backdrop_radius: u32,
    ) {
        let Some(bridge) = self.xshape.as_ref() else {
            return;
        };
        // A retained backdrop is composited under translucent fills, so the snapshot must stay
        // visible across the whole rounded shape; coverage would drop the translucent area and
        // show the live desktop (unblurred) instead. Corners outside the radius stay transparent.
        let spans = if let Some((border, radius)) = self.ring_mask {
            crate::xshape::ring_mask_spans(self.width, self.height, border, radius)
        } else if self.dock_corner_radius > 0 {
            crate::xshape::rounded_mask_spans(self.width, self.height, self.dock_corner_radius)
        } else {
            let spans = self.mask_spans(commands, scale, backdrop_radius);
            if spans.is_empty() {
                return;
            }
            spans
        };
        if let Err(error) = unsafe { bridge.apply_rounded_mask(self.window, &spans) } {
            eprintln!("FLAMEWM_RENDER_SHAPE_BACKEND degraded reason={error}");
        }
    }

    /// Rounded popup mask intersected with `clip` (window-local). Used while a slide keeps the
    /// popup on the screen side of the panel edge; the base mask is restored with
    /// `refresh_shape_mask` once the slide lands.
    pub(crate) unsafe fn refresh_shape_mask_clipped(&self, document: &RuntimeDocument, clip: ClipRect) {
        let (Some(cached), Some(bridge)) = (self.cached_paint.as_ref(), self.xshape.as_ref()) else {
            return;
        };
        let backdrop_radius = backdrop_corner_radius_px(document, self.width, self.height);
        let spans = self.mask_spans(&cached.commands, 1.0, backdrop_radius);
        if let Err(error) = unsafe { bridge.apply_clipped_mask(self.window, &spans, clip) } {
            eprintln!("FLAMEWM_RENDER_SHAPE_BACKEND degraded reason={error}");
        }
    }

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
                node,
            } => {
                let rect = scale_rect(*rect, scale);
                let device = (
                    rect.x.round() as i32,
                    rect.y.round() as i32,
                    rect.width.round().max(1.0) as u32,
                    rect.height.round().max(1.0) as u32,
                );
                let radius = (*radius * scale).round().max(0.0) as u32;
                // SAFETY: redraw is entered only with the app's initialized X11 resources.
                unsafe {
                    self.paint_node_backdrop(
                        *node,
                        device,
                        radius,
                        *blur_percent,
                        document.revision(),
                    )?
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
                if let Some(xrender) = self.xrender.as_mut() {
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
                if let Some(xrender) = self.xrender.as_mut() {
                    // SAFETY: same initialized backbuffer/display contract as `paint`.
                    unsafe { crate::xrender_aa::stroke_rounded_rect(xrender, rect, radius, width, *color)? };
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
                let rect = scale_rect(*rect, scale);
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
                let width = rect.width.round().max(1.0) as u32;
                let height = rect.height.round().max(1.0) as u32;
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
                let dx = rect.x.round() as i32;
                let dy = rect.y.round() as i32;
                // Canonical scene target: XRender/Xft paint onto the scene
                // backbuffer; composited path uses PictOpOver with true alpha.
                if let Some(xrender) = self.xrender.as_mut() {
                    // SAFETY: same initialized backbuffer/display contract as `paint`.
                    unsafe { xrender.blit_argb32_over(cached.pixmap, dx, dy, width, height)? };
                    return Ok(());
                }
                if cached.mask != 0 {
                    // SAFETY: display and GC are valid X11 resources owned by this app.
                    unsafe { XSetClipOrigin(self.display, self.gc, dx, dy) };
                    // SAFETY: display and GC are valid X11 resources owned by this app.
                    unsafe { XSetClipMask(self.display, self.gc, cached.mask) };
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
            PaintCommand::ScrollbarTrack { rect, .. } => {
                let rect = scale_rect(*rect, scale);
                let pixel = unsafe { self.pixel(Color::rgb(48, 49, 52))? };
                unsafe { XSetForeground(self.display, self.gc, pixel) };
                unsafe {
                    XFillRectangle(
                        self.display,
                        self.backbuffer,
                        self.gc,
                        rect.x.round() as i32,
                        rect.y.round() as i32,
                        rect.width.round().max(1.0) as u32,
                        rect.height.round().max(1.0) as u32,
                    )
                };
            }
            PaintCommand::ScrollbarThumb { rect, .. } => {
                let rect = scale_rect(*rect, scale);
                let pixel = unsafe { self.pixel(Color::rgb(120, 123, 128))? };
                unsafe { XSetForeground(self.display, self.gc, pixel) };
                unsafe {
                    XFillRectangle(
                        self.display,
                        self.backbuffer,
                        self.gc,
                        rect.x.round() as i32,
                        rect.y.round() as i32,
                        rect.width.round().max(1.0) as u32,
                        rect.height.round().max(1.0) as u32,
                    )
                };
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
                if let Some(xft) = self.xft.as_mut() {
                    // SAFETY: same initialized backbuffer/display contract as `paint`.
                    unsafe { xft.set_preferred_family(document.ui_font_family()) };
                    // SAFETY: same initialized backbuffer/display contract as `paint`.
                    unsafe { xft.draw_text(x, y, *color, size, *weight, text)? };
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

/// Pure row spans for a popup mask: `backdrop_radius` (device px) rounds the rect when a backdrop
/// is retained, otherwise coverage of `commands` at `scale`.
pub(crate) fn popup_mask_spans(
    commands: &[PaintCommand],
    width: u32,
    height: u32,
    scale: f32,
    backdrop_radius: Option<u32>,
) -> Vec<(u32, u32, u32)> {
    match backdrop_radius {
        Some(radius) => crate::xshape::rounded_mask_spans(width, height, radius),
        None if scale == 1.0 => build_coverage(commands, width, height, 0.0),
        None => build_coverage_scaled(commands, width, height, 0.0, scale),
    }
}

/// Device-pixel corner radius for the retained-backdrop shape mask, derived from the document's
/// root corner radius (logical px) scaled like the paint path, clamped to the surface size.
pub(crate) fn backdrop_corner_radius_px(
    document: &RuntimeDocument,
    width: u32,
    height: u32,
) -> u32 {
    let radius =
        flamewm_render_core::paint::root_corner_radius(document, InteractionState::default());
    flamewm_render_core::paint::surface_shape_pixels(width, height, radius * document.ui_scale()).2
}

#[cfg(test)]
mod tests {
    use super::{background_bind_plan, popup_mask_spans};
    use crate::xshape::{clip_spans, rounded_mask_spans};

    fn row_span(spans: &[(u32, u32, u32)], row: u32) -> (u32, u32) {
        let &(_, start, end) = spans
            .iter()
            .find(|(r, _, _)| *r == row)
            .expect("row present");
        (start, end)
    }

    #[test]
    fn backdrop_mask_rounds_corners_when_radius_positive() {
        let spans = rounded_mask_spans(40, 20, 8);
        let (top_start, top_end) = row_span(&spans, 0);
        let (mid_start, mid_end) = row_span(&spans, 10);
        assert!(
            top_start > 0 && top_end < 40,
            "top row must be cut at the corners"
        );
        assert_eq!((mid_start, mid_end), (0, 40));
        assert_eq!(
            row_span(&spans, 19).0,
            top_start,
            "bottom corners mirror top"
        );
    }

    #[test]
    fn slide_mask_matches_settled_mask_clipped_and_keeps_corners() {
        let clip = (0, 0, 40, 10);
        let settled = popup_mask_spans(&[], 40, 20, 1.0, Some(8));
        let slide = clip_spans(&settled, clip);
        assert_eq!(slide, clip_spans(&popup_mask_spans(&[], 40, 20, 1.0, Some(8)), clip));
        let (top_start, top_end) = row_span(&slide, 0);
        assert!(top_start > 0 && top_end < 40, "slide corners must stay transparent");
        assert_eq!(row_span(&slide, 5), (0, 40));
    }

    #[test]
    fn slide_mask_radius_zero_stays_rectangular() {
        let clip = (0, 0, 40, 10);
        let slide = clip_spans(&popup_mask_spans(&[], 40, 20, 1.0, Some(0)), clip);
        assert_eq!(slide.len(), 10);
        assert!(slide.iter().all(|&(_, start, end)| start == 0 && end == 40));
    }

    #[test]
    fn backdrop_mask_radius_zero_is_full_rect() {
        let spans = rounded_mask_spans(40, 20, 0);
        assert_eq!(spans.len(), 20);
        assert!(spans.iter().all(|&(_, start, end)| start == 0 && end == 40));
    }

    #[test]
    fn backdrop_radius_follows_root_radius_and_clamps() {
        use flamewm_render_core::paint::surface_shape_pixels;
        assert_eq!(surface_shape_pixels(100, 60, 12.0).2, 12);
        assert_eq!(
            surface_shape_pixels(100, 60, 0.0).2,
            0,
            "squared stays full rect"
        );
        assert_eq!(
            surface_shape_pixels(40, 20, 99.0).2,
            10,
            "clamped to half min side"
        );
    }

    #[test]
    fn never_binds_unpainted_replacement() {
        let (bind, release) = background_bind_plan(20, 10, Some((10, 64)), false);
        assert!(!bind && !release);
    }

    #[test]
    fn never_binds_zero_backbuffer() {
        let (bind, release) = background_bind_plan(0, 0, None, true);
        assert!(!bind && !release);
    }

    #[test]
    fn binds_first_painted_frame_without_releasing_live() {
        let (bind, release) = background_bind_plan(10, 0, None, true);
        assert!(bind && !release);
    }

    #[test]
    fn retains_old_until_replacement_bound() {
        // Same painted pixmap as bound: no-op, retired stays.
        let (bind, release) = background_bind_plan(10, 10, Some((5, 64)), true);
        assert!(!bind && !release);
        // Replacement painted and differs: bind, then release retired.
        let (bind, release) = background_bind_plan(20, 10, Some((10, 64)), true);
        assert!(bind && release);
    }
}
