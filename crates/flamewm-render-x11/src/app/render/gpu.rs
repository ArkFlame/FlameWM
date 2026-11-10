use super::text_shadow::{SHADOW_SCRATCH, shadow_origin};
use super::*;

/// Backdrop size and optional RGBA bytes handed to the GPU frame.
pub(super) type GpuBackdrop<'a> = (u32, u32, &'a [u8]);

/// Logs each distinct XRender fallback reason once, so the log holds the census of blockers.
pub(super) fn log_gpu_fallback(reason: &str) {
    static SEEN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let Ok(mut seen) = SEEN.lock() else { return };
    if !seen.iter().any(|known| known == reason) {
        eprintln!("FLAMEWM_RENDER_GPU_FRAME xrender fallback {reason}");
        seen.push(reason.to_string());
    }
}

impl X11App {
    /// Full clear, surface backdrop, every command, node backdrops, then present.
    /// Whole-frame GPU paint for `renderAcceleration=gpu`: one upload of the transparent scene,
    /// one readback, then one ARGB upload to the backbuffer. `None` keeps the XRender path, which
    /// also takes every GPU failure, so a frame never mixes the two painters.
    pub(super) unsafe fn paint_full_gpu(
        &mut self,
        document: &RuntimeDocument,
        scale: f32,
    ) -> Option<u64> {
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
                    (width, height),
                    "GPU frame upload",
                )
            }
            .or_else(|_| unsafe {
                self.upload_opaque_fallback(self.backbuffer, &rgba, width, height, width, height)
            })
        };
        uploaded.ok().map(|()| {
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
    pub(super) fn gpu_frame_pixels(
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
        let ops = match gpu_frame::frame_ops(commands, scale, &sources, (width, height), backdrop) {
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
    pub(super) fn gpu_backdrop(&self) -> Result<Option<GpuBackdrop<'_>>, &'static str> {
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
    pub(super) fn gpu_frame_sources(
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
    pub(super) fn gpu_source(
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
                let placed = self.text_shadow_image(
                    &mut scratch,
                    size,
                    *weight,
                    radius,
                    *color,
                    text,
                    document.ui_font_family(),
                );
                let source = match placed {
                    Ok(Some(image)) => {
                        let (dx, dy) = shadow_origin(x, y, size, radius);
                        let (width, height) = (image.width, image.height);
                        // The XRender blit keeps only pixels inside the clip bounds; the rest are
                        // transparent. The blurred alpha already carries the shadow colour's alpha,
                        // which is the top byte of each premultiplied word.
                        let (min_x, min_y, max_x, max_y) = gpu_frame::shadow_clip(stack);
                        let row = width as usize;
                        let alpha = image
                            .argb
                            .iter()
                            .enumerate()
                            .map(|(i, &word)| {
                                let a = (word >> 24) as u8;
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
                    ImageTreatment::Original => {
                        super::original_argb32(pixels, src_w, src_h, width, height, *tint)
                    }
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
}
