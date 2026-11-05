#![allow(non_camel_case_types)]

use std::collections::HashMap;
use std::env;
use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_short, c_uchar, c_ulong, c_ushort, c_void};
use std::path::Path;
use std::ptr;

use flamewm_render_core::Color;

use crate::ffi::dynamic_library::DynamicLibrary;
use crate::text_mask_cache::{TextMaskCache, TextMaskKey, TextMaskRef};
use crate::xlib::{
    Colormap, Display, Drawable, Visual, XCreateGC, XCreatePixmap, XDestroyImage, XFillRectangle,
    XFreeGC, XGetImage, XImageHead, XRectangle, XRootWindow, XSetForeground, ZPIXMAP,
};

#[repr(C)]
struct XftDraw {
    _private: [u8; 0],
}

#[repr(C)]
struct XftFont {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct XRenderColor {
    red: u16,
    green: u16,
    blue: u16,
    alpha: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct XftColor {
    pixel: c_ulong,
    color: XRenderColor,
}

#[repr(C)]
struct FcConfig {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct XGlyphInfo {
    width: c_ushort,
    height: c_ushort,
    x: c_short,
    y: c_short,
    x_off: c_short,
    y_off: c_short,
}

// XRender.h's XGlyphInfo is two unsigned shorts followed by four shorts.
const _: () = {
    assert!(std::mem::size_of::<XGlyphInfo>() == 12);
    assert!(std::mem::align_of::<XGlyphInfo>() == 2);
    let info = XGlyphInfo {
        width: 0,
        height: 0,
        x: 0,
        y: 0,
        x_off: 0,
        y_off: 0,
    };
    let _: c_ushort = info.width;
    let _: c_ushort = info.height;
    let _: c_short = info.x;
    let _: c_short = info.y;
    let _: c_short = info.x_off;
    let _: c_short = info.y_off;
};

type XftDrawCreateFn =
    unsafe extern "C" fn(*mut Display, Drawable, *mut Visual, Colormap) -> *mut XftDraw;
type XftDrawDestroyFn = unsafe extern "C" fn(*mut XftDraw);
type XftDrawCreateAlphaFn = unsafe extern "C" fn(*mut Display, Drawable, c_int) -> *mut XftDraw;
type XftDrawChangeFn = unsafe extern "C" fn(*mut XftDraw, Drawable);
/// `XftDrawSetClip(draw, region)`; a null region removes the clip.
type XftDrawSetClipFn = unsafe extern "C" fn(*mut XftDraw, *const c_void);
type XftDrawSetClipRectanglesFn =
    unsafe extern "C" fn(*mut XftDraw, c_int, c_int, *const XRectangle, c_int) -> c_int;
type XftDrawStringUtf8Fn = unsafe extern "C" fn(
    *mut XftDraw,
    *const XftColor,
    *mut XftFont,
    c_int,
    c_int,
    *const c_uchar,
    c_int,
);
type XftTextExtentsUtf8Fn =
    unsafe extern "C" fn(*mut Display, *mut XftFont, *const c_uchar, c_int, *mut XGlyphInfo);
type XftFontOpenNameFn = unsafe extern "C" fn(*mut Display, c_int, *const c_char) -> *mut XftFont;
type XftFontCloseFn = unsafe extern "C" fn(*mut Display, *mut XftFont);
type XftColorAllocValueFn = unsafe extern "C" fn(
    *mut Display,
    *mut Visual,
    Colormap,
    *const XRenderColor,
    *mut XftColor,
) -> c_int;
type XftColorFreeFn = unsafe extern "C" fn(*mut Display, *mut Visual, Colormap, *mut XftColor);

type FcConfigGetCurrentFn = unsafe extern "C" fn() -> *mut FcConfig;
type FcConfigAppFontAddFileFn = unsafe extern "C" fn(*mut FcConfig, *const c_uchar) -> c_int;
type FcConfigBuildFontsFn = unsafe extern "C" fn(*mut FcConfig) -> c_int;

#[derive(Clone, Copy)]
struct XftApi {
    draw_create: XftDrawCreateFn,
    draw_destroy: XftDrawDestroyFn,
    draw_create_alpha: Option<XftDrawCreateAlphaFn>,
    draw_change: XftDrawChangeFn,
    draw_string_utf8: XftDrawStringUtf8Fn,
    draw_set_clip: Option<XftDrawSetClipFn>,
    draw_set_clip_rectangles: Option<XftDrawSetClipRectanglesFn>,
    text_extents_utf8: Option<XftTextExtentsUtf8Fn>,
    font_open_name: XftFontOpenNameFn,
    font_close: XftFontCloseFn,
    color_alloc_value: XftColorAllocValueFn,
    color_free: XftColorFreeFn,
}

#[derive(Clone, Copy)]
struct FontconfigApi {
    config_get_current: FcConfigGetCurrentFn,
    config_app_font_add_file: FcConfigAppFontAddFileFn,
    config_build_fonts: FcConfigBuildFontsFn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct FontKey {
    pixels: u16,
    weight: u16,
}

/// Upper bound on rows per text-mask pixmap batch (bounds the batch to a few MB).
const MAX_MASK_BATCH_HEIGHT: u32 = 4096;

/// One text run to rasterize as an A8 coverage mask.
#[derive(Clone, Copy, Debug)]
pub struct AlphaMaskSpec<'a> {
    pub size: f32,
    pub weight: u16,
    pub text: &'a str,
    pub pad: u32,
}

/// A rendered A8 coverage mask, row-major, `width * height` bytes.
#[derive(Debug)]
pub(crate) struct AlphaMask {
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

pub struct XftBackend {
    display: *mut Display,
    screen: c_int,
    visual: *mut Visual,
    colormap: Colormap,
    draw: *mut XftDraw,
    api: XftApi,
    _xft_library: Option<DynamicLibrary>,
    _fontconfig_library: Option<DynamicLibrary>,
    fonts: HashMap<FontKey, *mut XftFont>,
    colors: HashMap<Color, XftColor>,
    preferred_family: String,
    text_masks: TextMaskCache,
    /// Synchronous `XGetImage` readbacks issued for text masks (one per batch).
    pub(crate) readbacks: u64,
}

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
        let xft_library = DynamicLibrary::open(&["libXft.so.2", "libXft.so"])?;
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
        let spec = AlphaMaskSpec { size, weight, text, pad };
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
            let key = TextMaskRef::new(&self.preferred_family, spec.text, spec.size, spec.weight, spec.pad);
            if self.text_masks.contains(&key) {
                continue;
            }
            let owned = TextMaskKey::new(&self.preferred_family, spec.text, spec.size, spec.weight, spec.pad);
            if seen.insert(owned) {
                missing.push(*spec);
            }
        }
        if !missing.is_empty() {
            let _ = unsafe { self.render_alpha_masks(&missing) };
        }
    }

    /// Renders each spec into its own A8 mask and inserts successes into the cache. Specs are
    /// laid out as horizontal bands of one pixmap, each clipped to its own band, so every mask
    /// matches a standalone render pixel for pixel. One `XGetImage` covers each batch.
    unsafe fn render_alpha_masks(&mut self, specs: &[AlphaMaskSpec<'_>]) -> Vec<Result<AlphaMask, String>> {
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
                            TextMaskKey::new(&self.preferred_family, spec.text, spec.size, spec.weight, spec.pad),
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
    unsafe fn render_mask_batch(
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
                    XGetImage(self.display, pixmap, 0, 0, total_width, total_height, !0, ZPIXMAP)
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
                        std::slice::from_raw_parts(head.data as *const u8, row_bytes * total_height as usize)
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
                            AlphaMask { width, height, bytes }
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

    unsafe fn font_for(&mut self, size: f32, weight: u16) -> Result<*mut XftFont, String> {
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

    unsafe fn color_for(&mut self, color: Color) -> Result<XftColor, String> {
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

    unsafe fn release_resources(&mut self) {
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

unsafe fn register_application_fonts(api: FontconfigApi) -> Result<(), String> {
    let Some(value) = env::var_os("FLAMEWM_RENDER_FONT_FILES") else {
        return Ok(());
    };
    // SAFETY: the config pointer is returned by fontconfig itself; NULL is rejected above.
    let config = unsafe { (api.config_get_current)() };
    if config.is_null() {
        return Err("FcConfigGetCurrent returned NULL".to_string());
    }
    let mut added = 0usize;
    for path in env::split_paths(&value) {
        if !path.is_file() {
            return Err(format!(
                "FLAMEWM_RENDER_FONT_FILES entry does not exist: {}",
                path.display()
            ));
        }
        let bytes = path_bytes(&path)?;
        let c_path = CString::new(bytes)
            .map_err(|_| format!("font path contains NUL: {}", path.display()))?;
        // SAFETY: `config` is non-NULL per the check above; `c_path` stays alive
        // for the call. A zero return is handled as an error below.
        if unsafe { (api.config_app_font_add_file)(config, c_path.as_ptr() as *const c_uchar) } == 0
        {
            return Err(format!(
                "FcConfigAppFontAddFile rejected {}",
                path.display()
            ));
        }
        added += 1;
    }
    // SAFETY: `config` is the non-NULL fontconfig handle checked above.
    if added > 0 && unsafe { (api.config_build_fonts)(config) } == 0 {
        return Err("FcConfigBuildFonts failed after adding application fonts".to_string());
    }
    Ok(())
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Result<Vec<u8>, String> {
    use std::os::unix::ffi::OsStrExt;
    Ok(path.as_os_str().as_bytes().to_vec())
}

#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Result<Vec<u8>, String> {
    path.to_str()
        .map(|value| value.as_bytes().to_vec())
        .ok_or_else(|| format!("font path is not UTF-8: {}", path.display()))
}

fn normalize_weight(weight: u16) -> u16 {
    match weight {
        0..=450 => 400,
        451..=550 => 500,
        551..=650 => 600,
        _ => 700,
    }
}

/// Runs `f` with layout widths measured by `xft` (`XftTextExtentsUtf8` at the requested weight).
/// With no Xft backend, layout falls back to the render-core model. Each call measures only the
/// text layout asks for; nothing is cached across frames, so the cost scales with layout work.
pub(crate) fn scoped_text_measure<R>(xft: Option<&mut XftBackend>, f: impl FnOnce() -> R) -> R {
    match xft {
        None => f(),
        Some(backend) => {
            let cell = std::cell::RefCell::new(backend);
            let measure = |text: &str, size: f32, weight: u16| {
                // SAFETY: the backend's display, fonts and draw surface live as long as the
                // surface, and `f` runs strictly inside this scope.
                unsafe { cell.borrow_mut().measure_text(size, weight, text).0 }
            };
            flamewm_render_core::text_layout::with_measure(&measure, f)
        }
    }
}

/// Pure estimate backing [`XftBackend::measure_text`]: 0.58em per char,
/// 1.30em line height. Shared so the fallback path is unit-testable.
pub(crate) fn xft_measure_estimate(text: &str, size: f32) -> (f32, f32) {
    let height = (size.max(1.0) * 1.30).round();
    ((text.chars().count() as f32 * size * 0.58).round(), height)
}

/// Straight-alpha `Color` -> 16-bit XRender color triple used by
/// `XftColorAllocValue`. Pure so the alpha expansion is unit-testable.
pub(crate) fn xrender_color_for(color: Color) -> (u16, u16, u16, u16) {
    (
        u16::from(color.r) * 257,
        u16::from(color.g) * 257,
        u16::from(color.b) * 257,
        u16::from(color.a) * 257,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Per-call cost of `XftBackend::measure_text` (XftTextExtentsUtf8) on a 40-char line.
    /// Run: `FLAMEWM_TEST_DISPLAY=:0 cargo test -p flamewm-render-x11 xft_text_extents_per_call_bench -- --ignored --nocapture`
    #[test]
    #[ignore = "bench: needs FLAMEWM_TEST_DISPLAY"]
    fn xft_text_extents_per_call_bench() {
        use crate::xlib::{XDefaultColormap, XDefaultScreen, XDefaultVisual, XOpenDisplay, XRootWindow};
        let Some(name) = std::env::var_os("FLAMEWM_TEST_DISPLAY") else {
            return;
        };
        let name = std::ffi::CString::new(name.to_string_lossy().as_bytes()).unwrap();
        unsafe {
            let display = XOpenDisplay(name.as_ptr());
            assert!(!display.is_null(), "XOpenDisplay failed");
            let screen = XDefaultScreen(display);
            let root = XRootWindow(display, screen);
            let visual = XDefaultVisual(display, screen);
            let colormap = XDefaultColormap(display, screen);
            let mut xft = XftBackend::new(display, screen, root, visual, colormap).expect("xft");
            let line = "Wi AVAST, Tofu: ilm 日本 Wa ff W./ VAT To ilm AVocado";
            const CALLS: u32 = 20_000;
            let started = std::time::Instant::now();
            let mut sink = 0.0f32;
            for _ in 0..CALLS {
                sink += xft.measure_text(13.0, 400, std::hint::black_box(line)).0;
            }
            let per_call = started.elapsed().as_nanos() as f64 / f64::from(CALLS);
            std::hint::black_box(sink);
            eprintln!("XftTextExtentsUtf8 bench: {per_call:.0} ns/call over {CALLS} calls");
            assert!(per_call.is_finite());
        }
    }

    /// Caret-vs-paint probe on a live display (`FLAMEWM_TEST_DISPLAY`). For each prefix length the
    /// caret is `measure_text(prefix)`; the painted edge is the rightmost inked column of the real
    /// Xft text mask of the same prefix. A growing `advance - ink_end` means the caret drifts.
    #[test]
    #[ignore = "live-display probe: FLAMEWM_TEST_DISPLAY=:99 cargo test -p flamewm-render-x11 --lib caret_x_tracks -- --ignored"]
    fn caret_x_tracks_painted_glyph_edge_on_live_display() {
        use crate::xlib::{XDefaultColormap, XDefaultScreen, XDefaultVisual, XOpenDisplay, XRootWindow};
        let Some(name) = std::env::var_os("FLAMEWM_TEST_DISPLAY") else {
            return;
        };
        let name = std::ffi::CString::new(name.to_string_lossy().as_bytes()).unwrap();
        unsafe {
            let display = XOpenDisplay(name.as_ptr());
            assert!(!display.is_null(), "XOpenDisplay failed");
            let screen = XDefaultScreen(display);
            let root = XRootWindow(display, screen);
            let visual = XDefaultVisual(display, screen);
            let colormap = XDefaultColormap(display, screen);
            let mut xft = XftBackend::new(display, screen, root, visual, colormap).expect("xft");
            let text = &"I".repeat(60);
            let pad = 8u32;
            let mut worst_drift = f32::MIN;
            let mut best_drift = f32::MAX;
            for n in [10usize, 20, 30, 40, 50, 60] {
                let prefix: String = text.chars().take(n).collect();
                let caret = xft.measure_text(13.0, 400, &prefix).0;
                let mut mask = Vec::new();
                let (width, height) = xft
                    .text_alpha_mask(13.0, 400, &prefix, pad, &mut mask)
                    .expect("mask");
                let mut ink_end: Option<u32> = None;
                for row in 0..height as usize {
                    let line = &mask[row * width as usize..(row + 1) * width as usize];
                    if let Some(col) = line.iter().rposition(|&c| c > 0) {
                        ink_end = Some(ink_end.map_or(col as u32, |e| e.max(col as u32)));
                    }
                }
                let ink_end = ink_end.expect("prefix must ink") as f32 + 1.0 - pad as f32;
                let drift = caret - ink_end;
                eprintln!("caret probe n={n:2} caret={caret:7.2} ink_end={ink_end:7.2} gap={drift:6.2}");
                worst_drift = worst_drift.max(drift);
                best_drift = best_drift.min(drift);
            }
            // The last glyph's right side bearing is the only legitimate gap: it is at most a few px
            // and must not grow with prefix length.
            assert!(
                worst_drift - best_drift < 4.0,
                "caret-to-ink gap varies by {} px across prefix lengths",
                worst_drift - best_drift
            );
        }
    }

    #[test]
    fn xglyph_info_matches_xrender_abi() {
        assert_eq!(std::mem::size_of::<XGlyphInfo>(), 12);
        assert_eq!(std::mem::align_of::<XGlyphInfo>(), 2);
        assert_eq!(std::mem::offset_of!(XGlyphInfo, width), 0);
        assert_eq!(std::mem::offset_of!(XGlyphInfo, height), 2);
        assert_eq!(std::mem::offset_of!(XGlyphInfo, x), 4);
        assert_eq!(std::mem::offset_of!(XGlyphInfo, y), 6);
        assert_eq!(std::mem::offset_of!(XGlyphInfo, x_off), 8);
        assert_eq!(std::mem::offset_of!(XGlyphInfo, y_off), 10);
    }

    #[test]
    fn css_weights_map_to_available_plex_faces() {
        assert_eq!(normalize_weight(400), 400);
        assert_eq!(normalize_weight(500), 500);
        assert_eq!(normalize_weight(600), 600);
        assert_eq!(normalize_weight(700), 700);
    }

    #[test]
    fn xft_measure_estimate_matches_layout() {
        assert_eq!(xft_measure_estimate("hello", 13.0), (38.0, 17.0));
        assert_eq!(xft_measure_estimate("", 13.0), (0.0, 17.0));
    }

    #[test]
    fn xft_color_expands_straight_alpha() {
        assert_eq!(
            xrender_color_for(Color {
                r: 255,
                g: 0,
                b: 0,
                a: 128
            }),
            (65535, 0, 0, 32896)
        );
    }

    /// Opens a private test display with Xft, or returns None (test skipped) without one.
    #[cfg(test)]
    fn open_test_xft() -> Option<(*mut crate::xlib::Display, XftBackend)> {
        use crate::xlib::{
            XDefaultColormap, XDefaultScreen, XDefaultVisual, XOpenDisplay, XRootWindow,
        };
        let name = std::env::var_os("FLAMEWM_TEST_DISPLAY")?;
        let name = std::ffi::CString::new(name.to_string_lossy().as_bytes()).unwrap();
        unsafe {
            let display = XOpenDisplay(name.as_ptr());
            assert!(!display.is_null(), "XOpenDisplay failed");
            let screen = XDefaultScreen(display);
            let root = XRootWindow(display, screen);
            let visual = XDefaultVisual(display, screen);
            let colormap = XDefaultColormap(display, screen);
            let xft = XftBackend::new(display, screen, root, visual, colormap).expect("xft");
            Some((display, xft))
        }
    }

    /// Batched shadow masks must be byte-identical to the original single-pixmap render, for
    /// every run, including when the batch mixes sizes, weights and pads.
    #[cfg(test)]
    #[test]
    fn batched_masks_match_reference_render_pixel_for_pixel() {
        let Some((display, mut xft)) = open_test_xft() else {
            return;
        };
        let runs: [(f32, u16, u32, &str); 6] = [
            (14.0, 400, 4, "Shadow"),
            (11.0, 400, 10, "0 processes \u{b7} 0 threads"),
            (13.0, 700, 6, "/usr/lib/jvm/zulu21/bin/java -Xmx1096m"),
            (9.0, 400, 3, "CPU 12.5%"),
            (14.0, 400, 4, "Shadow"),
            (16.0, 400, 8, "Wi\u{66}i \u{2014} Aa"),
        ];
        let mut reference = Vec::new();
        for &(size, weight, pad, text) in &runs {
            xft.text_masks.clear();
            let mut bytes = Vec::new();
            let dims = unsafe { xft.reference_alpha_mask(size, weight, text, pad, &mut bytes) }
                .expect("reference mask");
            reference.push((dims, bytes));
        }
        xft.text_masks.clear();
        let specs: Vec<AlphaMaskSpec<'_>> = runs
            .iter()
            .map(|&(size, weight, pad, text)| AlphaMaskSpec { size, weight, text, pad })
            .collect();
        unsafe { xft.prefetch_text_masks(&specs) };
        for (index, &(size, weight, pad, text)) in runs.iter().enumerate() {
            let mut batched = Vec::new();
            let dims = unsafe { xft.text_alpha_mask(size, weight, text, pad, &mut batched) }
                .expect("batched mask");
            assert_eq!(dims, reference[index].0, "dims differ for {text:?}");
            assert!(batched == reference[index].1, "pixels differ for {text:?}");
        }
        let libraries = unsafe { xft.into_deferred_libraries() };
        unsafe { crate::xlib::XCloseDisplay(display) };
        drop(libraries);
    }

    /// A paint's worth of uncached shadow runs costs one readback; the follow-up per-run calls
    /// and a repeated prefetch cost none.
    #[cfg(test)]
    #[test]
    fn prefetch_costs_one_readback_per_batch() {
        let Some((display, mut xft)) = open_test_xft() else {
            return;
        };
        let texts = ["one", "two two", "three", "four 4%", "five", "six 6.0%"];
        let specs: Vec<AlphaMaskSpec<'_>> = texts
            .iter()
            .map(|text| AlphaMaskSpec { size: 12.0, weight: 400, text, pad: 6 })
            .collect();
        let before = xft.readbacks;
        unsafe { xft.prefetch_text_masks(&specs) };
        assert_eq!(xft.readbacks - before, 1, "six short runs must share one readback");
        let mut out = Vec::new();
        for text in texts {
            unsafe { xft.text_alpha_mask(12.0, 400, text, 6, &mut out) }.expect("cached");
        }
        assert_eq!(xft.readbacks - before, 1, "cached runs must not read back");
        unsafe { xft.prefetch_text_masks(&specs) };
        assert_eq!(xft.readbacks - before, 1, "repeated prefetch must hit the cache");
        let libraries = unsafe { xft.into_deferred_libraries() };
        unsafe { crate::xlib::XCloseDisplay(display) };
        drop(libraries);
    }

    /// The text-mask cache must return the same coverage as a fresh A8 render, and
    /// a second identical request must be a hit (no new entry). Needs
    /// `FLAMEWM_TEST_DISPLAY` with Xft fonts; skipped otherwise.
    #[test]
    fn text_mask_cache_hit_matches_fresh_render() {
        use crate::xlib::{
            XCloseDisplay, XDefaultColormap, XDefaultScreen, XDefaultVisual, XOpenDisplay,
            XRootWindow,
        };
        let Some(name) = std::env::var_os("FLAMEWM_TEST_DISPLAY") else {
            return;
        };
        let name = std::ffi::CString::new(name.to_string_lossy().as_bytes()).unwrap();
        unsafe {
            let display = XOpenDisplay(name.as_ptr());
            assert!(!display.is_null(), "XOpenDisplay failed");
            let screen = XDefaultScreen(display);
            let root = XRootWindow(display, screen);
            let visual = XDefaultVisual(display, screen);
            let colormap = XDefaultColormap(display, screen);
            let mut xft = XftBackend::new(display, screen, root, visual, colormap).expect("xft");
            let (mut fresh, mut hit) = (Vec::new(), Vec::new());
            let dims = xft
                .text_alpha_mask(14.0, 400, "Shadow", 4, &mut fresh)
                .expect("fresh mask");
            assert_eq!(xft.text_masks.len(), 1);
            let hit_dims = xft
                .text_alpha_mask(14.0, 400, "Shadow", 4, &mut hit)
                .expect("cached mask");
            assert_eq!((dims, fresh.as_slice()), (hit_dims, hit.as_slice()));
            assert_eq!(xft.text_masks.len(), 1, "identical request must hit");
            xft.text_alpha_mask(15.0, 400, "Shadow", 4, &mut hit)
                .expect("other size");
            assert_eq!(xft.text_masks.len(), 2, "size is part of the key");
            // Libraries stay loaded through XCloseDisplay, as in production.
            let libraries = xft.into_deferred_libraries();
            XCloseDisplay(display);
            drop(libraries);
        }
    }
}
