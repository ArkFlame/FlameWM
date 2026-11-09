use super::*;
use crate::backdrop::LiveSurfaceFrame;

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
    if width == 0 || radius == 0 {
        return;
    }
    // The clamped arc radius is at most `radius`, so a pixel whose centre is at least `margin`
    // pixels inside every edge has coverage 1. Skipping those keeps the result exact and avoids the
    // square root for the interior.
    let margin = radius.min(width).min(height).saturating_add(2);
    for (index, word) in words.iter_mut().enumerate() {
        let index = index as u32;
        let (x, y) = (index % width, index / width);
        if x >= margin && x + margin < width && y >= margin && y + margin < height {
            continue;
        }
        let coverage = rounded_coverage(x, y, width, height, radius);
        if coverage >= 1.0 {
            continue;
        }
        let alpha = (coverage * 255.0).round() as u32;
        let premul = |shift: u32| ((*word >> shift) & 0xff) * alpha / 255;
        *word = (alpha << 24) | (premul(16) << 16) | (premul(8) << 8) | premul(0);
    }
}

/// Whether `rect` lies fully inside the root. A read outside the root would make `XGetImage`
/// fail; the caller then keeps the desktop-layer fallback.
pub(super) fn rect_inside_root(rect: (i32, i32, u32, u32), root: (u32, u32)) -> bool {
    let (x, y, width, height) = rect;
    width > 0
        && height > 0
        && x >= 0
        && y >= 0
        && i64::from(x) + i64::from(width) <= i64::from(root.0)
        && i64::from(y) + i64::from(height) <= i64::from(root.1)
}

/// Choose the popup backdrop source: the captured screen under the popup wins; only a failed or
/// empty capture falls back to the desktop layer. Pure so the selection is testable without X.
pub(super) fn popup_backdrop_words(
    capture: Option<crate::backdrop::BackdropCapture>,
    fallback: impl FnOnce() -> Option<Vec<u32>>,
    opacity: u8,
    blur: u8,
) -> Option<Vec<u32>> {
    match capture {
        Some(capture) => Some(crate::backdrop::blurred_argb_words(
            capture.width,
            capture.height,
            capture.pixels,
            opacity,
            blur,
        )),
        None => fallback(),
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
        let desktop = self.desktop.as_mut()?;
        // SAFETY: the controller owns a live display.
        let outputs = unsafe { crate::backdrop::published_wallpaper_outputs(self.display) };
        Some(desktop.crop(root, &outputs, rect, opacity, blur))
    }

    /// Live pseudo-transparency for surface `id` under root `rect`: the desktop and every redirected
    /// frame stacked below its window, composed on the server. `Ok(None)` when the WM publishes no
    /// redirected frames, so the caller keeps the desktop-only backdrop.
    pub fn live_backdrop(
        &mut self,
        id: SurfaceId,
        rect: (i32, i32, u32, u32),
        blur: u8,
    ) -> Result<Option<LiveSurfaceFrame>, String> {
        let window = self.instance_mut(id)?.app.window;
        let (display, screen) = (self.display, self.screen);
        if self.live_scene.is_none() {
            // SAFETY: the controller owns a live display.
            let scene = unsafe { crate::app::live_backdrop::LiveScene::new(display, screen) };
            if scene.is_some() {
                // LiveScene selects SubstructureNotify on the root, which replaces this client's root
                // mask. Restore the root watch so root child changes still reach `take_root_changed`.
                self.watch_root_changes();
            }
            self.live_scene = Some(scene);
        }
        let Some(Some(scene)) = self.live_scene.as_mut() else {
            self.live_in_use = false;
            return Ok(None);
        };
        // SAFETY: the display is live and `window` is this controller's surface window on it.
        let frame = unsafe { scene.compose(display, screen, window, rect, blur) };
        self.live_in_use = frame.is_some();
        Ok(frame.map(|frame| match frame {
            crate::app::live_backdrop::LiveFrame::Unchanged => LiveSurfaceFrame::Unchanged,
            crate::app::live_backdrop::LiveFrame::Pixels(words) => LiveSurfaceFrame::Pixels(words),
        }))
    }

    /// Whether the last live backdrop request was composed from the redirected frames.
    #[must_use]
    pub fn live_backdrop_in_use(&self) -> bool {
        self.live_in_use
    }

    /// One root readback of the popup rect, only on first show. `capture_drawable_rect` allocates
    /// exactly `width * height * 4` RGBA bytes; `blurred_argb_words` consumes that buffer (and any
    /// downscaled copy) and returns the ARGB words uploaded by `set_surface_backdrop`. The words
    /// are dropped after the upload, so neither buffer outlives this call.
    fn capture_popup_rect(
        &self,
        rect: (i32, i32, u32, u32),
    ) -> Option<crate::backdrop::BackdropCapture> {
        if !rect_inside_root(rect, self.root_geometry()) {
            return None;
        }
        // The controller owns a live display and the rect is fully inside the root.
        crate::backdrop::capture_drawable_rect(self.display, self.root_window(), rect).ok()
    }

    /// Real screen under the popup, blurred like the desktop layer. A failed capture falls back
    /// to the cached desktop layer unchanged.
    fn popup_backdrop_for(
        &mut self,
        rect: (i32, i32, u32, u32),
        opacity: u8,
        blur: u8,
    ) -> Option<Vec<u32>> {
        let capture = self.capture_popup_rect(rect);
        popup_backdrop_words(
            capture,
            || self.desktop_backdrop(rect, opacity, blur),
            opacity,
            blur,
        )
    }

    /// Installs the real screen under an unmapped popup's translucent fills, rounded to its shape.
    /// The popup is still unmapped here, so one root readback of exactly the popup rect is what
    /// lies behind it. Capture failure (or a rect outside the root) keeps the desktop-layer
    /// fallback.
    pub(super) fn install_popup_backdrop(&mut self, id: SurfaceId) -> Result<(), String> {
        let Some((opacity, blur)) = self.instance_mut(id)?.popup_backdrop else {
            return Ok(());
        };
        let rect = self.surface_device_rect(id)?;
        let (width, height) = (rect.width as u32, rect.height as u32);
        let words = if opacity < 100 && width > 0 && height > 0 {
            self.popup_backdrop_for((rect.x as i32, rect.y as i32, width, height), opacity, blur)
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

#[cfg(test)]
mod round_backdrop_tests {
    use super::*;

    /// The pre-optimisation per-pixel rounding, kept as the reference.
    fn round_reference(words: &mut [u32], width: u32, height: u32, radius: u32) {
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

    #[test]
    fn interior_skip_matches_the_per_pixel_rounding_exactly() {
        for (width, height, radius) in [
            (300u32, 320u32, 14u32),
            (301, 133, 9),
            (7, 5, 3),
            (1, 40, 12),
            (40, 1, 12),
            (61, 61, 200),
            (120, 44, 0),
        ] {
            let source: Vec<u32> = (0..width * height)
                .map(|i| 0xff00_0000 | (i.wrapping_mul(2_654_435) & 0x00ff_ffff))
                .collect();
            let mut fast = source.clone();
            let mut reference = source;
            round_backdrop_words(&mut fast, width, height, radius);
            round_reference(&mut reference, width, height, radius);
            assert_eq!(fast, reference, "{width}x{height} r={radius}");
        }
    }
}
