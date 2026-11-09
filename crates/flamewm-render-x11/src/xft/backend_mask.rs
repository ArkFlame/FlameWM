use super::*;

impl XftBackend {
    /// Renders each spec into its own A8 mask and inserts successes into the cache. Specs are
    /// laid out as horizontal bands of one pixmap, each clipped to its own band, so every mask
    /// matches a standalone render pixel for pixel. One `XGetImage` covers each batch.
    pub(super) unsafe fn render_alpha_masks(
        &mut self,
        specs: &[AlphaMaskSpec<'_>],
    ) -> Vec<Result<AlphaMask, String>> {
        let mut out: Vec<Option<Result<AlphaMask, String>>> = specs.iter().map(|_| None).collect();
        let dims: Vec<(u32, u32)> = specs
            .iter()
            .map(|spec| {
                let (advance, _) = unsafe { self.measure_text(spec.size, spec.weight, spec.text) };
                let width =
                    ((advance.ceil().max(1.0) as u32).saturating_add(spec.pad * 2 + 8)).min(1024);
                let height = ((spec.size.max(1.0).ceil() as u32)
                    .saturating_mul(2)
                    .saturating_add(spec.pad * 2))
                .min(192);
                (width, height)
            })
            .collect();
        // Without XftDrawSetClipRectangles, bands could bleed into each other: one spec per batch.
        let max_height = if self.api.draw_set_clip_rectangles.is_some() {
            MAX_MASK_BATCH_HEIGHT
        } else {
            0
        };
        let mut start = 0;
        while start < specs.len() {
            let mut end = start;
            let mut total = 0;
            while end < specs.len() && (end == start || total + dims[end].1 <= max_height) {
                total += dims[end].1;
                end += 1;
            }
            match unsafe { self.render_mask_batch(&specs[start..end], &dims[start..end]) } {
                Ok(masks) => {
                    for (offset, mask) in masks.into_iter().enumerate() {
                        let spec = specs[start + offset];
                        self.text_masks.insert(
                            TextMaskKey::new(
                                &self.preferred_family,
                                spec.text,
                                spec.size,
                                spec.weight,
                                spec.pad,
                            ),
                            mask.width,
                            mask.height,
                            &mask.bytes,
                        );
                        out[start + offset] = Some(Ok(mask));
                    }
                }
                Err(error) => {
                    for slot in &mut out[start..end] {
                        *slot = Some(Err(error.clone()));
                    }
                }
            }
            start = end;
        }
        out.into_iter()
            .map(|slot| slot.unwrap_or_else(|| Err("text mask missing from batch".to_string())))
            .collect()
    }

    /// One pixmap holding every band of `specs`, one `XGetImage`, then each band copied out.
    pub(super) unsafe fn render_mask_batch(
        &mut self,
        specs: &[AlphaMaskSpec<'_>],
        dims: &[(u32, u32)],
    ) -> Result<Vec<AlphaMask>, String> {
        let create_alpha = self
            .api
            .draw_create_alpha
            .ok_or("XftDrawCreateAlpha unavailable")?;
        let set_clip = self
            .api
            .draw_set_clip_rectangles
            .ok_or("XftDrawSetClipRectangles unavailable")?;
        let total_width = dims.iter().map(|d| d.0).max().unwrap_or(1).max(1);
        let total_height: u32 = dims.iter().map(|d| d.1).sum::<u32>().max(1);
        let pixmap = unsafe {
            XCreatePixmap(
                self.display,
                XRootWindow(self.display, self.screen),
                total_width,
                total_height,
                8,
            )
        };
        if pixmap == 0 {
            return Err("Xft text shadow A8 pixmap unavailable".to_string());
        }
        let result = (|| {
            let gc = unsafe { XCreateGC(self.display, pixmap, 0, ptr::null_mut()) };
            if gc.is_null() {
                return Err("text shadow A8 GC unavailable".to_string());
            }
            unsafe {
                XSetForeground(self.display, gc, 0);
                XFillRectangle(self.display, pixmap, gc, 0, 0, total_width, total_height);
                XFreeGC(self.display, gc);
            }
            let draw = unsafe { create_alpha(self.display, pixmap, 8) };
            if draw.is_null() {
                return Err("XftDrawCreateAlpha returned NULL".to_string());
            }
            let draw_result = (|| {
                let white = XftColor {
                    pixel: 0,
                    color: XRenderColor {
                        red: 65535,
                        green: 65535,
                        blue: 65535,
                        alpha: 65535,
                    },
                };
                let mut band_top = 0u32;
                let mut bands = Vec::with_capacity(specs.len());
                for (spec, &(width, height)) in specs.iter().zip(dims) {
                    let font = unsafe { self.font_for(spec.size, spec.weight) }?;
                    let clip = XRectangle {
                        x: 0,
                        y: band_top as i16,
                        width: width as u16,
                        height: height as u16,
                    };
                    unsafe { set_clip(draw, 0, 0, &clip, 1) };
                    let bytes = spec.text.as_bytes();
                    unsafe {
                        (self.api.draw_string_utf8)(
                            draw,
                            &white,
                            font,
                            spec.pad as c_int + 4,
                            band_top as c_int + spec.pad as c_int + spec.size.ceil() as c_int,
                            bytes.as_ptr(),
                            bytes.len().min(c_int::MAX as usize) as c_int,
                        );
                    }
                    bands.push(band_top);
                    band_top += height;
                }
                self.readbacks += 1;
                let image = unsafe {
                    XGetImage(
                        self.display,
                        pixmap,
                        0,
                        0,
                        total_width,
                        total_height,
                        !0,
                        ZPIXMAP,
                    )
                };
                if image.is_null() {
                    return Err("XGetImage of A8 text shadow failed".to_string());
                }
                let extracted = (|| {
                    let head = unsafe { &*(image as *const XImageHead) };
                    if head.bits_per_pixel != 8 {
                        return Err("text shadow A8 image is not 8 bits per pixel".to_string());
                    }
                    let row_bytes = head.bytes_per_line as usize;
                    let data = unsafe {
                        std::slice::from_raw_parts(
                            head.data as *const u8,
                            row_bytes * total_height as usize,
                        )
                    };
                    Ok(dims
                        .iter()
                        .zip(&bands)
                        .map(|(&(width, height), &top)| {
                            let mut bytes = vec![0u8; width as usize * height as usize];
                            for (y, row) in bytes.chunks_exact_mut(width as usize).enumerate() {
                                let start = (top as usize + y) * row_bytes;
                                row.copy_from_slice(&data[start..start + width as usize]);
                            }
                            AlphaMask {
                                width,
                                height,
                                bytes,
                            }
                        })
                        .collect::<Vec<_>>())
                })();
                unsafe { XDestroyImage(image) };
                extracted
            })();
            unsafe { (self.api.draw_destroy)(draw) };
            draw_result
        })();
        unsafe { crate::native::xresource::free_pixmap_raw(self.display, pixmap) };
        result
    }

    #[cfg(test)]
    /// The original single-pixmap render, kept as the pixel reference for batching tests.
    /// Render a bounded antialiased A8 glyph coverage mask into `mask` (reused capacity), once per retained paint.
    /// A real alpha mask is blurred by the shared image engine; never overlay shifted glyphs.
    /// # Safety
    /// This backend must hold the live X display, font and Xft library.
    pub unsafe fn reference_alpha_mask(
        &mut self,
        size: f32,
        weight: u16,
        text: &str,
        pad: u32,
        mask: &mut Vec<u8>,
    ) -> Result<(u32, u32), String> {
        // Coverage depends only on family, glyphs, size (scale is folded in),
        // weight and pad; a hit skips the A8 pixmap, GC, draw and XGetImage.
        let key = TextMaskRef::new(&self.preferred_family, text, size, weight, pad);
        if let Some(dims) = self.text_masks.get_into(&key, mask) {
            return Ok(dims);
        }
        let create_alpha = self
            .api
            .draw_create_alpha
            .ok_or("XftDrawCreateAlpha unavailable")?;
        let (advance, _) = unsafe { self.measure_text(size, weight, text) };
        let width = ((advance.ceil().max(1.0) as u32).saturating_add(pad * 2 + 8)).min(1024);
        let height = ((size.max(1.0).ceil() as u32)
            .saturating_mul(2)
            .saturating_add(pad * 2))
        .min(192);
        let pixmap = unsafe {
            XCreatePixmap(
                self.display,
                XRootWindow(self.display, self.screen),
                width,
                height,
                8,
            )
        };
        if pixmap == 0 {
            return Err("Xft text shadow A8 pixmap unavailable".to_string());
        }
        let result = (|| {
            let gc = unsafe { XCreateGC(self.display, pixmap, 0, ptr::null_mut()) };
            if gc.is_null() {
                return Err("text shadow A8 GC unavailable".to_string());
            }
            unsafe {
                XSetForeground(self.display, gc, 0);
                XFillRectangle(self.display, pixmap, gc, 0, 0, width, height);
                XFreeGC(self.display, gc);
            }
            let draw = unsafe { create_alpha(self.display, pixmap, 8) };
            if draw.is_null() {
                return Err("XftDrawCreateAlpha returned NULL".to_string());
            }
            let draw_result = (|| {
                let font = unsafe { self.font_for(size, weight) }?;
                let white = XftColor {
                    pixel: 0,
                    color: XRenderColor {
                        red: 65535,
                        green: 65535,
                        blue: 65535,
                        alpha: 65535,
                    },
                };
                let bytes = text.as_bytes();
                unsafe {
                    (self.api.draw_string_utf8)(
                        draw,
                        &white,
                        font,
                        pad as c_int + 4,
                        pad as c_int + size.ceil() as c_int,
                        bytes.as_ptr(),
                        bytes.len().min(c_int::MAX as usize) as c_int,
                    );
                }
                let image =
                    unsafe { XGetImage(self.display, pixmap, 0, 0, width, height, !0, ZPIXMAP) };
                if image.is_null() {
                    return Err("XGetImage of A8 text shadow failed".to_string());
                }
                let result = (|| {
                    let head = unsafe { &*(image as *const XImageHead) };
                    if head.bits_per_pixel != 8
                        || head.bytes_per_line < width as i32
                        || head.data.is_null()
                    {
                        return Err("unsupported A8 text shadow pixel layout".to_string());
                    }
                    let row_bytes = head.bytes_per_line as usize;
                    let data = unsafe {
                        std::slice::from_raw_parts(
                            head.data as *const u8,
                            row_bytes * height as usize,
                        )
                    };
                    mask.clear();
                    mask.resize(width as usize * height as usize, 0);
                    for (y, row) in mask.chunks_exact_mut(width as usize).enumerate() {
                        row.copy_from_slice(&data[y * row_bytes..y * row_bytes + width as usize]);
                    }
                    Ok((width, height))
                })();
                unsafe { XDestroyImage(image) };
                result
            })();
            unsafe { (self.api.draw_destroy)(draw) };
            draw_result
        })();
        unsafe { crate::native::xresource::free_pixmap_raw(self.display, pixmap) };
        if let Ok(dims) = result {
            let owned = TextMaskKey::new(&self.preferred_family, text, size, weight, pad);
            self.text_masks.insert(owned, dims.0, dims.1, mask);
        }
        result
    }
}
