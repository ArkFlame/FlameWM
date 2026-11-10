use super::*;
use crate::text_shadow_cache::{ShadowImage, ShadowKey};

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

/// Whether `text` has any glyph that could draw ink. Whitespace draws none, so its shadow is
/// transparent: the blur, upload and blit for it are skipped, and the output is unchanged.
fn has_ink(text: &str) -> bool {
    !text.chars().all(char::is_whitespace)
}

/// Margin around the glyph coverage that holds the blur's reach. Depends only on the radius.
fn shadow_pad(radius: f32) -> u32 {
    (radius * 3.0).ceil() as u32 + 4
}

/// Device origin of a shadow image for a run at `(x, baseline)`. Depends on the pad, so on radius.
pub(super) fn shadow_origin(x: f32, baseline: f32, size: f32, radius: f32) -> (i32, i32) {
    let pad = shadow_pad(radius) as i32;
    (
        x.round() as i32 - pad - 4,
        baseline.round() as i32 - pad - size.ceil() as i32,
    )
}

/// Device-pixel clip as `(min_x, min_y, max_x, max_y)`, half-open. `i32::MIN`/`MAX` when unclipped.
type ClipBounds = (i32, i32, i32, i32);

/// Whether the `w x h` image at `(dx, dy)` lies entirely inside `clip`.
fn image_inside_clip(clip: ClipBounds, dx: i32, dy: i32, w: u32, h: u32) -> bool {
    let (min_x, min_y, max_x, max_y) = clip;
    i64::from(dx) >= i64::from(min_x)
        && i64::from(dy) >= i64::from(min_y)
        && i64::from(dx) + i64::from(w) <= i64::from(max_x)
        && i64::from(dy) + i64::from(h) <= i64::from(max_y)
}

/// Copies `src` into `out`, keeping only the pixels whose device position lies inside `clip`.
/// Pixels outside are zero, the same transparent value the premultiply path writes for them.
fn clip_words_into(
    out: &mut Vec<u32>,
    src: &[u32],
    w: u32,
    h: u32,
    dx: i32,
    dy: i32,
    clip: ClipBounds,
) {
    let (min_x, min_y, max_x, max_y) = clip;
    out.clear();
    out.reserve(w as usize * h as usize);
    for yy in 0..h {
        let gy = dy + yy as i32;
        let row_open = gy >= min_y && gy < max_y;
        for xx in 0..w {
            let gx = dx + xx as i32;
            let i = yy as usize * w as usize + xx as usize;
            out.push(if row_open && gx >= min_x && gx < max_x {
                src[i]
            } else {
                0
            });
        }
    }
}

/// Blurred, premultiplied ARGB32 image of one shadow from its glyph coverage `mask` (`w x h`).
/// This is the whole per-run pipeline: coverage to coloured RGBA, the shared bounded blur, then
/// premultiply. Pure in its inputs, so a cached image equals a fresh render of the same key.
fn shadow_image_from_coverage(
    blur: &mut BlurScratch,
    rgba: &mut Vec<u8>,
    mask: &[u8],
    w: u32,
    h: u32,
    color: Color,
    radius: f32,
) -> Result<ShadowImage, String> {
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
    let mut argb = Vec::new();
    crate::native::image::premultiply_masked_into(&mut argb, &rgba[..], w, h, |_, _| true);
    Ok(ShadowImage::new(w, h, argb))
}

thread_local! {
    pub(super) static SHADOW_SCRATCH: std::cell::RefCell<ShadowScratch> = std::cell::RefCell::default();
}

impl X11App {
    /// The finished shadow image for one text run: cached by [`ShadowKey`], or rendered once from
    /// the canonical Xft glyph coverage and stored. Returns `None` when Xft cannot draw the glyphs.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn text_shadow_image(
        &mut self,
        scratch: &mut ShadowScratch,
        size: f32,
        weight: u16,
        radius: f32,
        color: Color,
        text: &str,
        family: &str,
    ) -> Result<Option<std::rc::Rc<ShadowImage>>, String> {
        let key = ShadowKey::new(
            family,
            text,
            size,
            weight,
            radius,
            [color.r, color.g, color.b, color.a],
        );
        if let Some(image) = self.shadow_images.get(&key) {
            return Ok(Some(image));
        }
        self.ensure_xft();
        if self.xrender.is_none() || self.xft.is_none() {
            return Ok(None);
        }
        let pad = shadow_pad(radius);
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
        let image = shadow_image_from_coverage(blur, rgba, mask, w, h, color, radius)?;
        let mut displaced = Vec::new();
        let image = self.shadow_images.insert(key, image, &mut displaced);
        self.free_shadow_images(displaced);
        Ok(Some(image))
    }

    /// Batches every uncached text-shadow mask of this frame into one readback before the command
    /// loop, so a frame costs one `XGetImage` instead of one per changed text run. Runs whose
    /// finished image is already cached need no mask. Runs that are not prefetched still render
    /// through [`Self::paint_text_shadow`].
    pub(crate) unsafe fn prefetch_text_shadow_masks(
        &mut self,
        count: usize,
        document: &RuntimeDocument,
        scale: f32,
    ) {
        let family = document.ui_font_family();
        let mut runs: Vec<(f32, u16, u32, std::sync::Arc<str>)> = Vec::new();
        for index in 0..count {
            // SAFETY: index is below the retained count; no cache replacement happens here.
            let command = unsafe { &*self.retained_command(index) };
            if let PaintCommand::TextShadow {
                size,
                weight,
                blur_radius,
                color,
                text,
                ..
            } = command
                && has_ink(text)
            {
                let size = *size * scale;
                let radius = (*blur_radius * scale).clamp(0.0, 8.0);
                let key = ShadowKey::new(
                    family,
                    text,
                    size,
                    *weight,
                    radius,
                    [color.r, color.g, color.b, color.a],
                );
                if self.shadow_images.contains(&key) {
                    continue;
                }
                runs.push((
                    size,
                    *weight,
                    shadow_pad(radius),
                    std::sync::Arc::clone(text),
                ));
            }
        }
        if runs.is_empty() {
            return;
        }
        self.ensure_xft();
        if self.xrender.is_none() {
            return;
        }
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

    /// Device-pixel bounds of every retained clip, intersected.
    fn shadow_clip_bounds(&self) -> ClipBounds {
        let (mut min_x, mut min_y) = (i32::MIN, i32::MIN);
        let (mut max_x, mut max_y) = (i32::MAX, i32::MAX);
        for clip in &self.clip_stack {
            min_x = min_x.max(clip.x.floor() as i32);
            min_y = min_y.max(clip.y.floor() as i32);
            max_x = max_x.min((clip.x + clip.width).ceil() as i32);
            max_y = max_y.min((clip.y + clip.height).ceil() as i32);
        }
        (min_x, min_y, max_x, max_y)
    }

    /// Soft alpha shadow uses the canonical Xft glyph coverage once, the shared
    /// bounded Gaussian blur, then a transient premultiplied ARGB32 XRender blit.
    /// A repeated run reuses its cached premultiplied image and skips the render.
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
        if !has_ink(text) {
            // Nothing to blur: the shadow of blank text is fully transparent.
            return Ok(true);
        }
        let mut scratch = SHADOW_SCRATCH.take();
        let image =
            match self.text_shadow_image(&mut scratch, size, weight, radius, color, text, family) {
                Ok(Some(image)) => image,
                other => {
                    SHADOW_SCRATCH.with(|slot| slot.replace(scratch));
                    return other.map(|_| false);
                }
            };
        let (dx, dy) = shadow_origin(x, baseline, size, radius);
        let clip = self.shadow_clip_bounds();
        let Some(visual) = self.argb_visual else {
            SHADOW_SCRATCH.with(|slot| slot.replace(scratch));
            return Ok(false);
        };
        let result = self.draw_shadow(&image, &mut scratch, visual, clip, (dx, dy));
        SHADOW_SCRATCH.with(|slot| slot.replace(scratch));
        result.map(|()| true)
    }

    /// Frees the server copies of images that left the cache.
    pub(crate) fn free_shadow_images(&mut self, images: Vec<std::rc::Rc<ShadowImage>>) {
        for image in images {
            if let Some((pixmap, bytes)) = image.server.take() {
                self.free_pixmap_tracked(pixmap, bytes);
            }
        }
    }

    /// The server-side copy of `image`, uploaded once on first use. `None` when the pixmap
    /// allocator refuses the size, so the caller uses the per-draw upload instead.
    fn shadow_server_copy(
        &mut self,
        image: &ShadowImage,
        visual: *mut Visual,
    ) -> Result<Option<Pixmap>, String> {
        if let Some((pixmap, _)) = image.server.get() {
            return Ok(Some(pixmap));
        }
        let root = unsafe { XRootWindow(self.display, self.screen) };
        let Ok((pixmap, bytes)) = self.alloc_pixmap(root, image.width, image.height, 32) else {
            return Ok(None);
        };
        if let Err(error) = unsafe {
            self.upload_argb32_words(pixmap, visual, &image.argb, image.width, image.height)
        } {
            self.free_pixmap_tracked(pixmap, bytes);
            return Err(error);
        }
        image.server.set(Some((pixmap, bytes)));
        Ok(Some(pixmap))
    }

    /// Composites `image` at device origin `(dx, dy)`. A run wholly inside the clip blits its
    /// server copy, so a repeated run costs one composite and no upload. A clipped run, or one
    /// without a server copy, is uploaded through the reusable target with pixels outside the
    /// clip zeroed.
    fn draw_shadow(
        &mut self,
        image: &ShadowImage,
        scratch: &mut ShadowScratch,
        visual: *mut Visual,
        clip: ClipBounds,
        (dx, dy): (i32, i32),
    ) -> Result<(), String> {
        let (w, h) = (image.width, image.height);
        let inside = image_inside_clip(clip, dx, dy, w, h);
        if inside && let Some(pixmap) = self.shadow_server_copy(image, visual)? {
            if let Some(render) = self.xrender.as_mut() {
                unsafe { render.blit_argb32_over(pixmap, dx, dy, w, h)? };
            }
            return Ok(());
        }
        let words: &[u32] = if inside {
            &image.argb[..]
        } else {
            clip_words_into(&mut scratch.argb, &image.argb, w, h, dx, dy, clip);
            &scratch.argb[..]
        };
        let root = unsafe { XRootWindow(self.display, self.screen) };
        let pixmap = self.shadow_upload_pixmap(root, w, h)?;
        unsafe { self.upload_argb32_words(pixmap, visual, words, w, h)? };
        if let Some(render) = self.xrender.as_mut() {
            unsafe { render.blit_argb32_over(pixmap, dx, dy, w, h)? };
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coverage(w: u32, h: u32) -> Vec<u8> {
        (0..w * h).map(|i| ((i * 37 + 11) % 256) as u8).collect()
    }

    fn fresh(mask: &[u8], w: u32, h: u32, color: Color, radius: f32) -> ShadowImage {
        let mut blur = BlurScratch::default();
        let mut rgba = Vec::new();
        shadow_image_from_coverage(&mut blur, &mut rgba, mask, w, h, color, radius)
            .expect("fresh shadow renders")
    }

    #[test]
    fn whitespace_only_text_has_no_ink_so_its_shadow_work_is_skipped() {
        assert!(!has_ink(""));
        assert!(!has_ink("        "));
        assert!(!has_ink("\t \u{a0}"));
        assert!(has_ink(" a "));
        assert!(has_ink("$ "));
    }

    #[test]
    fn fresh_render_is_deterministic_so_a_cached_hit_matches_it_byte_for_byte() {
        let color = Color {
            r: 9,
            g: 17,
            b: 33,
            a: 0x80,
        };
        let mask = coverage(13, 9);
        let first = fresh(&mask, 13, 9, color, 2.5);
        let second = fresh(&mask, 13, 9, color, 2.5);
        assert_eq!(first, second);

        let mut cache = crate::text_shadow_cache::TextShadowCache::with_capacity(1 << 20);
        let key = ShadowKey::new("Sans", "On", 13.0, 400, 2.5, [9, 17, 33, 0x80]);
        cache.insert(
            key.clone(),
            fresh(&mask, 13, 9, color, 2.5),
            &mut Vec::new(),
        );
        let hit = cache.get(&key).expect("stored shadow hits");
        assert_eq!(hit.argb, first.argb);
        assert_eq!((hit.width, hit.height), (13, 9));
    }

    #[test]
    fn colour_and_radius_change_the_rendered_pixels() {
        let mask = coverage(11, 7);
        let base = fresh(
            &mask,
            11,
            7,
            Color {
                r: 0,
                g: 0,
                b: 0,
                a: 0x80,
            },
            2.0,
        );
        let recoloured = fresh(
            &mask,
            11,
            7,
            Color {
                r: 255,
                g: 0,
                b: 0,
                a: 0x80,
            },
            2.0,
        );
        let reblurred = fresh(
            &mask,
            11,
            7,
            Color {
                r: 0,
                g: 0,
                b: 0,
                a: 0x80,
            },
            4.0,
        );
        assert_ne!(base.argb, recoloured.argb);
        assert_ne!(base.argb, reblurred.argb);
    }

    #[test]
    fn clipped_words_equal_masked_premultiply_for_the_same_clip() {
        let (w, h) = (6u32, 5u32);
        let rgba: Vec<u8> = (0..w * h * 4).map(|i| (i * 13 + 7) as u8).collect();
        let mut full = Vec::new();
        crate::native::image::premultiply_masked_into(&mut full, &rgba, w, h, |_, _| true);
        // Shadow origin at (-2, 1), clip covering device x in [0, 2), y in [2, 5).
        let (dx, dy) = (-2i32, 1i32);
        let clip: ClipBounds = (0, 2, 2, 5);
        let mut expected = Vec::new();
        crate::native::image::premultiply_masked_into(&mut expected, &rgba, w, h, |x, y| {
            let gx = dx + x as i32;
            let gy = dy + y as i32;
            gx >= clip.0 && gy >= clip.1 && gx < clip.2 && gy < clip.3
        });
        let mut got = Vec::new();
        clip_words_into(&mut got, &full, w, h, dx, dy, clip);
        assert_eq!(got, expected);
        assert!(!image_inside_clip(clip, dx, dy, w, h));
        assert!(image_inside_clip(
            (i32::MIN, i32::MIN, i32::MAX, i32::MAX),
            dx,
            dy,
            w,
            h
        ));
        assert!(image_inside_clip((-2, 0, 4, 6), dx, dy, w, h));
    }
}
