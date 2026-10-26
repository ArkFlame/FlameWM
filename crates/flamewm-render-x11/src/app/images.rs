//! Image cache {pixmap,picture,mask} keyed by {node,asset,revision,w,h,treatment,foreground}.
//! SymbolicForeground premul math: eff_a=round(sa*fa/255),
//! premul=round(fg*eff_a/255). No feature-specific icon branches.
//! Runtime per-node overrides are never keyed by compiled asset_id alone:
//! the node index plus the node-local revision are part of every key.

use std::sync::Once;

use super::color::rgb_to_pixel;
use super::*;
use flamewm_render_core::ImageTreatment;

static MASK_LOG_ONCE: Once = Once::new();

const MAX_IMAGE_CACHE_ENTRIES: usize = 64;
const MAX_IMAGE_CACHE_BYTES: usize = 48 * 1024 * 1024;

fn log_degraded_mask_once(asset_id: u16) {
    MASK_LOG_ONCE.call_once(|| {
        eprintln!(
            "FLAMEWM_RENDER_IMAGE_ALPHA degraded-clip-mask asset={asset_id} reason=xrender-unavailable"
        );
    });
}

/// Creates a ZPIXMAP XImage with zeroed, malloc'd data (XDestroyImage frees it).
/// Returns the image and its data size. Caller must XDestroyImage on success.
pub(crate) unsafe fn create_zeroed_ximage(
    display: *mut Display,
    visual: *mut Visual,
    depth: u32,
    what: &str,
    width: u32,
    height: u32,
) -> Result<(*mut XImage, usize), String> {
    let ximage = unsafe {
        XCreateImage(
            display,
            visual,
            depth,
            ZPIXMAP,
            0,
            ptr::null_mut(),
            width,
            height,
            32,
            0,
        )
    };
    if ximage.is_null() {
        return Err(format!("XCreateImage failed for {what}"));
    }
    let head = unsafe { &mut *(ximage as *mut XImageHead) };
    if head.bytes_per_line <= 0 {
        unsafe { XDestroyImage(ximage) };
        return Err("XCreateImage produced invalid bytes_per_line".to_string());
    }
    let total = match (head.bytes_per_line as usize).checked_mul(height as usize) {
        Some(total) => total,
        None => {
            unsafe { XDestroyImage(ximage) };
            return Err("XImage buffer size overflow".to_string());
        }
    };
    let data = unsafe { malloc(total) };
    if data.is_null() {
        unsafe { XDestroyImage(ximage) };
        return Err(format!("malloc failed for {total}-byte XImage"));
    }
    unsafe { ptr::write_bytes(data as *mut u8, 0, total) };
    head.data = data as *mut _;
    Ok((ximage, total))
}

fn copy_argb32_rows(
    source: &[u32],
    width: usize,
    height: usize,
    stride: usize,
    destination: &mut [u32],
) -> Result<(), String> {
    let expected = width
        .checked_mul(height)
        .ok_or_else(|| "ARGB32 upload size overflow".to_string())?;
    if source.len() != expected {
        return Err(format!(
            "ARGB32 upload has {} words; expected {expected}",
            source.len()
        ));
    }
    let required = stride
        .checked_mul(height)
        .ok_or_else(|| "ARGB32 image stride overflow".to_string())?;
    if stride < width || destination.len() < required {
        return Err("ARGB32 image stride is smaller than its pixel rows".to_string());
    }
    for y in 0..height {
        let dst = y * stride;
        let src = y * width;
        destination[dst..dst + width].copy_from_slice(&source[src..src + width]);
    }
    Ok(())
}

/// Uploads ARGB32 words into a pixmap using a temporary pixmap-bound GC.
///
/// # Safety
/// `display` must be live, `pixmap` must accept `visual` at depth 32, and the
/// caller must own the pixmap for the duration of this call.
pub(crate) unsafe fn upload_argb32_image(
    display: *mut Display,
    pixmap: Pixmap,
    visual: *mut Visual,
    argb: &[u32],
    width: u32,
    height: u32,
    what: &str,
) -> Result<(), String> {
    if visual.is_null() {
        return Err(format!("{what}: ARGB32 visual is NULL"));
    }
    let (ximage, total) =
        unsafe { create_zeroed_ximage(display, visual, 32, what, width, height)? };
    let head = unsafe { &*(ximage as *const XImageHead) };
    let bytes_per_line = head.bytes_per_line as usize;
    if bytes_per_line % std::mem::size_of::<u32>() != 0 {
        unsafe { XDestroyImage(ximage) };
        return Err(format!("{what}: XImage row stride is not word-aligned"));
    }
    let stride = bytes_per_line / std::mem::size_of::<u32>();
    let raw = unsafe { std::slice::from_raw_parts_mut(head.data as *mut u32, total / 4) };
    if let Err(error) = copy_argb32_rows(argb, width as usize, height as usize, stride, raw) {
        unsafe { XDestroyImage(ximage) };
        return Err(format!("{what}: {error}"));
    }
    let pixmap_gc = unsafe { XCreateGC(display, pixmap, 0, ptr::null_mut()) };
    if pixmap_gc.is_null() {
        unsafe { XDestroyImage(ximage) };
        return Err(format!("XCreateGC failed for {what}"));
    }
    unsafe {
        XPutImage(
            display, pixmap, pixmap_gc, ximage, 0, 0, 0, 0, width, height,
        );
        XFreeGC(display, pixmap_gc);
        XDestroyImage(ximage);
    }
    Ok(())
}

pub(crate) fn image_cache_key(
    node: u32,
    asset_id: u16,
    width: u32,
    height: u32,
    revision: u64,
    treatment: ImageTreatment,
    fg: Color,
) -> ImageCacheKey {
    let treatment_byte = match treatment {
        ImageTreatment::Original => 0u8,
        ImageTreatment::SymbolicForeground => 1u8,
    };
    ImageCacheKey {
        node,
        asset: asset_id,
        width,
        height,
        revision,
        treatment: treatment_byte,
        fg_r: fg.r,
        fg_g: fg.g,
        fg_b: fg.b,
        fg_a: fg.a,
    }
}

impl X11App {
    fn refresh_image_cache_gauges(&self) {
        let total: usize = self.images.values().map(|image| image.bytes).sum();
        refresh_image_gauge(total as u64);
    }

    pub(super) fn alloc_pixmap(
        &mut self,
        drawable: Drawable,
        width: u32,
        height: u32,
        depth: u32,
    ) -> Result<(Pixmap, usize), String> {
        let (root_w, root_h) = self.root_extent();
        unsafe {
            self.allocator.create_pixmap(
                self.display,
                drawable,
                width,
                height,
                depth,
                root_w,
                root_h,
            )
        }
    }

    pub(super) fn free_pixmap_tracked(&mut self, pixmap: Pixmap, bytes: usize) {
        unsafe { self.allocator.free_pixmap(self.display, pixmap, bytes) };
    }

    unsafe fn evict_node_images(&mut self, key: ImageCacheKey) {
        let stale_keys: Vec<ImageCacheKey> = self
            .images
            .keys()
            .filter(|cached| cached.node == key.node && **cached != key)
            .copied()
            .collect();
        for stale_key in stale_keys {
            if let Some(stale) = self.images.remove(&stale_key) {
                let display = self.display;
                unsafe { free_cached_image(display, &mut self.allocator, stale) };
            }
        }
        self.refresh_image_cache_gauges();
    }

    unsafe fn insert_cached_image(
        &mut self,
        key: ImageCacheKey,
        cached: CachedImage,
    ) -> Result<(), String> {
        if cached.bytes > MAX_IMAGE_CACHE_BYTES {
            let display = self.display;
            unsafe { free_cached_image(display, &mut self.allocator, cached) };
            return Err(format!(
                "image asset {}x{} exceeds {MAX_IMAGE_CACHE_BYTES}-byte cache budget",
                key.width, key.height
            ));
        }
        let mut cache_bytes = self.images.values().map(|image| image.bytes).sum::<usize>();
        while self.images.len() >= MAX_IMAGE_CACHE_ENTRIES
            || cache_bytes.saturating_add(cached.bytes) > MAX_IMAGE_CACHE_BYTES
        {
            let Some(eviction_key) = self.images.keys().next().copied() else {
                break;
            };
            let evicted = self
                .images
                .remove(&eviction_key)
                .expect("cache key must exist");
            cache_bytes = cache_bytes.saturating_sub(evicted.bytes);
            let display = self.display;
            unsafe { free_cached_image(display, &mut self.allocator, evicted) };
        }
        self.images.insert(key, cached);
        self.refresh_image_cache_gauges();
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn image_pixmap(
        &mut self,
        node: u32,
        asset_id: u16,
        pixels: &[u8],
        src_width: u32,
        src_height: u32,
        revision: u64,
        width: u32,
        height: u32,
        treatment: ImageTreatment,
        tint: Option<Color>,
    ) -> Result<CachedImage, String> {
        let fg = tint.unwrap_or(Color {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        });
        let key = image_cache_key(node, asset_id, width, height, revision, treatment, fg);
        if let Some(cached) = self.images.get(&key).copied() {
            return Ok(cached);
        }
        unsafe { self.evict_node_images(key) };
        if self.xrender.is_some() {
            // True-alpha path: 32-bit TrueColor visual pixmap + XPutImage of
            // ARGB32 words. XCreatePixmap against the (24-bit default) window
            // with depth 32 has no matching visual and XPutImage then dies
            // with BadMatch, so match a 32-bit TrueColor visual explicitly.
            let root = unsafe { XRootWindow(self.display, self.screen) };
            let mut vinfo: XVisualInfo = unsafe { std::mem::zeroed() };
            let matched = unsafe {
                XMatchVisualInfo(self.display, self.screen, 32, TRUE_COLOR_CLASS, &mut vinfo)
            };
            if matched != 0 && !vinfo.visual.is_null() {
                let (pixmap, pix_bytes) = match self.alloc_pixmap(root, width, height, 32) {
                    Ok(created) => created,
                    Err(error) => {
                        return Err(format!("image asset {asset_id}: {error}"));
                    }
                };
                let argb = match treatment {
                    ImageTreatment::Original => crate::native::image::scale_and_premultiply(
                        pixels, src_width, src_height, width, height,
                    ),
                    ImageTreatment::SymbolicForeground => super::symbolic_foreground_argb32(
                        pixels, src_width, src_height, width, height, fg,
                    ),
                };
                if let Err(error) =
                    unsafe { self.upload_argb32_words(pixmap, vinfo.visual, &argb, width, height) }
                {
                    self.free_pixmap_tracked(pixmap, pix_bytes);
                    return Err(error);
                }
                let cached = CachedImage {
                    pixmap,
                    picture: 1,
                    mask: 0,
                    mask_bytes: 0,
                    bytes: pix_bytes,
                };
                unsafe { self.insert_cached_image(key, cached) }?;
                return Ok(cached);
            }
            eprintln!(
                "FLAMEWM_RENDER_IMAGE_ALPHA degraded-clip-mask asset={asset_id} reason=no-32bit-visual"
            );
        }
        // Degraded path (no XRender): depth pixmap + 1-bit threshold mask.
        log_degraded_mask_once(asset_id);
        let (pixmap, pix_bytes) =
            match self.alloc_pixmap(self.window, width, height, self.depth as u32) {
                Ok(created) => created,
                Err(error) => return Err(format!("image asset {asset_id}: {error}")),
            };
        if let Err(error) = unsafe {
            self.upload_opaque_fallback(pixmap, pixels, src_width, src_height, width, height)
        } {
            self.free_pixmap_tracked(pixmap, pix_bytes);
            return Err(error);
        }
        let mask = match unsafe {
            self.build_threshold_mask(pixels, src_width, src_height, width, height, asset_id)
        } {
            Ok(mask) => mask,
            Err(error) => {
                self.free_pixmap_tracked(pixmap, pix_bytes);
                return Err(error);
            }
        };
        let mask_bytes = if mask == 0 {
            0
        } else {
            super::native::xresource::pixmap_byte_estimate(width, height, 1)
        };
        let cached = CachedImage {
            pixmap,
            picture: 0,
            mask,
            mask_bytes,
            bytes: pix_bytes.saturating_add(mask_bytes),
        };
        unsafe { self.insert_cached_image(key, cached) }?;
        Ok(cached)
    }

    pub(crate) unsafe fn upload_argb32_words(
        &mut self,
        pixmap: Pixmap,
        visual: *mut Visual,
        argb: &[u32],
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        if visual.is_null() {
            return Err("ARGB32 visual is NULL".to_string());
        }
        unsafe {
            upload_argb32_image(
                self.display,
                pixmap,
                visual,
                argb,
                width,
                height,
                "ARGB32 upload",
            )
        }
    }

    unsafe fn upload_opaque_fallback(
        &mut self,
        pixmap: Pixmap,
        pixels: &[u8],
        src_width: u32,
        src_height: u32,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let visual = unsafe { XDefaultVisual(self.display, self.screen) };
        if visual.is_null() {
            return Err("XDefaultVisual returned NULL".to_string());
        }
        let (ximage, _) = unsafe {
            create_zeroed_ximage(
                self.display,
                visual,
                self.depth as u32,
                "image fallback",
                width,
                height,
            )?
        };
        let head = unsafe { &*(ximage as *const XImageHead) };
        for y in 0..height {
            let sy = ((y as u64 * src_height as u64) / height as u64) as u32;
            for x in 0..width {
                let sx = ((x as u64 * src_width as u64) / width as u64) as u32;
                let offset = ((sy as usize * src_width as usize) + sx as usize) * 4;
                let pixel = rgb_to_pixel(
                    pixels[offset],
                    pixels[offset + 1],
                    pixels[offset + 2],
                    head.red_mask as u64,
                    head.green_mask as u64,
                    head.blue_mask as u64,
                );
                unsafe { XPutPixel(ximage, x as i32, y as i32, pixel as _) };
            }
        }
        unsafe {
            XPutImage(
                self.display,
                pixmap,
                self.gc,
                ximage,
                0,
                0,
                0,
                0,
                width,
                height,
            )
        };
        unsafe { XDestroyImage(ximage) };
        Ok(())
    }

    unsafe fn build_threshold_mask(
        &mut self,
        pixels: &[u8],
        src_width: u32,
        src_height: u32,
        width: u32,
        height: u32,
        asset_id: u16,
    ) -> Result<Pixmap, String> {
        let mut transparent = vec![false; (width as usize) * (height as usize)];
        for y in 0..height {
            let sy = ((y as u64 * src_height as u64) / height as u64) as u32;
            for x in 0..width {
                let sx = ((x as u64 * src_width as u64) / width as u64) as u32;
                let a = pixels[((sy as usize * src_width as usize) + sx as usize) * 4 + 3];
                transparent[(y as usize) * (width as usize) + x as usize] = a < 128;
            }
        }
        if !transparent.iter().any(|t| *t) {
            return Ok(0);
        }
        let (root_w, root_h) = self.root_extent();
        let (mask, _) = unsafe {
            self.allocator.create_pixmap(
                self.display,
                self.window,
                width,
                height,
                1,
                root_w,
                root_h,
            )
        }
        .map_err(|error| format!("transparency mask asset {asset_id}: {error}"))?;
        let mask_gc = unsafe { XCreateGC(self.display, mask, 0, ptr::null_mut()) };
        if mask_gc.is_null() {
            self.free_pixmap_tracked(
                mask,
                super::native::xresource::pixmap_byte_estimate(width, height, 1),
            );
            return Err(format!(
                "XCreateGC failed for transparency mask asset {asset_id}"
            ));
        }
        unsafe { XSetForeground(self.display, mask_gc, 0) };
        unsafe { XFillRectangle(self.display, mask, mask_gc, 0, 0, width, height) };
        unsafe { XSetForeground(self.display, mask_gc, 1) };
        for y in 0..height {
            let mut x = 0u32;
            while x < width {
                while x < width && transparent[(y as usize) * (width as usize) + x as usize] {
                    x += 1;
                }
                let start = x;
                while x < width && !transparent[(y as usize) * (width as usize) + x as usize] {
                    x += 1;
                }
                if x > start {
                    unsafe {
                        XFillRectangle(
                            self.display,
                            mask,
                            mask_gc,
                            start as i32,
                            y as i32,
                            x - start,
                            1,
                        )
                    };
                }
            }
        }
        unsafe { XFreeGC(self.display, mask_gc) };
        Ok(mask)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_budget_admits_one_4k_rgba_surface_and_stays_bounded() {
        let four_k_rgba = 3840usize * 2160 * 4;
        assert!(four_k_rgba <= MAX_IMAGE_CACHE_BYTES);
        const { assert!(MAX_IMAGE_CACHE_BYTES <= 64 * 1024 * 1024) };
        assert!(four_k_rgba * 2 > MAX_IMAGE_CACHE_BYTES);
    }

    #[test]
    fn argb32_row_copy_respects_ximage_stride() {
        let source = [0x1122_3344, 0x5566_7788, 0x99aa_bbcc, 0xddee_ff00];
        let mut destination = [0xdead_beef; 6];

        copy_argb32_rows(&source, 2, 2, 3, &mut destination).unwrap();

        assert_eq!(
            destination,
            [
                0x1122_3344,
                0x5566_7788,
                0xdead_beef,
                0x99aa_bbcc,
                0xddee_ff00,
                0xdead_beef
            ]
        );
    }

    #[test]
    fn argb32_row_copy_rejects_short_source_and_stride() {
        assert!(copy_argb32_rows(&[1, 2, 3], 2, 2, 2, &mut [0; 4]).is_err());
        assert!(copy_argb32_rows(&[1, 2, 3, 4], 2, 2, 1, &mut [0; 2]).is_err());
    }

    #[test]
    fn argb32_uploader_keeps_caller_error_context() {
        let error = unsafe {
            upload_argb32_image(
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                &[],
                1,
                1,
                "external ARGB32 upload",
            )
        }
        .unwrap_err();

        assert_eq!(error, "external ARGB32 upload: ARGB32 visual is NULL");
    }

    #[test]
    fn per_node_overrides_never_collide_on_asset_alone() {
        let fg = Color::rgb(255, 255, 255);
        let a = image_cache_key(0, 0, 10, 10, 1, ImageTreatment::Original, fg);
        let b = image_cache_key(1, 0, 10, 10, 1, ImageTreatment::Original, fg);
        assert_ne!(a, b, "sibling nodes sharing asset must key separately");
        let a2 = image_cache_key(0, 0, 10, 10, 2, ImageTreatment::Original, fg);
        assert_ne!(a, a2, "revision advance must change the key");
        let a_resized = image_cache_key(0, 0, 12, 10, 2, ImageTreatment::Original, fg);
        assert_ne!(a2, a_resized, "size is part of the key");
        let a_tinted = image_cache_key(
            0,
            0,
            12,
            10,
            2,
            ImageTreatment::SymbolicForeground,
            Color::rgb(10, 20, 30),
        );
        assert_ne!(a_resized, a_tinted, "treatment+palette are part of the key");
    }

    #[test]
    fn pixmap_accounting_includes_depth() {
        use super::super::native::xresource::pixmap_byte_estimate;
        assert_eq!(pixmap_byte_estimate(2, 3, 32), 24);
        assert_eq!(pixmap_byte_estimate(2, 3, 1), 6);
    }
}
