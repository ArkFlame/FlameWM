use super::clip::{Visible, intersect, visible_under};

/// Picture clip for a partly visible fill: the visible part, cut to the active region paint
/// when there is one. `None` means nothing of the fill lies inside the region.
fn partial_fill_clip(part: Rect, region: Option<Rect>) -> Option<Rect> {
    match region {
        None => Some(part),
        Some(region) => intersect(part, region),
    }
}
use super::damage_plan::{PixelRegion, RedrawPlan, RegionInput, command_bounds, plan_region_paint};
use flamewm_render_core::ImageTreatment;

use super::gpu_frame::{self, FrameSource};
use super::primitives::{scale_rect, snap_device_rect};
use super::*;
use crate::native::coverage::{build_coverage, build_coverage_scaled};
use crate::xshape::ClipRect;
use flamewm_image_core::blur::BlurScratch;

/// Overlap of two rectangles, `None` when they do not share an area.
fn intersect_rect(a: Rect, b: Rect) -> Option<Rect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.width).min(b.x + b.width);
    let y1 = (a.y + a.height).min(b.y + b.height);
    (x1 > x0 && y1 > y0).then_some(Rect {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    })
}

/// Per-thread shadow work buffers. The display thread is the only shadow painter,
/// so every text shadow reuses the blur scratch and staging buffers instead of
/// reallocating them per paint.
#[derive(Default)]
struct ShadowScratch {
    blur: BlurScratch,
    rgba: Vec<u8>,
    argb: Vec<u32>,
    mask: Vec<u8>,
}

thread_local! {
    static SHADOW_SCRATCH: std::cell::RefCell<ShadowScratch> = std::cell::RefCell::default();
}

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

/// Only shapes independent of paint commands may skip coverage reconstruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StaticShapeKey {
    Ring {
        size: (u32, u32),
        border: u32,
        radius: u32,
    },
    Dock {
        size: (u32, u32),
        radius: u32,
    },
}

fn fixed_shape_unchanged(
    last: Option<StaticShapeKey>,
    current: Option<StaticShapeKey>,
    server_shape_valid: bool,
) -> bool {
    current.is_some() && last == current && server_shape_valid
}

fn static_shape_key(
    width: u32,
    height: u32,
    ring: Option<(u32, u32)>,
    dock_radius: u32,
) -> Option<StaticShapeKey> {
    if let Some((border, radius)) = ring {
        Some(StaticShapeKey::Ring {
            size: (width, height),
            border,
            radius,
        })
    } else if dock_radius > 0 {
        Some(StaticShapeKey::Dock {
            size: (width, height),
            radius: dock_radius,
        })
    } else {
        None
    }
}

#[cfg(test)]
mod static_shape_tests {
    use super::*;

    #[test]
    fn static_shape_changes_on_resize_or_style_and_dynamic_masks_are_not_memoized() {
        let dock = static_shape_key(1920, 50, None, 12).unwrap();
        assert_eq!(dock, static_shape_key(1920, 50, None, 12).unwrap());
        assert_ne!(dock, static_shape_key(1920, 49, None, 12).unwrap());
        assert_ne!(dock, static_shape_key(1920, 50, None, 13).unwrap());
        assert_ne!(dock, static_shape_key(1920, 50, Some((2, 12)), 12).unwrap());
        assert_ne!(
            static_shape_key(1920, 50, Some((2, 12)), 12),
            static_shape_key(1920, 50, Some((3, 12)), 12)
        );
        assert_eq!(static_shape_key(1920, 50, None, 0), None);
    }

    #[test]
    fn invalidated_shape_latch_does_not_skip_reapplication() {
        let key = static_shape_key(800, 48, None, 12);
        assert!(fixed_shape_unchanged(key, key, true));
        // A slide or an overlay mask writer invalidates the server shape latch.
        assert!(!fixed_shape_unchanged(key, key, false));
        assert!(!fixed_shape_unchanged(
            key,
            static_shape_key(801, 48, None, 12),
            true,
        ));
        assert!(!fixed_shape_unchanged(key, None, true));
    }
}

impl X11App {
    /// Blurred shadow for one text run: the canonical Xft glyph coverage, then the shared bounded
    /// Gaussian blur, left as straight RGBA in `scratch.rgba`. Returns the device placement, or
    /// `None` when Xft cannot draw the glyphs.
    #[allow(clippy::too_many_arguments)]
    fn text_shadow_rgba(
        &mut self,
        scratch: &mut ShadowScratch,
        x: f32,
        baseline: f32,
        color: Color,
        size: f32,
        weight: u16,
        radius: f32,
        text: &str,
        family: &str,
    ) -> Result<Option<(i32, i32, u32, u32)>, String> {
        self.ensure_xft();
        if self.xrender.is_none() || self.xft.is_none() {
            return Ok(None);
        }
        let pad = (radius * 3.0).ceil() as u32 + 4;
        let dims = {
            let xft = self.xft.as_mut().expect("checked Xft");
            unsafe { xft.set_preferred_family(family) };
            unsafe { xft.text_alpha_mask(size, weight, text, pad, &mut scratch.mask) }
        };
        let Ok((w, h)) = dims else {
            return Ok(None);
        };
        let ShadowScratch {
            blur, rgba, mask, ..
        } = scratch;
        rgba.clear();
        rgba.resize(w as usize * h as usize * 4, 0);
        for (out, &coverage) in rgba.chunks_exact_mut(4).zip(mask.iter()) {
            out.copy_from_slice(&[
                color.r,
                color.g,
                color.b,
                ((u16::from(coverage) * u16::from(color.a) + 127) / 255) as u8,
            ]);
        }
        flamewm_image_core::blur::gaussian_blur_rgba_with(blur, rgba, w, h, radius)
            .map_err(|e| format!("text shadow blur: {e:?}"))?;
        let dx = x.round() as i32 - pad as i32 - 4;
        let dy = baseline.round() as i32 - pad as i32 - size.ceil() as i32;
        Ok(Some((dx, dy, w, h)))
    }

    /// Soft alpha shadow uses the canonical Xft glyph coverage once, the shared
    /// bounded Gaussian blur, then a transient premultiplied ARGB32 XRender blit.
    #[allow(clippy::too_many_arguments)]
    fn paint_text_shadow(
        &mut self,
        x: f32,
        baseline: f32,
        color: Color,
        size: f32,
        weight: u16,
        radius: f32,
        text: &str,
        family: &str,
    ) -> Result<bool, String> {
        let mut scratch = SHADOW_SCRATCH.take();
        let placed = self.text_shadow_rgba(
            &mut scratch,
            x,
            baseline,
            color,
            size,
            weight,
            radius,
            text,
            family,
        );
        let (dx, dy, w, h) = match placed {
            Ok(Some(dims)) => dims,
            other => {
                SHADOW_SCRATCH.with(|slot| slot.replace(scratch));
                return other.map(|_| false);
            }
        };
        let ShadowScratch { rgba, argb, .. } = &mut scratch;
        // The XRender target does not inherit the GC clip stack. Keep only the
        // pixels inside every retained clip (intersection of the clip bounds);
        // the rest is transparent in the premultiplied upload.
        let (mut min_x, mut min_y) = (i32::MIN, i32::MIN);
        let (mut max_x, mut max_y) = (i32::MAX, i32::MAX);
        for clip in &self.clip_stack {
            min_x = min_x.max(clip.x.floor() as i32);
            min_y = min_y.max(clip.y.floor() as i32);
            max_x = max_x.min((clip.x + clip.width).ceil() as i32);
            max_y = max_y.min((clip.y + clip.height).ceil() as i32);
        }
        let Some(visual) = self.argb_visual else {
            SHADOW_SCRATCH.with(|slot| slot.replace(scratch));
            return Ok(false);
        };
        crate::native::image::premultiply_masked_into(argb, rgba, w, h, |xx, yy| {
            let gx = dx + xx as i32;
            let gy = dy + yy as i32;
            gx >= min_x && gy >= min_y && gx < max_x && gy < max_y
        });
        let root = unsafe { XRootWindow(self.display, self.screen) };
        let (pixmap, bytes) = self.alloc_pixmap(root, w, h, 32)?;
        let result = (|| {
            unsafe { self.upload_argb32_words(pixmap, visual, argb, w, h)? };
            if let Some(render) = self.xrender.as_mut() {
                unsafe { render.blit_argb32_over(pixmap, dx, dy, w, h)? };
            }
            Ok::<_, String>(())
        })();
        self.free_pixmap_tracked(pixmap, bytes);
        SHADOW_SCRATCH.with(|slot| slot.replace(scratch));
        result.map(|()| true)
    }
}

/// Logs each distinct XRender fallback reason once, so the log holds the census of blockers.
fn log_gpu_fallback(reason: &str) {
    static SEEN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let Ok(mut seen) = SEEN.lock() else { return };
    if !seen.iter().any(|known| known == reason) {
        eprintln!("FLAMEWM_RENDER_GPU_FRAME xrender fallback {reason}");
        seen.push(reason.to_string());
    }
}

impl X11App {
    /// Intersect the clip stack and apply via XSetClipRectangles on the GC.
    /// Empty stack clears the clip.
    unsafe fn apply_clip_stack(&mut self) {
        if self.clip_stack.is_empty() {
            if self.gc_clip != Some(None) {
                unsafe { XSetClipMask(self.display, self.gc, 0) };
                unsafe { XSetClipOrigin(self.display, self.gc, 0, 0) };
                self.gc_clip = Some(None);
            }
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
        let key = Some((rect.x, rect.y, rect.width, rect.height));
        if self.gc_clip == Some(key) {
            return;
        }
        // Unsorted single rectangle; ordering constant 0 (Unsorted).
        unsafe { XSetClipRectangles(self.display, self.gc, 0, 0, &rect, 1, 0) };
        self.gc_clip = Some(key);
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
        let Some(visual) = self.argb_visual else {
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
        // Only `redraw_if_dirty` may mark a redraw as interaction-only; a direct call (resize,
        // RepaintFull) leaves this false so an unchanged command list still repaints.
        let interaction_only = std::mem::take(&mut self.skip_eligible);
        let layout_guard = flamewm_profiler::start("render.layout");
        let scale = document.ui_scale();
        let layout_key = LayoutKey {
            layout_revision: document.layout_revision(),
            geometry_revision: document.geometry_revision(),
            viewport: (self.width, self.height),
            scale_bits: scale.to_bits(),
        };
        let previous_key = self.cached_layout_key;
        let reuse_layout = self.layout.is_some() && self.cached_layout_key == Some(layout_key);
        if !reuse_layout {
            // Same viewport, scale and layout revision: only isolated geometry moved since the
            // retained layout, so patch its boxes in place. Otherwise recompute and fit scrolls.
            let same_base = previous_key.is_some_and(|key| {
                key.layout_revision == layout_key.layout_revision
                    && key.viewport == layout_key.viewport
                    && key.scale_bits == layout_key.scale_bits
            });
            let patched = same_base
                && self
                    .layout
                    .as_mut()
                    .is_some_and(|layout| document.patch_layout(layout, self.interaction));
            if !patched {
                let layout = LayoutEngine::compute(
                    document,
                    self.width as f32 / scale,
                    self.height as f32 / scale,
                    self.interaction,
                );
                self.layout = Some(layout);
                if let Some(layout) = self.layout.as_mut() {
                    super::scroll::fit_scroll_extents(layout, document, self.interaction);
                    super::scroll::translate_scrolled_content(layout, document, self.interaction);
                }
            }
            self.cached_layout_key = Some(layout_key);
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
        // Before planning: an invalidated window backdrop must already count as changed.
        // SAFETY: redraw runs on the display thread; the snapshot is laid after the clear below.
        unsafe { self.refresh_window_backdrop() };
        let force_full = std::mem::take(&mut self.region_force_full);
        let base_valid = std::mem::take(&mut self.region_base_valid);
        let backdrop_unchanged = self.window_backdrop_unchanged();
        let mut plan = RedrawPlan::Full;
        if !cache_valid {
            let layout = self.layout.as_mut().expect("layout retained above");
            let fresh = super::scroll::build_scrolled_paint_cached(
                document,
                layout,
                self.interaction,
                &mut self.paint_nodes,
            );
            let previous = self.cached_paint.take();
            plan = plan_region_paint(RegionInput {
                old: previous.as_ref().map(|cached| cached.commands.as_slice()),
                new: &fresh,
                scale,
                viewport: (self.width, self.height),
                enabled: self.region_paint,
                viewport_unchanged: previous_key.is_some_and(|key| {
                    key.viewport == (self.width, self.height) && key.scale_bits == scale.to_bits()
                }),
                backdrops: self.surface_backdrop.is_some()
                    || (self.window_backdrop_pixmap.is_some() && !backdrop_unchanged)
                    || !self.clip_stack.is_empty(),
                translucent: backdrop_unchanged && self.surface_backdrop.is_none(),
                interaction_only,
                full_latched: force_full,
                base_valid,
            });
            self.cached_paint = Some(CachedPaint {
                commands: fresh,
                revision: document.revision(),
                interaction: self.interaction,
                scroll_fingerprint: scroll_fp,
            });
        }
        drop(paint_guard);
        // The rebuilt scene is identical to the one already in the backbuffer, so there is nothing to
        // paint and nothing to present. Without this, a pointer move over a node with no hover style
        // redraws the whole window to identical pixels.
        if plan == RedrawPlan::Skip {
            // The backbuffer still holds exactly these commands, so it is still a valid base for a
            // later region paint (`base_valid` above was taken, not read).
            self.region_base_valid = true;
            self.render_counters
                .record(RedrawPlan::Skip, (self.width, self.height), 0, false);
            return Ok(());
        }
        let _present_guard = flamewm_profiler::start("render.present");
        // A failed region paint leaves the backbuffer partly written; the Full path below
        // clears and repaints every pixel, so it is the safe fallback.
        let mut region_done = None;
        if let RedrawPlan::Region(region) = plan {
            self.region_base_valid = false;
            match unsafe { self.paint_region(region, document, scale) } {
                Ok(visits) => region_done = Some((plan, visits)),
                Err(error) => eprintln!("FLAMEWM_RENDER_REGION fallback reason={error}"),
            }
        }
        let (used, visits) = match region_done {
            Some(done) => done,
            None => (RedrawPlan::Full, unsafe {
                self.paint_full(document, scale)?
            }),
        };
        unsafe { self.bind_presented_backbuffer_as_window_background()? };
        self.presented_size = Some((self.width, self.height));
        if self.role == X11WindowRole::Desktop {
            unsafe { self.publish_desktop_frame() };
        }
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
        self.render_counters
            .record(used, (self.width, self.height), visits, true);
        self.record_window_backdrop_painted();
        self.region_base_valid = true;
        Ok(())
    }

    /// Full clear, surface backdrop, every command, node backdrops, then present.
    /// Whole-frame GPU paint for `renderAcceleration=gpu`: one upload of the transparent scene,
    /// one readback, then one ARGB upload to the backbuffer. `None` keeps the XRender path, which
    /// also takes every GPU failure, so a frame never mixes the two painters.
    unsafe fn paint_full_gpu(&mut self, document: &RuntimeDocument, scale: f32) -> Option<u64> {
        let gpu = crate::accel::render_acceleration() == crate::accel::RenderAcceleration::Gpu;
        let scene_ready = self.xrender.is_some() && self.argb_visual.is_some();
        if !gpu_frame::lowering_allowed(gpu, scene_ready) {
            return None;
        }
        // The commands leave `self` while text masks and images are read from the app, then go
        // back, so a `None` below never drops the retained scene.
        let cached = self.cached_paint.take()?;
        let count = cached.commands.len() as u64;
        let rgba = self.gpu_frame_pixels(&cached.commands, document, scale);
        self.cached_paint = Some(cached);
        let rgba = rgba?;
        let (width, height) = (self.width, self.height);
        // Over the transparent scene the ops already leave premultiplied colour, which is what
        // the ARGB32 words hold, so the bytes pack as they are.
        let words: Vec<u32> = rgba
            .chunks_exact(4)
            .map(|p| u32::from_be_bytes([p[3], p[0], p[1], p[2]]))
            .collect();
        // SAFETY: the backbuffer and the ARGB visual belong to this app; checked above. A depth-32
        // backbuffer takes the ARGB words; any other depth takes the same premultiplied colour as an
        // opaque image, as XRender would leave it over the cleared scene.
        let uploaded = if self.depth == 32 {
            unsafe { self.upload_argb32_pub(self.backbuffer, &words, width, height) }
        } else {
            // SAFETY: the default visual and the backbuffer belong to this app.
            let visual = unsafe { XDefaultVisual(self.display, self.screen) };
            unsafe {
                super::images::upload_words_image(
                    self.display,
                    self.backbuffer,
                    visual,
                    self.depth as u32,
                    &words,
                    width,
                    height,
                    "GPU frame upload",
                )
            }
            .or_else(|_| unsafe {
                self.upload_opaque_fallback(self.backbuffer, &rgba, width, height, width, height)
            })
        };
        uploaded
            .ok()
            .map(|()| {
                static LOGGED: std::sync::atomic::AtomicBool =
                    std::sync::atomic::AtomicBool::new(false);
                if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    eprintln!(
                        "FLAMEWM_RENDER_GPU_FRAME whole-frame gpu paint active ({width}x{height})"
                    );
                }
                count
            })
    }

    /// The RGBA8 pixels of one whole frame on the GPU, or `None` when a command keeps the XRender
    /// path or the GPU batch fails. Text masks and images come from the same sources the XRender
    /// painter uses, so the GPU frame composites the same coverage and pixels.
    fn gpu_frame_pixels(
        &mut self,
        commands: &[PaintCommand],
        document: &RuntimeDocument,
        scale: f32,
    ) -> Option<Vec<u8>> {
        let Some(sources) = self.gpu_frame_sources(commands, document, scale) else {
            gpu_frame::count_fallback("source-failed");
            log_gpu_fallback("reason=source-failed");
            return None;
        };
        let backdrop = match self.gpu_backdrop() {
            Ok(backdrop) => backdrop,
            Err(reason) => {
                gpu_frame::count_fallback(reason);
                log_gpu_fallback(&format!("reason={reason}"));
                return None;
            }
        };
        let (width, height) = (self.width, self.height);
        let ops = match gpu_frame::frame_ops(
            commands,
            scale,
            &sources,
            (width, height),
            backdrop,
        ) {
            Ok(ops) => ops,
            Err(reason) => {
                gpu_frame::count_fallback(reason);
                log_gpu_fallback(&format!("reason={reason}"));
                return None;
            }
        };
        // The scene starts transparent, exactly as `clear_scene` leaves it.
        let mut rgba = vec![0_u8; width as usize * height as usize * 4];
        if crate::accel::try_gpu_batch(&mut rgba, width, height, &ops) {
            flamewm_profiler::CounterPoint::new("render.gpu_frame.lowered").increment();
            Some(rgba)
        } else {
            gpu_frame::count_fallback("gpu-batch");
            None
        }
    }

    /// The retained backdrop the GPU frame starts from: the surface snapshot with its byte copy.
    /// A window backdrop has no byte copy, so it keeps the XRender path with a named reason.
    fn gpu_backdrop(&self) -> Result<Option<(u32, u32, &[u8])>, &'static str> {
        match (&self.surface_backdrop, &self.window_backdrop_pixmap) {
            (Some(backdrop), _) => match backdrop.rgba.as_deref() {
                Some(rgba) => Ok(Some((backdrop.width, backdrop.height, rgba))),
                None => Err("BackdropBytes"),
            },
            (None, Some(_)) => Err("WindowBackdrop"),
            (None, None) => Ok(None),
        }
    }

    /// One source per text, text shadow and image command, in command order.
    fn gpu_frame_sources(
        &mut self,
        commands: &[PaintCommand],
        document: &RuntimeDocument,
        scale: f32,
    ) -> Option<Vec<FrameSource>> {
        let mut sources = Vec::new();
        let mut stack: Vec<Rect> = Vec::new();
        for command in commands {
            match command {
                PaintCommand::PushClip { rect } => stack.push(scale_rect(*rect, scale)),
                PaintCommand::PopClip => {
                    stack.pop();
                }
                PaintCommand::Text { .. }
                | PaintCommand::TextShadow { .. }
                | PaintCommand::Image { .. } => {
                    sources.push(self.gpu_source(command, document, scale, &stack)?);
                }
                _ => {}
            }
        }
        Some(sources)
    }

    /// The coverage or premultiplied pixels for one command, computed exactly as the XRender
    /// painter computes them. `None` when the XRender painter would fail for this command too.
    fn gpu_source(
        &mut self,
        command: &PaintCommand,
        document: &RuntimeDocument,
        scale: f32,
        stack: &[Rect],
    ) -> Option<FrameSource> {
        match command {
            PaintCommand::Text {
                x,
                y,
                color,
                size,
                weight,
                text,
            } => {
                let (x, y, size) = (*x * scale, *y * scale, *size * scale);
                self.ensure_xft();
                let xft = self.xft.as_mut()?;
                // SAFETY: same initialized display and Xft contract as the XRender text draw.
                unsafe { xft.set_preferred_family(document.ui_font_family()) };
                let mut alpha = Vec::new();
                // With no padding the glyph origin sits 4px in and `ceil(size)` down the mask,
                // which places it where `draw_text` puts the baseline at `round(y)`.
                // SAFETY: same contract as above.
                let (width, height) =
                    unsafe { xft.text_alpha_mask(size, *weight, text, 0, &mut alpha) }.ok()?;
                let (x0, y0) = (x.round() as i32 - 4, y.round() as i32 - size.ceil() as i32);
                let (x1, y1) = (x0 + width as i32, y0 + height as i32);
                let color = [color.r, color.g, color.b, color.a];
                // The Xft draw clips glyphs to the rounded stack bounds: keep only the pixels inside.
                let keep = match gpu_frame::glyph_clip(stack) {
                    None => Some((x0, y0, x1, y1)),
                    Some(None) => None,
                    Some(Some((cx0, cy0, cx1, cy1))) => {
                        let (kx0, ky0) = (x0.max(cx0), y0.max(cy0));
                        let (kx1, ky1) = (x1.min(cx1), y1.min(cy1));
                        (kx1 > kx0 && ky1 > ky0).then_some((kx0, ky0, kx1, ky1))
                    }
                };
                let Some((kx0, ky0, kx1, ky1)) = keep else {
                    return Some(FrameSource::Mask {
                        x: x0,
                        y: y0,
                        width: 0,
                        height: 0,
                        alpha: Vec::new(),
                        color,
                    });
                };
                let alpha = if (kx0, ky0, kx1, ky1) == (x0, y0, x1, y1) {
                    alpha
                } else {
                    let row = width as usize;
                    let span = (kx1 - kx0) as usize;
                    let mut cut = Vec::with_capacity(span * (ky1 - ky0) as usize);
                    for yy in ky0..ky1 {
                        let start = (yy - y0) as usize * row + (kx0 - x0) as usize;
                        cut.extend_from_slice(&alpha[start..start + span]);
                    }
                    cut
                };
                Some(FrameSource::Mask {
                    x: kx0,
                    y: ky0,
                    width: (kx1 - kx0) as u32,
                    height: (ky1 - ky0) as u32,
                    alpha,
                    color,
                })
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
                let (x, y, size) = (*x * scale, *y * scale, *size * scale);
                let radius = (*blur_radius * scale).clamp(0.0, 8.0);
                let mut scratch = SHADOW_SCRATCH.take();
                let placed = self.text_shadow_rgba(
                    &mut scratch,
                    x,
                    y,
                    *color,
                    size,
                    *weight,
                    radius,
                    text,
                    document.ui_font_family(),
                );
                let source = match placed {
                    Ok(Some((dx, dy, width, height))) => {
                        // The XRender blit keeps only pixels inside the clip bounds; the rest are
                        // transparent. The blurred alpha already carries the shadow colour's alpha.
                        let (min_x, min_y, max_x, max_y) = gpu_frame::shadow_clip(stack);
                        let row = width as usize;
                        let alpha = scratch
                            .rgba
                            .iter()
                            .skip(3)
                            .step_by(4)
                            .enumerate()
                            .map(|(i, &a)| {
                                let gx = dx + (i % row) as i32;
                                let gy = dy + (i / row) as i32;
                                let inside = gx >= min_x && gy >= min_y && gx < max_x && gy < max_y;
                                if inside { a } else { 0 }
                            })
                            .collect();
                        Some(FrameSource::Mask {
                            x: dx,
                            y: dy,
                            width,
                            height,
                            alpha,
                            color: [color.r, color.g, color.b, 255],
                        })
                    }
                    _ => None,
                };
                SHADOW_SCRATCH.with(|slot| slot.replace(scratch));
                source
            }
            PaintCommand::Image {
                rect,
                node,
                treatment,
                tint,
                ..
            } => {
                let (x, y, width, height) = snap_device_rect(*rect, scale);
                // XRender blits an image only when it is wholly visible; a cut blit is skipped.
                let device = Rect {
                    x: x as f32,
                    y: y as f32,
                    width: width as f32,
                    height: height as f32,
                };
                if visible_under(device, stack) != Visible::Whole {
                    return Some(FrameSource::Image {
                        x,
                        y,
                        width: 0,
                        height: 0,
                        rgba: Vec::new(),
                    });
                }
                let (_, src_w, src_h, _, pixels) = document.image_for_node(*node)?;
                // The XRender blit's own conversion: decoded pixels become premultiplied ARGB32
                // words, so the GPU composites the same colours. Tint is the same default too.
                let words = match treatment {
                    ImageTreatment::Original => super::original_argb32(
                        pixels, src_w, src_h, width, height, *tint,
                    ),
                    ImageTreatment::SymbolicForeground => super::symbolic_foreground_argb32(
                        pixels,
                        src_w,
                        src_h,
                        width,
                        height,
                        (*tint).unwrap_or(Color {
                            r: 255,
                            g: 255,
                            b: 255,
                            a: 255,
                        }),
                    ),
                };
                let rgba: Vec<u8> = words
                    .iter()
                    .flat_map(|&word| {
                        [
                            (word >> 16) as u8,
                            (word >> 8) as u8,
                            word as u8,
                            (word >> 24) as u8,
                        ]
                    })
                    .collect();
                Some(FrameSource::Image {
                    x,
                    y,
                    width,
                    height,
                    rgba,
                })
            }
            _ => None,
        }
    }

    unsafe fn paint_full(&mut self, document: &RuntimeDocument, scale: f32) -> Result<u64, String> {
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
    fn retained_command(&self, index: usize) -> *const PaintCommand {
        let base = self
            .cached_paint
            .as_ref()
            .expect("paint cached above")
            .commands
            .as_ptr();
        // SAFETY: callers pass `index` below the cached command count.
        unsafe { base.add(index) }
    }

    /// Repaints only `region` of the retained backbuffer: clear it, paint the commands that
    /// touch it under a picture clip, reset the clip, then copy that rect to the window.
    /// Returns the number of commands painted.
    unsafe fn paint_region(
        &mut self,
        region: PixelRegion,
        document: &RuntimeDocument,
        scale: f32,
    ) -> Result<u64, String> {
        let _region_guard = flamewm_profiler::start("render.region");
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

    unsafe fn paint_region_commands(
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

    /// Publishes the presented desktop frame on the root ([`crate::backdrop::DESKTOP_FRAME_PROPERTY`])
    /// with the next present serial. The window origin is only queried when the pixmap or size
    /// changed.
    unsafe fn publish_desktop_frame(&mut self) {
        // The root origin is queried on every publish: the desktop can move (XMoveResizeWindow)
        // without its backbuffer or size changing, and readers would then sample the wrong rect.
        let root = unsafe { XRootWindow(self.display, self.screen) };
        let (mut x, mut y, mut child) = (0, 0, 0);
        unsafe {
            crate::xlib_ext::XTranslateCoordinates(
                self.display,
                self.window,
                root,
                0,
                0,
                &mut x,
                &mut y,
                &mut child,
            )
        };
        let serial = self.published_desktop_frame.map_or(0, |last| last.wrapping_add(1));
        let atom = unsafe { self.desktop_frame_atom() };
        let data: [std::os::raw::c_long; 6] = [
            self.backbuffer as std::os::raw::c_long,
            std::os::raw::c_long::from(x as u32),
            std::os::raw::c_long::from(y as u32),
            std::os::raw::c_long::from(self.width),
            std::os::raw::c_long::from(self.height),
            std::os::raw::c_long::from(serial),
        ];
        // SAFETY: six 32-bit property units, matching format 32 (c_long items).
        unsafe {
            XChangeProperty(
                self.display,
                root,
                atom,
                XA_CARDINAL,
                32,
                PROP_MODE_REPLACE,
                data.as_ptr().cast(),
                6,
            );
        }
        self.published_desktop_frame = Some(serial);
    }

    /// Deletes the published desktop frame: its pixmap is about to be freed.
    pub(crate) unsafe fn withdraw_desktop_frame(&mut self) {
        let root = unsafe { XRootWindow(self.display, self.screen) };
        let atom = unsafe { self.desktop_frame_atom() };
        unsafe { XDeleteProperty(self.display, root, atom) };
        self.published_desktop_frame = None;
    }

    unsafe fn desktop_frame_atom(&mut self) -> Atom {
        if self.desktop_frame_atom == 0 {
            let name = std::ffi::CString::new(crate::backdrop::DESKTOP_FRAME_PROPERTY)
                .expect("property name has no NUL");
            self.desktop_frame_atom = unsafe { XInternAtom(self.display, name.as_ptr(), 0) };
        }
        self.desktop_frame_atom
    }

    /// Whether the server already restored an exposed area of the desktop surface: its window
    /// background is the backbuffer holding the last presented frame at the current size, so
    /// repainting would only rewrite identical pixels. Limited to the desktop, the root-sized
    /// surface every moving window and outline exposes; other surfaces keep repainting, since a
    /// resize can leave their window edges different from the retained backbuffer.
    pub(crate) fn exposure_restored_by_background(&self) -> bool {
        self.role == X11WindowRole::Desktop
            && self.backbuffer != 0
            && self.background_bound == self.backbuffer
            && self.presented_size == Some((self.width, self.height))
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
        self.gc_clip = None;
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
            // XCopyArea precedes XMapWindow on first presentation, so first
            // observable frame is the finished document rather than black.
            if self.initial_map_pending {
                unsafe { XMapWindow(self.display, self.window) };
                if self.role == X11WindowRole::PopupMenu {
                    // Override-redirect popups are never focused by the WM; typing needs focus.
                    // SAFETY: live display and the window just mapped above.
                    unsafe {
                        crate::surface_controller::raw_press::XSetInputFocus(
                            self.display,
                            self.window,
                            crate::surface_controller::raw_press::REVERT_TO_POINTER_ROOT,
                            0,
                        );
                    }
                }
                self.initial_map_pending = false;
            }
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

    /// Canonical surface shape shared by full presentation and slide clipping.
    /// A dock corner mask (or border ring) must never disappear while closing.
    fn base_shape_spans(
        &self,
        commands: &[PaintCommand],
        scale: f32,
        backdrop_radius: u32,
    ) -> Vec<(u32, u32, u32)> {
        if let Some(spans) = &self.shape_override {
            return spans.clone();
        }
        if let Some((border, radius)) = self.ring_mask {
            crate::xshape::ring_mask_spans(self.width, self.height, border, radius)
        } else if self.dock_corner_radius > 0 {
            crate::xshape::rounded_mask_spans(self.width, self.height, self.dock_corner_radius)
        } else {
            self.mask_spans(commands, scale, backdrop_radius)
        }
    }

    /// A fixed ring/dock silhouette depends on geometry and radius, never on paint commands.
    /// Coverage-based shapes and caller-supplied alpha silhouettes deliberately bypass the cache.
    /// A clipped slide invalidates `shape_applied`, which forces the next fixed mask to re-apply.
    pub(crate) fn static_shape_key(&self) -> Option<StaticShapeKey> {
        if self.shape_override.is_some() {
            return None;
        }
        static_shape_key(
            self.width,
            self.height,
            self.ring_mask,
            self.dock_corner_radius,
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
        let static_key = self.static_shape_key();
        if fixed_shape_unchanged(
            self.static_shape_applied.get(),
            static_key,
            self.shape_applied.get().is_some(),
        ) {
            return;
        }
        // A retained backdrop is composited under translucent fills, so the snapshot must stay
        // visible across the whole rounded shape; coverage would drop the translucent area and
        // show the live desktop (unblurred) instead. Corners outside the radius stay transparent.
        let spans = self.base_shape_spans(commands, scale, backdrop_radius);
        if spans.is_empty() {
            return;
        }
        // Identical spans leave the server-side bounding region unchanged; skip the XShape request.
        if !shape_needs_apply(&self.shape_applied, self.window, &spans) {
            self.static_shape_applied.set(static_key);
            return;
        }
        if let Err(error) = unsafe { bridge.apply_rounded_mask(self.window, &spans) } {
            self.shape_applied.set(None);
            self.static_shape_applied.set(None);
            eprintln!("FLAMEWM_RENDER_SHAPE_BACKEND degraded reason={error}");
        } else {
            self.static_shape_applied.set(static_key);
        }
    }

    /// Rounded popup mask intersected with `clip` (window-local). Used while a slide keeps the
    /// popup on the screen side of the panel edge; the base mask is restored with
    /// `refresh_shape_mask` once the slide lands.
    pub(crate) unsafe fn refresh_shape_mask_clipped(
        &self,
        document: &RuntimeDocument,
        clip: ClipRect,
    ) {
        let (Some(cached), Some(bridge)) = (self.cached_paint.as_ref(), self.xshape.as_ref())
        else {
            return;
        };
        let backdrop_radius = backdrop_corner_radius_px(document, self.width, self.height);
        let spans = self.base_shape_spans(&cached.commands, 1.0, backdrop_radius);
        // The clipped mask replaces the base mask, so the next base refresh must re-apply it.
        self.shape_applied.set(None);
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
                        let filled =
                            unsafe { crate::xrender_aa::fill_rounded_rect(xrender, rect, radius, *color) };
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

/// Desktop colour a depth-24 scene is cleared to: its pixels are opaque, so 0x00000000 would
/// paint black under node corners and the panel's rounded edge.
pub(crate) const OPAQUE_SCENE_CLEAR_PIXEL: std::os::raw::c_ulong = 0x0023_2629;

/// Clear pixel for a backbuffer of `depth`: transparent on ARGB (depth 32), desktop colour otherwise.
pub(crate) fn scene_clear_pixel(depth: i32) -> std::os::raw::c_ulong {
    if depth >= 32 {
        TRANSPARENT_CLEAR_PIXEL
    } else {
        OPAQUE_SCENE_CLEAR_PIXEL
    }
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
        assert_eq!(
            slide,
            clip_spans(&popup_mask_spans(&[], 40, 20, 1.0, Some(8)), clip)
        );
        let (top_start, top_end) = row_span(&slide, 0);
        assert!(
            top_start > 0 && top_end < 40,
            "slide corners must stay transparent"
        );
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

/// Latch for the base bounding shape: true when `spans` for `window` differ from what was last
/// applied (and records them). Any other writer of the bounding shape must reset the latch.
pub(crate) fn shape_needs_apply(
    applied: &std::cell::Cell<Option<u64>>,
    window: Window,
    spans: &[(u32, u32, u32)],
) -> bool {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    window.hash(&mut hasher);
    spans.hash(&mut hasher);
    let key = hasher.finish();
    if applied.get() == Some(key) {
        return false;
    }
    applied.set(Some(key));
    true
}

#[cfg(test)]
mod shape_latch_tests {
    use super::shape_needs_apply;
    use std::cell::Cell;

    #[test]
    fn identical_spans_apply_once_and_changes_or_resets_reapply() {
        let latch = Cell::new(None);
        let a = [(0u32, 10u32, 20u32), (1, 9, 18)];
        let b = [(0u32, 10u32, 21u32)];
        let applies =
            |spans: &[(u32, u32, u32)], win| usize::from(shape_needs_apply(&latch, win, spans));
        let total = applies(&a, 7) + applies(&a, 7) + applies(&a, 7);
        assert_eq!(total, 1, "100 identical frames must apply the shape once");
        assert_eq!(applies(&b, 7), 1, "changed spans re-apply");
        assert_eq!(applies(&b, 8), 1, "another window re-applies");
        latch.set(None); // clipped writer reset
        assert_eq!(
            applies(&b, 8),
            1,
            "after a clipped mask the base mask re-applies"
        );
    }
}

#[cfg(test)]
mod layout_reuse_tests {
    use super::*;
    use crate::xlib::{XCloseDisplay, XOpenDisplay};
    use flamewm_render_core::{
        Color, CompiledDocument, CompiledNode, NodeKind, RuntimeDocument, Style,
        layout_passes_on_this_thread,
    };

    fn document() -> RuntimeDocument {
        RuntimeDocument::new(CompiledDocument {
            source_fingerprint: 0,
            root: 0,
            variables: Vec::new(),
            assets: Vec::new(),
            nodes: vec![CompiledNode {
                kind: NodeKind::Element,
                parent: None,
                first_child: None,
                next_sibling: None,
                id: "box".to_string().into(),
                action: std::borrow::Cow::Borrowed(""),
                text: "".to_string().into(),
                image: None,
                style: Style::default().into(),
                hover_style: None,
                active_style: None,
            }],
        })
        .expect("fixture document validates")
    }

    /// Color edits repaint without a layout pass; geometry/text edits relayout.
    /// Needs `FLAMEWM_TEST_DISPLAY` (skipped otherwise).
    #[test]
    fn color_edits_reuse_layout_and_geometry_edits_relayout() {
        let Some(name) = std::env::var_os("FLAMEWM_TEST_DISPLAY") else {
            return;
        };
        let name = std::ffi::CString::new(name.to_string_lossy().as_bytes()).unwrap();
        unsafe {
            let display = XOpenDisplay(name.as_ptr());
            assert!(!display.is_null(), "XOpenDisplay failed");
            let config = X11Config {
                width: 200,
                height: 120,
                ..X11Config::default()
            };
            let mut app = X11App::new(display, &config).expect("app");
            {
                let mut doc = document();
                app.redraw(&doc).expect("first redraw");
                let first = layout_passes_on_this_thread();
                for step in 0..25u32 {
                    let color = if step % 2 == 0 {
                        Color::BLACK
                    } else {
                        Color::TRANSPARENT
                    };
                    doc.set_background_color("box", color).expect("color");
                    app.redraw(&doc).expect("repaint");
                }
                assert_eq!(
                    layout_passes_on_this_thread(),
                    first,
                    "color edits must not relayout"
                );
                doc.set_font_size_px("box", 21.0).expect("size");
                app.redraw(&doc).expect("geometry repaint");
                assert_eq!(
                    layout_passes_on_this_thread(),
                    first + 1,
                    "geometry edit relayouts once"
                );
            }
            // Owner order (see runtime.rs): take deferred native handles, drop the app, close
            // the display, and only then release the handles.
            let deferred = app.take_deferred_native_libraries();
            drop(app);
            XCloseDisplay(display);
            drop(deferred);
        }
    }
}

#[cfg(test)]
mod scene_clear_tests {
    use super::*;

    #[test]
    fn depth24_scene_clear_is_never_opaque_black() {
        assert_ne!(scene_clear_pixel(24), 0, "corner pixels must not be 0x000000 on depth 24");
        assert_eq!(scene_clear_pixel(32), TRANSPARENT_CLEAR_PIXEL);
    }
}

#[cfg(test)]
mod partial_fill_clip_tests {
    use super::partial_fill_clip;
    use flamewm_render_core::Rect;

    fn r(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn partial_fill_without_region_keeps_its_visible_part() {
        let part = r(10.0, 85.0, 20.0, 5.0);
        assert_eq!(partial_fill_clip(part, None), Some(part));
    }

    #[test]
    fn partial_fill_in_region_paint_is_cut_to_the_region() {
        // The corner radius is kept by the picture clip; the clip must not leave the region.
        let region = r(0.0, 80.0, 50.0, 20.0);
        assert_eq!(
            partial_fill_clip(r(10.0, 85.0, 60.0, 5.0), Some(region)),
            Some(r(10.0, 85.0, 40.0, 5.0))
        );
    }

    #[test]
    fn partial_fill_outside_region_paint_is_dropped() {
        let region = r(0.0, 80.0, 50.0, 20.0);
        assert_eq!(partial_fill_clip(r(60.0, 85.0, 20.0, 5.0), Some(region)), None);
    }
}
