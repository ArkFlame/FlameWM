use super::*;

impl XftBackend {
    /// Reusable measurement for decorations: true Xft advance when
    /// `XftTextExtentsUtf8` is available, else the canonical layout
    /// estimate (0.58em/char, 1.30em height) so callers never branch.
    ///
    /// # Safety
    /// `display` stays live for the backend lifetime (same as `draw_text`).
    pub unsafe fn measure_text(&mut self, size: f32, weight: u16, text: &str) -> (f32, f32) {
        let estimate = xft_measure_estimate(text, size);
        if text.is_empty() {
            return estimate;
        }
        let Ok(font) = (unsafe { self.font_for(size, weight) }) else {
            return estimate;
        };
        let Some(extents_fn) = self.api.text_extents_utf8 else {
            return estimate;
        };
        let bytes = text.as_bytes();
        let len = bytes.len().min(c_int::MAX as usize) as c_int;
        let mut info = XGlyphInfo::default();
        // SAFETY: `font` is a live Xft handle from `font_for`, `bytes`
        // outlives the call for `len` bytes, `info` is a valid out-pointer.
        unsafe {
            extents_fn(
                self.display,
                font,
                bytes.as_ptr(),
                len,
                &mut info as *mut XGlyphInfo,
            )
        };
        ((info.x_off as f32).max(0.0), estimate.1)
    }

    pub(super) unsafe fn font_for(
        &mut self,
        size: f32,
        weight: u16,
    ) -> Result<*mut XftFont, String> {
        let key = FontKey {
            pixels: size.round().clamp(6.0, 96.0) as u16,
            weight: normalize_weight(weight),
        };
        if let Some(font) = self.fonts.get(&key).copied() {
            return Ok(font);
        }

        let weight_name = match key.weight {
            0..=400 => "regular",
            401..=500 => "medium",
            501..=600 => "semibold",
            _ => "bold",
        };
        let mut families = Vec::with_capacity(4);
        families.push(self.preferred_family.as_str());
        if self.preferred_family != "IBM Plex Sans" {
            families.push("IBM Plex Sans");
        }
        families.extend(["Noto Sans", "DejaVu Sans", "sans-serif"]);

        for family in families {
            let pattern = format!(
                "{family}:pixelsize={}:weight={weight_name}:antialias=true:hinting=true:hintstyle=hintslight:rgba=none",
                key.pixels
            );
            let c_pattern =
                CString::new(pattern).map_err(|_| "font pattern contains NUL".to_string())?;
            // SAFETY: `display` is live per `new`'s contract and `c_pattern` stays
            // alive for the call; NULL return is handled by trying the next family.
            let font =
                unsafe { (self.api.font_open_name)(self.display, self.screen, c_pattern.as_ptr()) };
            if !font.is_null() {
                self.fonts.insert(key, font);
                return Ok(font);
            }
        }
        Err(format!("Xft could not open a UI font at {} px", key.pixels))
    }

    pub(super) unsafe fn color_for(&mut self, color: Color) -> Result<XftColor, String> {
        if let Some(value) = self.colors.get(&color).copied() {
            return Ok(value);
        }
        let (red, green, blue, alpha) = xrender_color_for(color);
        let render = XRenderColor {
            red,
            green,
            blue,
            alpha,
        };
        let mut allocated = XftColor {
            pixel: 0,
            color: render,
        };
        // SAFETY: `display`/`visual`/`colormap` are live per the backend contract;
        // `render`/`allocated` are valid stack references for the duration of the call.
        if unsafe {
            (self.api.color_alloc_value)(
                self.display,
                self.visual,
                self.colormap,
                &render,
                &mut allocated,
            )
        } == 0
        {
            return Err(format!(
                "XftColorAllocValue failed for rgba({},{},{},{})",
                color.r, color.g, color.b, color.a
            ));
        }
        self.colors.insert(color, allocated);
        Ok(allocated)
    }

    /// Release Xft-owned display resources while the display is live, then
    /// transfer the dynamic-library handles to the display lifetime owner.
    /// Function pointers and Xft objects are never allowed to outlive these
    /// handles; the returned libraries must remain alive through XCloseDisplay.
    pub(crate) unsafe fn into_deferred_libraries(mut self) -> Vec<DynamicLibrary> {
        unsafe { self.release_resources() };
        let libraries = [self._xft_library.take(), self._fontconfig_library.take()]
            .into_iter()
            .flatten()
            .collect();
        drop(self);
        libraries
    }

    pub(super) unsafe fn release_resources(&mut self) {
        for (_, mut color) in self.colors.drain() {
            // SAFETY: the display, visual, and colormap remain live; drained colors are backend-owned.
            unsafe { (self.api.color_free)(self.display, self.visual, self.colormap, &mut color) };
        }
        for (_, font) in self.fonts.drain() {
            // SAFETY: the display remains live and the drained font is closed exactly once.
            unsafe { (self.api.font_close)(self.display, font) };
        }
        if !self.draw.is_null() {
            // SAFETY: `draw` is a live backend-owned handle and its library remains loaded.
            unsafe { (self.api.draw_destroy)(self.draw) };
            self.draw = ptr::null_mut();
        }
    }
}

impl Drop for XftBackend {
    fn drop(&mut self) {
        // SAFETY: teardown only touches backend-owned Xft handles while `display`
        // remains live; draining first guarantees each handle is freed once.
        unsafe { self.release_resources() };
    }
}
