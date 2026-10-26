use super::*;

/// Device-px corner radius of a popup, the same value the retained shape mask uses
/// (`app::render::backdrop_corner_radius_px`, duplicated here because that module is private).
pub(super) fn popup_backdrop_radius_px(document: &RuntimeDocument, width: u32, height: u32) -> u32 {
    let radius = flamewm_render_core::paint::root_corner_radius(
        document,
        flamewm_render_core::InteractionState::default(),
    );
    flamewm_render_core::paint::surface_shape_pixels(width, height, radius * document.ui_scale()).2
}

/// Anti-aliased coverage (0..=1) of pixel `(px, py)` inside a `width` x `height` rounded rect.
/// Pixel centres are sampled, so the arc has a one-pixel ramp rather than a hard edge.
pub(super) fn rounded_coverage(px: u32, py: u32, width: u32, height: u32, radius: u32) -> f32 {
    if radius == 0 {
        return 1.0;
    }
    let (w, h) = (width as f32, height as f32);
    let r = (radius as f32).min(w / 2.0).min(h / 2.0);
    let lx = px as f32 + 0.5;
    let ly = py as f32 + 0.5;
    let cx = lx.clamp(r, w - r);
    let cy = ly.clamp(r, h - r);
    let dist = ((lx - cx).powi(2) + (ly - cy).powi(2)).sqrt();
    (r + 0.5 - dist).clamp(0.0, 1.0)
}

/// Masks an opaque ARGB snapshot (0xAARRGGBB, row-major) to the rounded shape, writing
/// premultiplied words so corners outside the arc are transparent in every channel.
pub(super) fn round_backdrop_words(words: &mut [u32], width: u32, height: u32, radius: u32) {
    if width == 0 {
        return;
    }
    for (index, word) in words.iter_mut().enumerate() {
        let index = index as u32;
        let coverage = rounded_coverage(index % width, index / width, width, height, radius);
        if coverage >= 1.0 {
            continue;
        }
        let alpha = (coverage * 255.0).round() as u32;
        let premul = |shift: u32| ((*word >> shift) & 0xff) * alpha / 255;
        *word = (alpha << 24) | (premul(16) << 16) | (premul(8) << 8) | premul(0);
    }
}

impl SurfaceController {
    /// Sets (or clears with `None`) the `(opacity, blur)` percents of the backdrop captured for
    /// the popup's next first show. Opaque popups (`opacity >= 100`) never capture.
    pub fn set_popup_backdrop(
        &mut self,
        id: SurfaceId,
        percents: Option<(u8, u8)>,
    ) -> Result<(), String> {
        self.instance_mut(id)?.popup_backdrop = percents;
        Ok(())
    }

    /// Sets (or clears with `None`) the region-sized wallpaper source for the popup's next show.
    /// The crop is consumed on first show, avoiding retention of root-sized pixels.
    pub fn set_popup_wallpaper_region(
        &mut self,
        id: SurfaceId,
        region: Option<dock::PopupWallpaper>,
    ) -> Result<(), String> {
        self.instance_mut(id)?.popup_wallpaper = None;
        if let Some((rect, pixels)) = region.as_ref() {
            if pixels.len() != Self::popup_wallpaper_byte_len(*rect)? {
                return Err("popup wallpaper region has an invalid pixel length".to_owned());
            }
        }
        self.instance_mut(id)?.popup_wallpaper = region;
        Ok(())
    }

    fn popup_wallpaper_byte_len(rect: (i32, i32, u32, u32)) -> Result<usize, String> {
        (rect.2 as usize)
            .checked_mul(rect.3 as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| "popup wallpaper region size overflow".to_owned())
    }

    /// Validates a one-shot wallpaper region against the current popup rect and byte count.
    pub(super) fn popup_wallpaper_capture(
        rect: flamewm_render_core::Rect,
        region: Option<dock::PopupWallpaper>,
    ) -> Result<Option<crate::backdrop::BackdropCapture>, String> {
        let Some((expected, pixels)) = region else {
            return Ok(None);
        };
        let actual = (
            rect.x as i32,
            rect.y as i32,
            rect.width as u32,
            rect.height as u32,
        );
        if actual != expected {
            return Err("popup wallpaper region does not match the final popup rect".to_owned());
        }
        let expected_len = Self::popup_wallpaper_byte_len(actual)?;
        if pixels.len() != expected_len {
            return Err("popup wallpaper region has an invalid pixel length".to_owned());
        }
        Ok(Some(crate::backdrop::BackdropCapture {
            width: actual.2,
            height: actual.3,
            pixels,
        }))
    }

    pub(super) fn take_popup_wallpaper(
        region: &mut Option<dock::PopupWallpaper>,
    ) -> Option<dock::PopupWallpaper> {
        region.take()
    }

    pub(super) fn clear_popup_wallpaper(region: &mut Option<dock::PopupWallpaper>) {
        *region = None;
    }

    /// Popup-sized backdrop source: the one-shot wallpaper region when set, else the root capture.
    fn popup_backdrop_source(
        &self,
        id: SurfaceId,
        region: Option<dock::PopupWallpaper>,
    ) -> Result<crate::backdrop::BackdropCapture, String> {
        let rect = self.surface_device_rect(id)?;
        if let Some(capture) = Self::popup_wallpaper_capture(rect, region)? {
            Ok(capture)
        } else {
            self.capture_root_under(id)
        }
    }

    /// Captures the root under an unmapped popup, blurs it, and installs it beneath the translucent
    /// fills. A blank or uniform capture (root not painted yet) installs nothing: solid colour.
    pub(super) fn install_popup_backdrop(&mut self, id: SurfaceId) -> Result<(), String> {
        let (backdrop, region) = {
            let instance = self.instance_mut(id)?;
            (
                instance.popup_backdrop,
                Self::take_popup_wallpaper(&mut instance.popup_wallpaper),
            )
        };
        let Some((opacity, blur)) = backdrop else {
            return Ok(());
        };
        let snapshot = if opacity < 100 {
            self.popup_backdrop_source(id, region)
                .ok()
                .filter(|capture| crate::backdrop::capture_is_ready(&capture.pixels))
        } else {
            None
        };
        let instance = self.instance_mut(id)?;
        let Some(capture) = snapshot else {
            instance.app.clear_surface_backdrop();
            return Ok(());
        };
        let (width, height) = (capture.width, capture.height);
        let mut words =
            crate::backdrop::blurred_argb_words(width, height, capture.pixels, opacity, blur);
        let radius = popup_backdrop_radius_px(&instance.document, width, height);
        round_backdrop_words(&mut words, width, height, radius);
        // SAFETY: the popup's display and backbuffer are live and the window is still unmapped.
        let installed = unsafe { instance.app.set_surface_backdrop(&words, width, height) };
        if installed.is_err() {
            instance.app.clear_surface_backdrop();
        }
        installed?;
        self.redraw(id)
    }

    /// Root pixels under the surface's current device rect, as straight RGBA8. Feeds the
    /// pseudo-transparency snapshot; the caller owns blur and refresh policy.
    pub fn capture_root_under(
        &self,
        id: SurfaceId,
    ) -> Result<crate::backdrop::BackdropCapture, String> {
        let rect = self.surface_device_rect(id)?;
        let root = unsafe { XRootWindow(self.display, self.screen) };
        crate::backdrop::capture_drawable_rect(
            self.display,
            root as u64,
            (
                rect.x as i32,
                rect.y as i32,
                rect.width as u32,
                rect.height as u32,
            ),
        )
    }

    /// Root pixels under a mapped surface, read with the surface unmapped so its own translucent
    /// fills never enter the snapshot. A surface that was mapped is mapped again before returning.
    pub fn capture_root_under_unmapped(
        &mut self,
        id: SurfaceId,
    ) -> Result<crate::backdrop::BackdropCapture, String> {
        let display = self.display;
        let instance = self.instance_mut(id)?;
        let (window, was_shown) = (instance.app.window, instance.shown);
        if was_shown {
            // SAFETY: display and window are live; the pending unmap is flushed before the read.
            unsafe {
                XUnmapWindow(display, window);
                XSync(display, 0);
            }
        }
        let captured = self.capture_root_under(id);
        if was_shown {
            // SAFETY: display and window are live; the window repaints on the Expose that follows.
            unsafe {
                XMapWindow(display, window);
                XFlush(display);
            }
        }
        captured
    }

    /// Installs a static ARGB snapshot (row-major 0xAARRGGBB) under the surface's translucent
    /// fills, then repaints. `None` removes it.
    pub fn set_surface_backdrop(
        &mut self,
        id: SurfaceId,
        backdrop: Option<(&[u32], u32, u32)>,
    ) -> Result<(), String> {
        {
            let instance = self.instance_mut(id)?;
            match backdrop {
                Some((argb, width, height)) => unsafe {
                    instance.app.set_surface_backdrop(argb, width, height)?
                },
                None => instance.app.clear_surface_backdrop(),
            }
        }
        self.redraw(id)
    }
}
