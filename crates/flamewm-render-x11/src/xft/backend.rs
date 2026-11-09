use super::*;

impl XftBackend {
    /// # Safety
    ///
    /// `display` must be a live Xlib display, `drawable`/`visual`/`colormap`
    /// valid for that display, and `self` must retain the loaded Xft symbols.
    pub unsafe fn new(
        display: *mut Display,
        screen: c_int,
        drawable: Drawable,
        visual: *mut Visual,
        colormap: Colormap,
    ) -> Result<Self, String> {
        // Xft and XRender register Xlib close-display hooks. Keep both DSOs mapped so
        // XCloseDisplay can safely run those hooks even when initialization returns Err.
        let xft_library = DynamicLibrary::open_nodelete(&["libXft.so.2", "libXft.so"])?;
        let _xrender_library =
            DynamicLibrary::open_nodelete(&["libXrender.so.1", "libXrender.so"])?;
        // SAFETY: each name is a static NUL-terminated symbol and `T` is the exact
        // function-pointer type declared in `XftApi`, matching the C ABI.
        let api = XftApi {
            draw_create: unsafe { xft_library.symbol(b"XftDrawCreate\0") }?,
            draw_destroy: unsafe { xft_library.symbol(b"XftDrawDestroy\0") }?,
            draw_create_alpha: unsafe { xft_library.symbol(b"XftDrawCreateAlpha\0").ok() },
            draw_change: unsafe { xft_library.symbol(b"XftDrawChange\0") }?,
            draw_string_utf8: unsafe { xft_library.symbol(b"XftDrawStringUtf8\0") }?,
            draw_set_clip: unsafe { xft_library.symbol(b"XftDrawSetClip\0").ok() },
            draw_set_clip_rectangles: unsafe {
                xft_library.symbol(b"XftDrawSetClipRectangles\0").ok()
            },
            text_extents_utf8: unsafe { xft_library.symbol(b"XftTextExtentsUtf8\0").ok() },
            font_open_name: unsafe { xft_library.symbol(b"XftFontOpenName\0") }?,
            font_close: unsafe { xft_library.symbol(b"XftFontClose\0") }?,
            color_alloc_value: unsafe { xft_library.symbol(b"XftColorAllocValue\0") }?,
            color_free: unsafe { xft_library.symbol(b"XftColorFree\0") }?,
        };

        let fontconfig_library = match DynamicLibrary::open(&[
            "libfontconfig.so.1",
            "libfontconfig.so",
        ]) {
            Ok(library) => {
                // SAFETY: same contract as above; targets are the `FontconfigApi` ABI types.
                let fc = FontconfigApi {
                    config_get_current: unsafe { library.symbol(b"FcConfigGetCurrent\0") }?,
                    config_app_font_add_file: unsafe {
                        library.symbol(b"FcConfigAppFontAddFile\0")
                    }?,
                    config_build_fonts: unsafe { library.symbol(b"FcConfigBuildFonts\0") }?,
                };
                // SAFETY: `fc` holds live fontconfig symbols; paths are validated inside.
                unsafe { register_application_fonts(fc) }?;
                Some(library)
            }
            Err(error) => {
                eprintln!(
                    "FLAMEWM_RENDER_FONTCONFIG_WARNING {error}; only system-visible fonts can be used"
                );
                None
            }
        };

        let draw = unsafe { (api.draw_create)(display, drawable, visual, colormap) };
        if draw.is_null() {
            return Err("XftDrawCreate returned NULL".to_string());
        }

        let preferred_family = env::var("FLAMEWM_RENDER_UI_FONT")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "IBM Plex Sans".to_string());

        let mut backend = Self {
            display,
            screen,
            visual,
            colormap,
            draw,
            api,
            _xft_library: Some(xft_library),
            _fontconfig_library: fontconfig_library,
            fonts: HashMap::new(),
            colors: HashMap::new(),
            preferred_family,
            text_masks: TextMaskCache::default(),
            readbacks: 0,
        };

        // Fail early only if *no* usable Xft font exists. IBM Plex Sans remains
        // the preferred face, with conservative system fallbacks for portability.
        // SAFETY: `display` is live per `new`'s contract; sentinels probe font availability.
        unsafe { backend.font_for(13.0, 400) }?;
        Ok(backend)
    }

    pub unsafe fn set_drawable(&mut self, drawable: Drawable) {
        // SAFETY: `self.draw` is a live XftDraw and caller guarantees `drawable`
        // is valid for `display` per this method's contract.
        unsafe { (self.api.draw_change)(self.draw, drawable) };
    }

    /// Restricts later glyph draws to `clip` (device pixels, `x y w h`); `None` removes the
    /// restriction. Xft keeps its own draw clip, so the GC clip stack does not reach text.
    /// # Safety
    /// `self.draw` must be a live XftDraw on this backend's display.
    pub unsafe fn set_clip(&mut self, clip: Option<(i32, i32, u32, u32)>) {
        match clip {
            Some((x, y, width, height)) => {
                let Some(set_rects) = self.api.draw_set_clip_rectangles else {
                    return;
                };
                let rect = XRectangle {
                    x: x.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
                    y: y.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
                    width: width.min(u16::MAX as u32) as u16,
                    height: height.min(u16::MAX as u32) as u16,
                };
                // SAFETY: `self.draw` is live per the contract; `rect` outlives the call.
                unsafe { set_rects(self.draw, 0, 0, &rect, 1) };
            }
            None => {
                let Some(set_clip) = self.api.draw_set_clip else {
                    return;
                };
                // SAFETY: `self.draw` is live per the contract; a null region removes the clip.
                unsafe { set_clip(self.draw, ptr::null()) };
            }
        }
    }

    pub unsafe fn set_preferred_family(&mut self, family: &str) {
        let family = family.trim();
        if family.is_empty() || family == self.preferred_family {
            return;
        }
        for (_, font) in self.fonts.drain() {
            // SAFETY: drained fonts were opened on `self.display` and owned here.
            unsafe { (self.api.font_close)(self.display, font) };
        }
        self.preferred_family = family.to_string();
        self.text_masks.clear();
    }

    pub unsafe fn draw_text(
        &mut self,
        x: f32,
        baseline_y: f32,
        color: Color,
        size: f32,
        weight: u16,
        text: &str,
    ) -> Result<(), String> {
        if text.is_empty() {
            return Ok(());
        }
        // SAFETY: `display` stays live for the backend lifetime; the color cache
        // above holds only valid Xft allocations returned on this display.
        let font = unsafe { self.font_for(size, weight) }?;
        let xft_color = unsafe { self.color_for(color) }?;
        let bytes = text.as_bytes();
        let len = bytes.len().min(i32::MAX as usize) as c_int;
        // SAFETY: `self.draw`/`font` are live Xft handles, `xft_color` is a valid
        // allocated color, and `bytes` outlives the call for `len` bytes.
        unsafe {
            (self.api.draw_string_utf8)(
                self.draw,
                &xft_color,
                font,
                x.round() as c_int,
                baseline_y.round() as c_int,
                bytes.as_ptr(),
                len,
            )
        };
        Ok(())
    }

    /// Coverage of `text` as a bounded A8 mask (`pad` px margin), cached per (family, text, size,
    /// weight, pad). A miss renders through the batched path. Paints should call
    /// [`Self::prefetch_text_masks`] first so a whole paint costs one readback, not one per run.
    /// # Safety
    /// This backend must hold the live X display, font and Xft library.
    pub unsafe fn text_alpha_mask(
        &mut self,
        size: f32,
        weight: u16,
        text: &str,
        pad: u32,
        mask: &mut Vec<u8>,
    ) -> Result<(u32, u32), String> {
        let key = TextMaskRef::new(&self.preferred_family, text, size, weight, pad);
        if let Some(dims) = self.text_masks.get_into(&key, mask) {
            return Ok(dims);
        }
        let spec = AlphaMaskSpec {
            size,
            weight,
            text,
            pad,
        };
        let rendered = unsafe { self.render_alpha_masks(&[spec]) }
            .pop()
            .unwrap_or_else(|| Err("text mask batch returned no entry".to_string()))?;
        *mask = rendered.bytes;
        Ok((rendered.width, rendered.height))
    }

    /// Renders every uncached run in `specs` with as few readbacks as possible (one per batch of
    /// at most [`MAX_MASK_BATCH_HEIGHT`] rows). Results land in the mask cache; failures are left
    /// uncached, and the per-run path retries them.
    /// # Safety
    /// This backend must hold the live X display, font and Xft library.
    pub unsafe fn prefetch_text_masks(&mut self, specs: &[AlphaMaskSpec<'_>]) {
        let mut seen = std::collections::HashSet::new();
        let mut missing = Vec::new();
        for spec in specs {
            let key = TextMaskRef::new(
                &self.preferred_family,
                spec.text,
                spec.size,
                spec.weight,
                spec.pad,
            );
            if self.text_masks.contains(&key) {
                continue;
            }
            let owned = TextMaskKey::new(
                &self.preferred_family,
                spec.text,
                spec.size,
                spec.weight,
                spec.pad,
            );
            if seen.insert(owned) {
                missing.push(*spec);
            }
        }
        if !missing.is_empty() {
            let _ = unsafe { self.render_alpha_masks(&missing) };
        }
    }
}
