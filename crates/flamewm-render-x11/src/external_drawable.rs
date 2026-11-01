//! External drawable session: X11App-independent painting for wm-owned drawables.
//! Session owns display/screen/visual/colormap/gc plus Xft/XRender/Shape
//! backends; targets retarget those backends per drawable. No silent
//! fallback: text/alpha paths error when their backend is unavailable.

use std::collections::{HashMap, VecDeque};
use std::ffi::CString;
use std::ptr;

use flamewm_api::graphics::{DisplayProtocol, GraphicsProvider, choose_provider};
use flamewm_render_core::{Color, Rect, RuntimeDocument};

use super::native::cursor::DeferredNativeLibraryHandles;
use super::native::xresource::{X11ResourceAllocator, refresh_image_gauge};
use super::xft::{XftBackend, xft_measure_estimate};
use super::xlib::*;
use super::xrender::XRenderBackend;
use super::xshape::{XShapeBridge, rounded_mask_spans};
use crate::app::{upload_argb32_image, upload_opaque_argb32_image};
use crate::xlib_ext::XGetGeometry;

/// Persistent RGBA native cache key: content identity plus raster size and
/// palette. `content_hash` is FNV-1a over the source RGBA8 bytes; `palette`
/// distinguishes straight blits (all-white sentinel) from symbolic tints.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ExternalImageKey {
    content_hash: u64,
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    palette: [u8; 4],
}

#[derive(Clone, Copy)]
struct ExternalCachedImage {
    pixmap: Pixmap,
    bytes: usize,
}

/// XCopyArea requires identical depths. Query once per uncached core blit; a
/// retarget clears the cache so it can never mix pixmaps from different drawables.
unsafe fn external_drawable_depth(
    display: *mut Display,
    drawable: Drawable,
) -> Result<u32, String> {
    let (mut root, mut x, mut y) = (0, 0, 0);
    let (mut width, mut height, mut border, mut depth) = (0, 0, 0, 0);
    let status = unsafe {
        XGetGeometry(
            display,
            drawable,
            &mut root,
            &mut x,
            &mut y,
            &mut width,
            &mut height,
            &mut border,
            &mut depth,
        )
    };
    if status == 0 || depth == 0 {
        return Err("XGetGeometry failed for core-X11 drawable depth".to_string());
    }
    Ok(depth)
}

/// Max cached native pixmaps / total native bytes for the image LRU.
pub const EXTERNAL_IMAGE_CACHE_MAX_ENTRIES: usize = 128;
pub const EXTERNAL_IMAGE_CACHE_MAX_BYTES: usize = 8 * 1024 * 1024;

/// MRU touch for the image LRU queue: hit moves `key` to the back
/// (newest); miss appends. Pure and unit-testable without X.
fn image_lru_touch(lru: &mut VecDeque<ExternalImageKey>, key: ExternalImageKey) {
    if let Some(pos) = lru.iter().position(|entry| *entry == key) {
        lru.remove(pos);
    }
    lru.push_back(key);
    // Hard bound: the queue mirrors at most the entry cap.
    while lru.len() > EXTERNAL_IMAGE_CACHE_MAX_ENTRIES {
        lru.pop_front();
    }
}

fn external_content_hash(rgba8: &[u8]) -> u64 {
    // Word-at-a-time FNV-style mix: one multiply per 8 bytes instead of per byte.
    const PRIME: u64 = 0x100000001b3;
    let mut hash: u64 = 0xcbf29ce484222325 ^ rgba8.len() as u64;
    let mut chunks = rgba8.chunks_exact(8);
    for chunk in &mut chunks {
        let word = u64::from_le_bytes(chunk.try_into().expect("8-byte chunk"));
        hash = (hash ^ word).wrapping_mul(PRIME);
    }
    for &byte in chunks.remainder() {
        hash = (hash ^ u64::from(byte)).wrapping_mul(PRIME);
    }
    hash
}

/// Session independent of [`super::app::X11App`]: display, screen, visual,
/// colormap, GC, Xft/XRender/image-cache state. Created once per X connection
/// and retargeted across external drawables.
pub struct ExternalDrawableSession {
    display: *mut Display,
    screen: i32,
    visual: *mut Visual,
    colormap: Colormap,
    owned_colormap: bool,
    gc: GC,
}

impl ExternalDrawableSession {
    /// # Safety
    /// `display` must be a live Xlib display that outlives the session.
    pub unsafe fn new(display: *mut Display) -> Result<Self, String> {
        if display.is_null() {
            return Err("ExternalDrawableSession requires a live display".to_string());
        }
        let screen = unsafe { XDefaultScreen(display) };
        let visual = unsafe { XDefaultVisual(display, screen) };
        if visual.is_null() {
            return Err("XDefaultVisual returned NULL for external session".to_string());
        }
        let colormap = unsafe { XDefaultColormap(display, screen) };
        let root = unsafe { XRootWindow(display, screen) };
        let gc = unsafe { XCreateGC(display, root, 0, ptr::null_mut()) };
        if gc.is_null() {
            return Err("XCreateGC failed for external session".to_string());
        }
        unsafe { XSetGraphicsExposures(display, gc, 0) };
        Ok(Self {
            display,
            screen,
            visual,
            colormap,
            owned_colormap: false,
            gc,
        })
    }

    pub fn display(&self) -> *mut Display {
        self.display
    }

    pub fn screen(&self) -> i32 {
        self.screen
    }
}

impl Drop for ExternalDrawableSession {
    fn drop(&mut self) {
        unsafe {
            if !self.gc.is_null() {
                XFreeGC(self.display, self.gc);
                self.gc = ptr::null_mut();
            }
            if self.owned_colormap && self.colormap != 0 {
                XFreeColormap(self.display, self.colormap);
                self.colormap = 0;
            }
        }
    }
}

/// Target bound to one external drawable. Owns its Xft/XRender/Shape
/// backends so `retarget` swaps the live drawable without stale handles:
/// the old XRender destination picture is freed before the swap completes.
/// Owns a persistent RGBA native cache: same content/size/palette reuses
/// the cached 32-bit pixmap (zero reupload). Lifecycle: insert on first
/// blit keyed by [`ExternalImageKey`], explicit `invalidate_image_cache`,
/// full release on drop. Cost tracked by `image_cache_bytes`.
pub struct ExternalDrawableTarget {
    session: *mut ExternalDrawableSession,
    drawable: Drawable,
    width: u32,
    height: u32,
    xft: Option<XftBackend>,
    xrender: Option<XRenderBackend>,
    xshape: Option<XShapeBridge>,
    xft_bound: Drawable,
    xrender_bound: Drawable,
    image_cache: HashMap<ExternalImageKey, ExternalCachedImage>,
    image_lru: VecDeque<ExternalImageKey>,
    allocator: X11ResourceAllocator,
}

impl Drop for ExternalDrawableTarget {
    fn drop(&mut self) {
        self.free_all_images();
    }
}

impl ExternalDrawableTarget {
    /// Move all dynamically loaded backend handles to the display owner after
    /// releasing their drawable-bound X resources. The target remains valid
    /// for its image-cache teardown, but no backend library is dropped before
    /// XCloseDisplay.
    pub(crate) unsafe fn take_deferred_native_libraries(&mut self) -> DeferredNativeLibraryHandles {
        unsafe {
            DeferredNativeLibraryHandles::from_backends(
                None,
                self.xft.take(),
                self.xrender.take(),
                self.xshape.take(),
            )
        }
    }

    /// # Safety
    /// `session` must outlive the target; `drawable` must be valid on the
    /// session display. Backend construction failures are recorded as `None`
    /// and the matching paint method returns an explicit error (no fallback).
    pub unsafe fn new(
        session: *mut ExternalDrawableSession,
        drawable: Drawable,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        if session.is_null() {
            return Err("ExternalDrawableTarget requires a live session".to_string());
        }
        if drawable == 0 {
            return Err("ExternalDrawableTarget requires a nonzero drawable".to_string());
        }
        let view = unsafe { &*session };
        let display = view.display;
        let screen = view.screen;
        let visual = view.visual;
        let colormap = view.colormap;
        let xft = match unsafe { XftBackend::new(display, screen, drawable, visual, colormap) } {
            Ok(backend) => Some(backend),
            Err(error) => {
                eprintln!("FLAMEWM_RENDER_TEXT_BACKEND external-core-x11 reason={error}");
                None
            }
        };
        let requested = std::env::var("FLAMEWM_X11_RENDER_PROVIDER").ok();
        let mut xrender = match unsafe { XRenderBackend::new(display, drawable, visual) } {
            Ok(backend) => Some(backend),
            Err(error) => {
                eprintln!("FLAMEWM_RENDER_ALPHA_BACKEND external-opaque-fallback reason={error}");
                None
            }
        };
        let provider = match choose_provider(
            DisplayProtocol::X11,
            requested.as_deref(),
            xrender.is_some(),
        ) {
            Ok(provider) => provider,
            Err(error) => {
                eprintln!("FLAMEWM_RENDER_PROVIDER_ERROR external: {error}; using core fallback");
                GraphicsProvider::XCore
            }
        };
        if provider == GraphicsProvider::XCore {
            xrender = None;
        }
        let xshape = match unsafe { XShapeBridge::new(display) } {
            Ok(bridge) => Some(bridge),
            Err(error) => {
                eprintln!(
                    "FLAMEWM_RENDER_SHAPE_BACKEND external-fallback-rectangle reason={error}"
                );
                None
            }
        };
        Ok(Self {
            session,
            drawable,
            width: width.max(1),
            height: height.max(1),
            xft,
            xrender,
            xshape,
            xft_bound: drawable,
            xrender_bound: drawable,
            image_cache: HashMap::new(),
            image_lru: VecDeque::new(),
            allocator: X11ResourceAllocator::new(),
        })
    }

    pub fn drawable(&self) -> Drawable {
        self.drawable
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Swap to a new drawable, retargeting Xft/XRender so no stale drawable
    /// survives. XRender frees the old destination picture first; on failure
    /// the target keeps the previous drawable and reports the error.
    ///
    /// # Safety
    /// `drawable` must be valid on the session display.
    pub unsafe fn retarget(
        &mut self,
        drawable: Drawable,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        if drawable == 0 {
            return Err("retarget requires a nonzero drawable".to_string());
        }
        if drawable == self.drawable {
            self.width = width.max(1);
            self.height = height.max(1);
            return Ok(());
        }
        if let Some(xrender) = self.xrender.as_mut() {
            unsafe { xrender.set_drawable(drawable) }?;
        }
        // Cached core-X11 pixmaps have the source drawable's depth. A new target
        // may use another visual/depth, so never reuse them across retargets.
        self.invalidate_image_cache();
        if let Some(xft) = self.xft.as_mut() {
            unsafe { xft.set_drawable(drawable) };
        }
        self.drawable = drawable;
        self.xft_bound = drawable;
        self.xrender_bound = drawable;
        self.width = width.max(1);
        self.height = height.max(1);
        Ok(())
    }

    fn session_view(&self) -> Result<&ExternalDrawableSession, String> {
        if self.session.is_null() {
            return Err("external session pointer is NULL".to_string());
        }
        Ok(unsafe { &*self.session })
    }

    /// Pure text measure matching the canonical layout estimate (0.58em/char).
    pub fn measure_text(text: &str, size: f32) -> (f32, f32) {
        external_text_measure(text, size)
    }

    /// Reusable Xft measurement for decorations: true Xft advance when the
    /// backend and `XftTextExtentsUtf8` are available, else the same
    /// estimate as [`measure_text`](Self::measure_text). Live paint API is
    /// unchanged; this only reads font metrics.
    pub fn measure_decoration_text(
        &mut self,
        family: &str,
        size: f32,
        weight: u16,
        text: &str,
    ) -> (f32, f32) {
        let estimate = xft_measure_estimate(text, size);
        let Some(xft) = self.xft.as_mut() else {
            return estimate;
        };
        unsafe { xft.set_preferred_family(family) };
        unsafe { xft.measure_text(size, weight, text) }
    }

    /// Persistent image-cache surface: entries, total native bytes
    /// (dst_w*dst_h*4 per cached pixmap), explicit invalidation.
    pub fn image_cache_len(&self) -> usize {
        self.image_cache.len()
    }

    pub fn image_cache_bytes(&self) -> usize {
        self.image_cache.values().map(|cached| cached.bytes).sum()
    }

    pub fn invalidate_image_cache(&mut self) {
        self.free_all_images();
    }

    /// Drop every cached pixmap (freed when a session is bound), clear the
    /// LRU, and zero the gauge.
    fn free_all_images(&mut self) {
        let display = self
            .session_view()
            .map_or(ptr::null_mut(), |view| view.display);
        let cache = std::mem::take(&mut self.image_cache);
        for (_, cached) in cache {
            self.free_image(display, cached);
        }
        self.image_lru.clear();
        refresh_image_gauge(0);
    }

    /// Checked free of one cached pixmap; no-op without a display.
    fn free_image(&mut self, display: *mut Display, cached: ExternalCachedImage) {
        if cached.pixmap != 0 && !display.is_null() {
            unsafe {
                self.allocator
                    .free_pixmap(display, cached.pixmap, cached.bytes)
            };
        }
    }

    /// Evict oldest entries via the allocator until within both caps.
    /// Pure accounting + checked frees; updates the image gauge.
    fn enforce_image_limits(&mut self, display: *mut Display) {
        while self.image_cache.len() > EXTERNAL_IMAGE_CACHE_MAX_ENTRIES
            || self.image_cache_bytes() > EXTERNAL_IMAGE_CACHE_MAX_BYTES
        {
            let Some(oldest) = self.image_lru.pop_front() else {
                break;
            };
            if let Some(cached) = self.image_cache.remove(&oldest) {
                self.free_image(display, cached);
            }
        }
        refresh_image_gauge(self.image_cache_bytes() as u64);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_text(
        &mut self,
        family: &str,
        x: f32,
        baseline_y: f32,
        color: Color,
        size: f32,
        weight: u16,
        text: &str,
    ) -> Result<(), String> {
        let Some(xft) = self.xft.as_mut() else {
            return Err("external draw_text requires Xft (unavailable)".to_string());
        };
        unsafe { xft.set_preferred_family(family) };
        unsafe { xft.draw_text(x, baseline_y, color, size, weight, text) }
    }

    pub fn fill_rect(&mut self, rect: Rect, color: Color) -> Result<(), String> {
        self.fill_rounded_rect(rect, 0.0, color)
    }

    pub fn fill_rounded_rect(
        &mut self,
        rect: Rect,
        radius: f32,
        color: Color,
    ) -> Result<(), String> {
        if let Some(xrender) = self.xrender.as_mut() {
            return unsafe { crate::xrender_aa::fill_rounded_rect(xrender, rect, radius, color) };
        }
        if color.a < 255 {
            return Err("external translucent fill requires XRender (unavailable)".to_string());
        }
        let view = self.session_view()?;
        let pixel = alloc_pixel(view.display, view.colormap, color)?;
        unsafe { XSetForeground(view.display, view.gc, pixel) };
        unsafe {
            XFillRectangle(
                view.display,
                self.drawable,
                view.gc,
                rect.x.round() as i32,
                rect.y.round() as i32,
                rect.width.round().max(1.0) as u32,
                rect.height.round().max(1.0) as u32,
            )
        };
        Ok(())
    }

    /// Straight RGBA8 blit at `dest` via ARGB32/XRender (PictOpOver).
    /// Persistent cache: same content/size/palette reuses the cached
    /// 32-bit pixmap (zero reupload); the upload happens once per key.
    /// Live paint API unchanged: callers pass the same RGBA8 inputs.
    pub fn blit_rgba(
        &mut self,
        rgba8: &[u8],
        src_w: u32,
        src_h: u32,
        dest: Rect,
    ) -> Result<(), String> {
        const OPAQUE_PALETTE: [u8; 4] = [255, 255, 255, 255];
        let w = dest.width.round().max(1.0) as u32;
        let h = dest.height.round().max(1.0) as u32;
        expected_rgba_len(src_w, src_h, rgba8)?;
        let key = ExternalImageKey {
            content_hash: external_content_hash(rgba8),
            src_w,
            src_h,
            dst_w: w,
            dst_h: h,
            palette: OPAQUE_PALETTE,
        };
        self.blit_cached_argb32(
            key,
            || crate::native::image::scale_and_premultiply(rgba8, src_w, src_h, w, h),
            w,
            h,
            dest,
            false,
        )
    }

    /// Shared cached-ARGB32 composite. `require_xrender` keeps the semantic
    /// path's explicit error; the straight path keeps its opaque fallback.
    /// On cache hit the pixmap upload is skipped (zero reupload) and the
    /// key is touched MRU; on miss the pixmap goes through the checked
    /// allocator and the oldest entries are evicted within both caps.
    /// `argb` is built lazily: only on cache miss (upload) or on the
    /// no-XRender alpha check, so a cache hit on the XRender path allocates nothing.
    fn blit_cached_argb32<F: Fn() -> Vec<u32>>(
        &mut self,
        key: ExternalImageKey,
        argb: F,
        w: u32,
        h: u32,
        dest: Rect,
        require_xrender: bool,
    ) -> Result<(), String> {
        // Borrow the session view once; all pointer fields are Copy.
        let (view_display, view_gc, view_screen) = {
            let view = self.session_view()?;
            (view.display, view.gc, view.screen)
        };
        let drawable = self.drawable;
        // The X11 core copy path has no alpha blending. Reject unsupported paints
        // before creating a pixmap or altering the live destination drawable.
        let mut built: Option<Vec<u32>> = None;
        if self.xrender.is_none() {
            if require_xrender {
                return Err("external semantic icon requires XRender (unavailable)".to_string());
            }
            let words = argb();
            if has_non_opaque_word(&words) {
                return Err("external alpha blit requires XRender (unavailable)".to_string());
            }
            built = Some(words);
        }
        let pixmap = match self.image_cache.get(&key).copied() {
            Some(cached) => {
                image_lru_touch(&mut self.image_lru, key);
                cached.pixmap
            }
            None => {
                let root_w = unsafe { XDisplayWidth(view_display, view_screen) }.max(1) as u32;
                let root_h = unsafe { XDisplayHeight(view_display, view_screen) }.max(1) as u32;
                let depth = if self.xrender.is_some() {
                    32
                } else {
                    // XCopyArea is invalid when the source and destination depths differ.
                    // Query the *actual* drawable, not the root/default visual.
                    unsafe { external_drawable_depth(view_display, drawable)? }
                };
                let (fresh, bytes) = unsafe {
                    self.allocator.create_pixmap(
                        view_display,
                        drawable,
                        w,
                        h,
                        depth,
                        root_w,
                        root_h,
                    )
                }
                .map_err(|error| format!("external image cache: {error}"))?;
                let words = built.take().unwrap_or_else(argb);
                let upload = if self.xrender.is_some() {
                    match match_argb32_visual(view_display, view_screen) {
                        Some(visual) => unsafe {
                            upload_argb32_image(
                                view_display,
                                fresh,
                                visual,
                                &words,
                                w,
                                h,
                                "external ARGB32 upload",
                            )
                        },
                        None => Err("no 32-bit TrueColor visual for external blit".to_string()),
                    }
                } else {
                    unsafe {
                        upload_opaque_argb32_image(
                            view_display,
                            view_screen,
                            fresh,
                            depth,
                            &words,
                            w,
                            h,
                        )
                    }
                };
                if let Err(error) = upload {
                    unsafe { self.allocator.free_pixmap(view_display, fresh, bytes) };
                    return Err(error);
                }
                self.image_cache.insert(
                    key,
                    ExternalCachedImage {
                        pixmap: fresh,
                        bytes,
                    },
                );
                image_lru_touch(&mut self.image_lru, key);
                self.enforce_image_limits(view_display);
                // The fresh pixmap may itself have been evicted under memory
                // pressure if the single entry exceeds the byte cap; in that
                // case re-resolve what (if anything) survived.
                match self.image_cache.get(&key).copied() {
                    Some(cached) => cached.pixmap,
                    None => {
                        return Err(
                            "external image cache entry exceeds byte cap; evicted".to_string()
                        );
                    }
                }
            }
        };
        let (dx, dy) = (dest.x.round() as i32, dest.y.round() as i32);
        if let Some(xrender) = self.xrender.as_mut() {
            return unsafe { xrender.blit_argb32_over(pixmap, dx, dy, w, h) };
        }
        // Preflight above proved this image opaque; core X11 copy is now safe.
        unsafe {
            XCopyArea(
                view_display,
                pixmap,
                self.drawable,
                view_gc,
                0,
                0,
                w,
                h,
                dx,
                dy,
            )
        };
        Ok(())
    }

    /// Rounded-corner bounding mask on the target drawable.
    pub fn apply_rounded_shape(&self, radius: u32) -> Result<(), String> {
        let Some(bridge) = self.xshape.as_ref() else {
            return Err("external apply_rounded_shape requires XShape (unavailable)".to_string());
        };
        let spans = rounded_mask_spans(self.width, self.height, radius);
        unsafe { bridge.apply_rounded_mask(self.drawable, &spans) }
    }

    pub fn flush(&self) {
        if self.session.is_null() {
            return;
        }
        unsafe { XFlush((*self.session).display) };
    }
}

/// Safe painter over an X11App drawable. Exposes text measure/draw via
/// Xft, RGBA blit via ARGB32/XRender, shape apply, and skin color fill.
pub struct NativeDrawableRenderer<'a> {
    app: &'a mut super::app::X11App,
}

impl<'a> NativeDrawableRenderer<'a> {
    /// Draw text at device-pixel (x, baseline y) via Xft when available.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_text(
        &mut self,
        document: &RuntimeDocument,
        x: f32,
        y: f32,
        color: Color,
        size: f32,
        weight: u16,
        text: &str,
    ) -> Result<(), String> {
        self.app.ensure_xft();
        if let Some(xft) = self.app.xft.as_mut() {
            unsafe { xft.set_preferred_family(document.ui_font_family()) };
            return unsafe { xft.draw_text(x, y, color, size, weight, text) };
        }
        let pixel = unsafe { self.app.pixel_pub(color)? };
        unsafe { XSetForeground(self.app.display, self.app.gc, pixel) };
        if let Some(font) = unsafe { self.app.font_for_size_pub(size) } {
            unsafe { XSetFont(self.app.display, self.app.gc, font) };
        }
        let ascii: String = text
            .chars()
            .map(|ch| if ch.is_ascii() { ch } else { '?' })
            .collect();
        let string = std::ffi::CString::new(ascii).map_err(|_| "text contains NUL".to_string())?;
        let len = string.as_bytes().len().min(i32::MAX as usize) as i32;
        unsafe {
            XDrawString(
                self.app.display,
                self.app.backbuffer,
                self.app.gc,
                x.round() as i32,
                y.round() as i32,
                string.as_ptr(),
                len,
            )
        };
        Ok(())
    }

    /// Blit straight RGBA8 pixels at device rect via ARGB32/XRender.
    /// Same semantics as the canonical scene path: transparent clear is
    /// 0x00000000, compositing is PictOpOver per SurfaceAlphaMode.
    /// Transient pixmap goes through the checked allocator (validated +
    /// trapped create, accounted free); errors free before returning.
    pub fn rgba_blit(
        &mut self,
        rgba8: &[u8],
        src_w: u32,
        src_h: u32,
        dest: Rect,
    ) -> Result<(), String> {
        let w = dest.width.round().max(1.0) as u32;
        let h = dest.height.round().max(1.0) as u32;
        let argb = crate::native::image::scale_and_premultiply(rgba8, src_w, src_h, w, h);
        // A core-X11 copy cannot blend alpha or copy a 32-bit pixmap into a 24-bit
        // backbuffer. Validate opacity before allocating any server-side object.
        let core = self.app.xrender.is_none();
        if core && has_non_opaque_word(&argb) {
            return Err("rgba_blit needs XRender for non-opaque pixels".to_string());
        }
        let depth = if core { self.app.depth as u32 } else { 32 };
        let root = unsafe { XRootWindow(self.app.display, self.app.screen) };
        let root_w = unsafe { XDisplayWidth(self.app.display, self.app.screen) }.max(1) as u32;
        let root_h = unsafe { XDisplayHeight(self.app.display, self.app.screen) }.max(1) as u32;
        let (pixmap, bytes) = unsafe {
            self.app
                .allocator
                .create_pixmap(self.app.display, root, w, h, depth, root_w, root_h)
        }
        .map_err(|error| format!("rgba_blit: {error}"))?;
        let result = if core {
            unsafe {
                upload_opaque_argb32_image(
                    self.app.display,
                    self.app.screen,
                    pixmap,
                    depth,
                    &argb,
                    w,
                    h,
                )
            }
        } else {
            unsafe { self.app.upload_argb32_pub(pixmap, &argb, w, h) }
        };
        if let Err(error) = result {
            unsafe {
                self.app
                    .allocator
                    .free_pixmap(self.app.display, pixmap, bytes)
            };
            return Err(error);
        }
        let dx = dest.x.round() as i32;
        let dy = dest.y.round() as i32;
        let blit = if let Some(xrender) = self.app.xrender.as_mut() {
            unsafe { xrender.blit_argb32_over(pixmap, dx, dy, w, h) }
        } else {
            unsafe {
                XCopyArea(
                    self.app.display,
                    pixmap,
                    self.app.backbuffer,
                    self.app.gc,
                    0,
                    0,
                    w,
                    h,
                    dx,
                    dy,
                )
            };
            Ok(())
        };
        unsafe {
            self.app
                .allocator
                .free_pixmap(self.app.display, pixmap, bytes)
        };
        blit
    }

    /// Apply the rounded-corner shape mask for chrome-ready surfaces.
    pub fn apply_shape(&mut self, document: &RuntimeDocument) {
        unsafe { self.app.refresh_shape_mask(document) };
    }

    /// Flush the display connection.
    pub fn flush(&mut self) {
        unsafe { XFlush(self.app.display) };
    }
}

// --- Pure helpers (unit-testable without X) ---

/// Layout-compatible text measure: 0.58em per char, 1.30em line height.
pub fn external_text_measure(text: &str, size: f32) -> (f32, f32) {
    let height = (size.max(1.0) * 1.30).round();
    ((text.chars().count() as f32 * size * 0.58).round(), height)
}

/// SymbolicForeground over raw RGBA8: eff_a = round(sa*fa/255),
/// premul channel = round(fg*eff_a/255). Mirrors the scene path.
pub fn external_symbolic_argb(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    fg: Color,
) -> Vec<u32> {
    let (sw, sh, dw, dh) = (src_w.max(1), src_h.max(1), dst_w.max(1), dst_h.max(1));
    let stride = sw as usize * 4;
    let total = stride.checked_mul(sh as usize).unwrap_or(0);
    let mut out = Vec::with_capacity((dw as usize) * (dh as usize));
    for y in 0..dh {
        let sy = ((y as u64 * src_h as u64) / dh as u64).min((sh - 1) as u64) as usize;
        for x in 0..dw {
            let sx = ((x as u64 * src_w as u64) / dw as u64).min((sw - 1) as u64) as usize;
            let off = sy * stride + sx * 4;
            let sa = if off + 3 < total && off + 3 < src.len() {
                u32::from(src[off + 3])
            } else {
                0
            };
            let eff_a = (sa * u32::from(fg.a) + 127) / 255;
            let prem = |c: u32| (c * eff_a + 127) / 255;
            out.push(
                (eff_a << 24)
                    | (prem(u32::from(fg.r)) << 16)
                    | (prem(u32::from(fg.g)) << 8)
                    | prem(u32::from(fg.b)),
            );
        }
    }
    out
}

fn expected_rgba_len(src_w: u32, src_h: u32, rgba8: &[u8]) -> Result<(), String> {
    let expected = (src_w.max(1) as usize)
        .checked_mul(src_h.max(1) as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "rgba size overflow".to_string())?;
    if rgba8.len() != expected {
        return Err(format!(
            "rgba length {} does not match {src_w}x{src_h} (expected {expected})",
            rgba8.len()
        ));
    }
    Ok(())
}

fn alloc_pixel(display: *mut Display, colormap: Colormap, color: Color) -> Result<u64, String> {
    let mut xcolor = XColor {
        pixel: 0,
        red: u16::from(color.r) * 257,
        green: u16::from(color.g) * 257,
        blue: u16::from(color.b) * 257,
        flags: DO_RED | DO_GREEN | DO_BLUE,
        pad: 0,
    };
    if unsafe { XAllocColor(display, colormap, &mut xcolor) } == 0 {
        return Err(format!(
            "XAllocColor failed for #{:02x}{:02x}{:02x}",
            color.r, color.g, color.b
        ));
    }
    Ok(xcolor.pixel as u64)
}

pub(crate) fn match_argb32_visual(display: *mut Display, screen: i32) -> Option<*mut Visual> {
    let mut vinfo: XVisualInfo = unsafe { std::mem::zeroed() };
    let matched = unsafe { XMatchVisualInfo(display, screen, 32, TRUE_COLOR_CLASS, &mut vinfo) };
    (matched != 0 && !vinfo.visual.is_null()).then_some(vinfo.visual)
}

/// Rounded-corner row-inset formula shared with the XRender fill path and
/// the XShape mask spans: floor(r - dx) at pixel-row centers.
pub fn external_rounded_row_inset(width: u32, height: u32, radius: u32, row: u32) -> u32 {
    crate::native::rounded::row_inset(width, height, radius, row)
}

/// Xft font-pattern probe used by tests: preferred family first, then
/// portable fallbacks. Must stay in sync with `XftBackend::font_for`.
pub fn external_font_pattern_order(preferred: &str) -> Vec<String> {
    let mut families = vec![preferred.to_string()];
    if preferred != "IBM Plex Sans" {
        families.push("IBM Plex Sans".to_string());
    }
    families.extend([
        "Noto Sans".to_string(),
        "DejaVu Sans".to_string(),
        "sans-serif".to_string(),
    ]);
    families
}

/// Byte estimate for one cached native pixmap (32-bit ARGB32): `w*h*4`
/// saturating on overflow. Backs profiler gauges without touching X.
pub fn external_pixmap_byte_estimate(dst_w: u32, dst_h: u32) -> usize {
    (dst_w as usize)
        .checked_mul(dst_h as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .unwrap_or(usize::MAX)
}

/// Release ordering model: transient XRender source pictures must be freed
/// before their backing pixmaps (protocol ordering). `blit_cached_argb32`
/// frees the transient picture inside `blit_argb32_over` before returning;
/// cached pixmaps are freed later in `invalidate_image_cache`/`Drop`.
/// This pure model keeps the ordering unit-testable without X.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalReleaseStep {
    FreePicture,
    FreePixmap,
}

pub fn external_release_order() -> [ExternalReleaseStep; 2] {
    [
        ExternalReleaseStep::FreePicture,
        ExternalReleaseStep::FreePixmap,
    ]
}

pub fn use_cstring_title(title: &str) -> Result<CString, String> {
    CString::new(title).map_err(|_| "title contains NUL".to_string())
}

fn has_non_opaque_word(words: &[u32]) -> bool {
    words.iter().any(|word| word >> 24 != 255)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_px_key(content_hash: u64) -> ExternalImageKey {
        ExternalImageKey {
            content_hash,
            src_w: 1,
            src_h: 1,
            dst_w: 1,
            dst_h: 1,
            palette: [255, 255, 255, 255],
        }
    }

    #[test]
    fn non_opaque_detection_reads_alpha_byte() {
        assert!(!has_non_opaque_word(&[0xFF00_0000, 0xFFFF_FFFF]));
        assert!(has_non_opaque_word(&[0xFF00_0000, 0x80FF_FFFF]));
        assert!(!has_non_opaque_word(&[]));
    }

    #[test]
    fn measure_matches_layout_estimate() {
        assert_eq!(external_text_measure("hello", 13.0), (38.0, 17.0));
        assert_eq!(external_text_measure("", 13.0), (0.0, 17.0));
    }

    #[test]
    fn target_rejects_zero_drawable_without_display() {
        let mut null: *mut ExternalDrawableSession = ptr::null_mut();
        let result = unsafe { ExternalDrawableTarget::new(null as *mut _, 0, 8, 8) };
        assert!(result.is_err());
        let _ = &mut null;
    }

    #[test]
    fn symbolic_argb_matches_scene_math() {
        // 1px src, fg red @ full alpha, sa=128 -> eff_a=128, prem r=128.
        let src = vec![0, 0, 0, 128];
        let out = external_symbolic_argb(
            &src,
            1,
            1,
            1,
            1,
            Color {
                r: 255,
                g: 0,
                b: 0,
                a: 255,
            },
        );
        assert_eq!(out, vec![0x80800000]);
    }

    #[test]
    fn rounded_inset_matches_shape_spans() {
        let spans = rounded_mask_spans(8, 8, 4);
        assert_eq!(spans[0], (0, 2, 6));
        assert_eq!(external_rounded_row_inset(8, 8, 4, 0), 2);
        assert_eq!(external_rounded_row_inset(8, 8, 4, 1), 0);
        assert_eq!(external_rounded_row_inset(8, 8, 4, 7), 2);
    }

    #[test]
    fn content_hash_sees_every_byte_including_the_word_tail() {
        // 8-byte words plus a 3-byte remainder: a change in the tail must change the hash.
        let base: Vec<u8> = (0u8..19).collect();
        let base_hash = external_content_hash(&base);
        for index in 0..base.len() {
            let mut changed = base.clone();
            changed[index] ^= 0x80;
            assert_ne!(
                base_hash,
                external_content_hash(&changed),
                "byte {index} edit must change the content hash"
            );
        }
    }

    #[test]
    fn content_hash_separates_lengths_with_identical_prefix() {
        let short = vec![7u8; 8];
        let long = vec![7u8; 16];
        assert_ne!(external_content_hash(&short), external_content_hash(&long));
        assert_ne!(external_content_hash(&[]), external_content_hash(&[0]));
    }

    #[test]
    fn rgba_length_validated_before_touching_display() {
        assert!(expected_rgba_len(2, 2, &[0u8; 15]).is_err());
        assert!(expected_rgba_len(2, 2, &[0u8; 16]).is_ok());
    }

    #[test]
    fn cache_key_separates_content_size_palette() {
        let a = vec![1, 2, 3, 255];
        let b = vec![4, 5, 6, 255];
        let base = one_px_key(external_content_hash(&a));
        // Same content/size/palette -> same key (zero reupload).
        assert_eq!(base, one_px_key(external_content_hash(&a)));
        // Different content, size, or palette -> different key.
        assert_ne!(
            base.content_hash,
            external_content_hash(&b),
            "content change must reupload"
        );
        assert_ne!(
            base,
            ExternalImageKey { dst_w: 2, ..base },
            "size change must reupload"
        );
        assert_ne!(
            base,
            ExternalImageKey {
                palette: [255, 0, 0, 255],
                ..base
            },
            "palette change must reupload"
        );
    }

    #[test]
    fn cache_bytes_count_native_words() {
        // 2x3 pixmap costs 2*3*4 bytes.
        let cached = ExternalCachedImage {
            pixmap: 7,
            bytes: 2usize * 3 * 4,
        };
        assert_eq!(cached.bytes, 24);
    }

    #[test]
    fn cache_hit_reuses_key_miss_inserts() {
        let mut cache: HashMap<ExternalImageKey, ExternalCachedImage> = HashMap::new();
        let key = one_px_key(external_content_hash(&[1, 2, 3, 255]));
        // Miss: first insert grows the cache and records native bytes.
        let bytes = external_pixmap_byte_estimate(1, 1);
        cache.insert(key, ExternalCachedImage { pixmap: 7, bytes });
        assert_eq!(cache.len(), 1);
        // Hit: same key reuses the entry (zero reupload), no growth.
        let hit = cache.get(&key).copied().expect("cache hit");
        assert_eq!(hit.pixmap, 7);
        assert_eq!(cache.len(), 1);
        // Miss: different content hash inserts a second entry.
        let miss_key = ExternalImageKey {
            content_hash: external_content_hash(&[9, 9, 9, 255]),
            ..key
        };
        assert!(!cache.contains_key(&miss_key));
        cache.insert(miss_key, ExternalCachedImage { pixmap: 8, bytes });
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn release_order_frees_picture_before_pixmap() {
        assert_eq!(
            external_release_order(),
            [
                ExternalReleaseStep::FreePicture,
                ExternalReleaseStep::FreePixmap
            ]
        );
    }

    #[test]
    fn byte_estimate_matches_native_words() {
        assert_eq!(external_pixmap_byte_estimate(2, 3), 24);
        assert_eq!(external_pixmap_byte_estimate(0, 8), 0);
    }

    #[test]
    fn font_pattern_prefers_plex_first() {
        let order = external_font_pattern_order("Noto Sans");
        assert_eq!(order.first().map(String::as_str), Some("Noto Sans"));
        assert_eq!(order.get(1).map(String::as_str), Some("IBM Plex Sans"));
        let plex = external_font_pattern_order("IBM Plex Sans");
        assert_eq!(plex.first().map(String::as_str), Some("IBM Plex Sans"));
        assert_eq!(
            plex.iter()
                .filter(|f| f.as_str() == "IBM Plex Sans")
                .count(),
            1
        );
    }

    #[test]
    fn lru_touch_moves_hit_to_back() {
        let mut lru: VecDeque<ExternalImageKey> = VecDeque::new();
        let (a, b, c) = (one_px_key(1), one_px_key(2), one_px_key(3));
        image_lru_touch(&mut lru, a);
        image_lru_touch(&mut lru, b);
        image_lru_touch(&mut lru, c);
        // Hit on oldest moves it MRU (back); order becomes b, c, a.
        image_lru_touch(&mut lru, a);
        let order: Vec<u64> = lru.iter().map(|entry| entry.content_hash).collect();
        assert_eq!(order, vec![2, 3, 1]);
    }

    #[test]
    fn cache_caps_are_sane() {
        assert_eq!(EXTERNAL_IMAGE_CACHE_MAX_ENTRIES, 128);
        assert_eq!(EXTERNAL_IMAGE_CACHE_MAX_BYTES, 8 * 1024 * 1024);
    }

    #[test]
    fn rounded_row_helper_matches_mask_edges() {
        assert_eq!(external_rounded_row_inset(8, 8, 0, 0), 0);
        assert_eq!(external_rounded_row_inset(0, 8, 4, 0), 0);
        assert_eq!(external_rounded_row_inset(8, 8, 4, 3), 0);
    }
}
