use super::*;

/// Per-thread shadow work buffers. The display thread is the only shadow painter,
/// so every text shadow reuses the blur scratch and staging buffers instead of
/// reallocating them per paint.
#[derive(Default)]
pub(super) struct ShadowScratch {
    pub(super) blur: BlurScratch,
    pub(super) rgba: Vec<u8>,
    pub(super) argb: Vec<u32>,
    pub(super) mask: Vec<u8>,
}

thread_local! {
    pub(super) static SHADOW_SCRATCH: std::cell::RefCell<ShadowScratch> = std::cell::RefCell::default();
}

impl X11App {
    /// Blurred shadow for one text run: the canonical Xft glyph coverage, then the shared bounded
    /// Gaussian blur, left as straight RGBA in `scratch.rgba`. Returns the device placement, or
    /// `None` when Xft cannot draw the glyphs.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn text_shadow_rgba(
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

    /// Batches every uncached text-shadow mask of this frame into one readback before the command
    /// loop, so a frame costs one `XGetImage` instead of one per changed text run. Runs that are
    /// not prefetched still render through [`Self::paint_text_shadow`].
    pub(crate) unsafe fn prefetch_text_shadow_masks(
        &mut self,
        count: usize,
        document: &RuntimeDocument,
        scale: f32,
    ) {
        let mut runs: Vec<(f32, u16, u32, std::sync::Arc<str>)> = Vec::new();
        for index in 0..count {
            // SAFETY: index is below the retained count; no cache replacement happens here.
            let command = unsafe { &*self.retained_command(index) };
            if let PaintCommand::TextShadow {
                size,
                weight,
                blur_radius,
                text,
                ..
            } = command
            {
                let radius = (*blur_radius * scale).clamp(0.0, 8.0);
                let pad = (radius * 3.0).ceil() as u32 + 4;
                runs.push((*size * scale, *weight, pad, std::sync::Arc::clone(text)));
            }
        }
        if runs.is_empty() {
            return;
        }
        self.ensure_xft();
        if self.xrender.is_none() {
            return;
        }
        let family = document.ui_font_family();
        let Some(xft) = self.xft.as_mut() else {
            return;
        };
        unsafe { xft.set_preferred_family(family) };
        let specs: Vec<crate::xft::AlphaMaskSpec<'_>> = runs
            .iter()
            .map(|(size, weight, pad, text)| crate::xft::AlphaMaskSpec {
                size: *size,
                weight: *weight,
                text,
                pad: *pad,
            })
            .collect();
        unsafe { xft.prefetch_text_masks(&specs) };
    }

    /// The text-shadow upload target: one ARGB pixmap reused by every shadow (grown on demand,
    /// never shrunk). Each shadow blits only its own `w x h` origin region, so reuse is
    /// pixel-identical to a fresh pixmap per shadow, without a create, sync and free per run.
    pub(super) fn shadow_upload_pixmap(
        &mut self,
        root: Drawable,
        width: u32,
        height: u32,
    ) -> Result<Pixmap, String> {
        if let Some((pixmap, cap_w, cap_h, _)) = self.shadow_pixmap
            && cap_w >= width
            && cap_h >= height
        {
            return Ok(pixmap);
        }
        let (prev_w, prev_h) = self.shadow_pixmap.map_or((0, 0), |(_, cw, ch, _)| (cw, ch));
        let (cap_w, cap_h) = (width.max(prev_w), height.max(prev_h));
        let (pixmap, bytes) = self.alloc_pixmap(root, cap_w, cap_h, 32)?;
        if let Some((old, _, _, old_bytes)) =
            self.shadow_pixmap.replace((pixmap, cap_w, cap_h, bytes))
        {
            self.free_pixmap_tracked(old, old_bytes);
        }
        Ok(pixmap)
    }

    /// Soft alpha shadow uses the canonical Xft glyph coverage once, the shared
    /// bounded Gaussian blur, then a transient premultiplied ARGB32 XRender blit.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn paint_text_shadow(
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
        let pixmap = self.shadow_upload_pixmap(root, w, h)?;
        let result = (|| {
            unsafe { self.upload_argb32_words(pixmap, visual, argb, w, h)? };
            if let Some(render) = self.xrender.as_mut() {
                unsafe { render.blit_argb32_over(pixmap, dx, dy, w, h)? };
            }
            Ok::<_, String>(())
        })();
        SHADOW_SCRATCH.with(|slot| slot.replace(scratch));
        result.map(|()| true)
    }
}
