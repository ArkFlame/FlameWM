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
    /// Sets (or clears with `None`) the `(opacity, blur)` percents of the backdrop installed for
    /// the popup's next first show. Opaque popups (`opacity >= 100`) install none.
    pub fn set_popup_backdrop(
        &mut self,
        id: SurfaceId,
        percents: Option<(u8, u8)>,
    ) -> Result<(), String> {
        self.instance_mut(id)?.popup_backdrop = percents;
        Ok(())
    }

    /// Sets what the desktop layer is made of (`None`: translucent surfaces stay solid). An
    /// unchanged source keeps the canvas already built.
    pub fn set_desktop_source(&mut self, source: Option<crate::backdrop::DesktopSource>) {
        if self
            .desktop
            .as_ref()
            .map(crate::backdrop::DesktopLayer::source)
            != source.as_ref()
        {
            self.desktop = source.map(crate::backdrop::DesktopLayer::new);
        }
    }

    /// Opaque ARGB32 words of the desktop layer under the root-space `rect` for a surface at
    /// `opacity` and `blur` percent. `None` without a desktop source.
    pub fn desktop_backdrop(
        &mut self,
        rect: (i32, i32, u32, u32),
        opacity: u8,
        blur: u8,
    ) -> Option<Vec<u32>> {
        // SAFETY: the controller owns a live display; these only read its cached screen size.
        let root = unsafe {
            (
                XDisplayWidth(self.display, self.screen).max(0) as u32,
                XDisplayHeight(self.display, self.screen).max(0) as u32,
            )
        };
        Some(self.desktop.as_mut()?.crop(root, rect, opacity, blur))
    }

    /// Installs the desktop layer under an unmapped popup's translucent fills, rounded to its
    /// shape. The root is never read: what is behind a popup is the desktop layer, like every
    /// other translucent surface. Without a desktop source the popup stays solid colour.
    pub(super) fn install_popup_backdrop(&mut self, id: SurfaceId) -> Result<(), String> {
        let Some((opacity, blur)) = self.instance_mut(id)?.popup_backdrop else {
            return Ok(());
        };
        let rect = self.surface_device_rect(id)?;
        let (width, height) = (rect.width as u32, rect.height as u32);
        let words = if opacity < 100 && width > 0 && height > 0 {
            self.desktop_backdrop((rect.x as i32, rect.y as i32, width, height), opacity, blur)
        } else {
            None
        };
        let instance = self.instance_mut(id)?;
        let Some(mut words) = words else {
            instance.app.clear_surface_backdrop();
            return Ok(());
        };
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
