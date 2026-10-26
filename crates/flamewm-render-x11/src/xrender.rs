#![allow(non_camel_case_types)]

use std::collections::HashMap;
use std::os::raw::{c_int, c_ulong, c_void};

use flamewm_render_core::{Color, Rect};

use crate::ffi::dynamic_library::DynamicLibrary;
use crate::native::xresource::take_pixmap_invalidations;
use crate::xlib::{Display, Drawable, Pixmap, Visual};

/// Upper bound on retained ARGB32 source pictures; reaching it releases them all.
const PIXMAP_PICTURE_CAP: usize = 64;

const PICT_OP_OVER: c_int = 3;

type Picture = c_ulong;

#[repr(C)]
struct XRenderPictFormat {
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

type FindVisualFormatFn = unsafe extern "C" fn(*mut Display, *mut Visual) -> *mut XRenderPictFormat;
type FindStandardFormatFn = unsafe extern "C" fn(*mut Display, c_int) -> *mut XRenderPictFormat;
type CreatePictureFn = unsafe extern "C" fn(
    *mut Display,
    Drawable,
    *mut XRenderPictFormat,
    c_ulong,
    *const c_void,
) -> Picture;
type CreateSolidFillFn = unsafe extern "C" fn(*mut Display, *const XRenderColor) -> Picture;
type CompositeFn = unsafe extern "C" fn(
    *mut Display,
    c_int,
    Picture,
    Picture,
    Picture,
    c_int,
    c_int,
    c_int,
    c_int,
    c_int,
    c_int,
    u32,
    u32,
);
type FreePictureFn = unsafe extern "C" fn(*mut Display, Picture);

/// `XRenderSetPictureClipRectangles(display, picture, xOrigin, yOrigin, rects, n)`.
type SetPictureClipFn = unsafe extern "C" fn(
    *mut Display,
    Picture,
    c_int,
    c_int,
    *const crate::xlib::XRectangle,
    c_int,
);

#[derive(Clone, Copy)]
struct XRenderApi {
    find_visual_format: FindVisualFormatFn,
    find_standard_format: FindStandardFormatFn,
    create_picture: CreatePictureFn,
    create_solid_fill: CreateSolidFillFn,
    composite: CompositeFn,
    free_picture: FreePictureFn,
    /// Optional: absent on very old libXrender; region painting then stays Full.
    set_picture_clip: Option<SetPictureClipFn>,
}

pub struct XRenderBackend {
    display: *mut Display,
    destination: Picture,
    format: *mut XRenderPictFormat,
    api: XRenderApi,
    _library: Option<DynamicLibrary>,
    solids: HashMap<Color, Picture>,
    /// ARGB32 source pictures keyed by pixmap XID. Entries are dropped when the
    /// pixmap is freed through `free_pixmap_raw` (drained before every lookup).
    pixmap_pictures: HashMap<Pixmap, Picture>,
}

impl XRenderBackend {
    /// # Safety
    ///
    /// `display` must be a live Xlib display and `drawable`/`visual` valid for
    /// it; the loaded XRender symbols stay alive inside the returned backend.
    pub unsafe fn new(
        display: *mut Display,
        drawable: Drawable,
        visual: *mut Visual,
    ) -> Result<Self, String> {
        let library = DynamicLibrary::open(&["libXrender.so.1", "libXrender.so"])?;
        // SAFETY: each name is a static NUL-terminated symbol and `T` is the exact
        // function-pointer type declared in `XRenderApi`, matching the C ABI.
        let api = XRenderApi {
            find_visual_format: unsafe { library.symbol(b"XRenderFindVisualFormat\0") }?,
            find_standard_format: unsafe { library.symbol(b"XRenderFindStandardFormat\0") }?,
            create_picture: unsafe { library.symbol(b"XRenderCreatePicture\0") }?,
            create_solid_fill: unsafe { library.symbol(b"XRenderCreateSolidFill\0") }?,
            composite: unsafe { library.symbol(b"XRenderComposite\0") }?,
            free_picture: unsafe { library.symbol(b"XRenderFreePicture\0") }?,
            set_picture_clip: unsafe { library.symbol(b"XRenderSetPictureClipRectangles\0") }.ok(),
        };
        // SAFETY: `display`/`visual` are live per `new`'s contract; NULL is rejected below.
        let format = unsafe { (api.find_visual_format)(display, visual) };
        if format.is_null() {
            return Err("XRenderFindVisualFormat returned NULL".to_string());
        }
        // SAFETY: same live `display`/`format`; NULL attributes select defaults.
        let destination =
            unsafe { (api.create_picture)(display, drawable, format, 0, std::ptr::null()) };
        if destination == 0 {
            return Err("XRenderCreatePicture returned 0".to_string());
        }
        Ok(Self {
            display,
            destination,
            format,
            api,
            _library: Some(library),
            solids: HashMap::new(),
            pixmap_pictures: HashMap::new(),
        })
    }

    /// Release XRender pictures while the display is live, then transfer the
    /// dynamic-library handle to the display lifetime owner. The handle must
    /// remain loaded through XCloseDisplay.
    pub(crate) unsafe fn into_deferred_libraries(mut self) -> Vec<DynamicLibrary> {
        unsafe { self.release_resources() };
        let library = self._library.take();
        drop(self);
        library.into_iter().collect()
    }

    unsafe fn release_resources(&mut self) {
        unsafe { self.release_pixmap_pictures() };
        for (_, picture) in self.solids.drain() {
            // SAFETY: the display remains live and the drained picture is backend-owned.
            unsafe { (self.api.free_picture)(self.display, picture) };
        }
        if self.destination != 0 {
            // SAFETY: the display remains live and `destination` is a live backend-owned picture.
            unsafe { (self.api.free_picture)(self.display, self.destination) };
            self.destination = 0;
        }
    }

    /// Free every cached pixmap source picture.
    unsafe fn release_pixmap_pictures(&mut self) {
        for (_, picture) in self.pixmap_pictures.drain() {
            // SAFETY: the display is live (callers hold it) and the picture is backend-owned.
            unsafe { (self.api.free_picture)(self.display, picture) };
        }
    }

    /// Source picture for an ARGB32 pixmap, created once and reused until the
    /// pixmap is freed or the cache is full.
    ///
    /// # Safety
    ///
    /// `source` must be a live pixmap on this backend's display.
    unsafe fn argb32_source_picture(&mut self, source: Drawable) -> Result<Picture, String> {
        let (freed, overflowed) = take_pixmap_invalidations();
        if overflowed {
            unsafe { self.release_pixmap_pictures() };
        } else {
            for pixmap in freed {
                if let Some(picture) = self.pixmap_pictures.remove(&pixmap) {
                    // SAFETY: the display is live and the picture is backend-owned.
                    unsafe { (self.api.free_picture)(self.display, picture) };
                }
            }
        }
        if let Some(picture) = self.pixmap_pictures.get(&source) {
            return Ok(*picture);
        }
        // PictStandardARGB32 == 0 in render.h.
        let format = unsafe { (self.api.find_standard_format)(self.display, 0) };
        if format.is_null() {
            return Err("XRenderFindStandardFormat(ARGB32) returned NULL".to_string());
        }
        if self.pixmap_pictures.len() >= PIXMAP_PICTURE_CAP {
            unsafe { self.release_pixmap_pictures() };
        }
        // SAFETY: `source` is live per the caller contract; `format` was just validated.
        let picture =
            unsafe { (self.api.create_picture)(self.display, source, format, 0, std::ptr::null()) };
        if picture == 0 {
            return Err("XRenderCreatePicture returned 0 for ARGB32 image blit".to_string());
        }
        self.pixmap_pictures.insert(source, picture);
        Ok(picture)
    }

    /// Number of cached pixmap source pictures (test visibility only).
    #[cfg(test)]
    pub fn cached_pixmap_pictures(&self) -> usize {
        self.pixmap_pictures.len()
    }

    #[cfg(test)]
    pub fn cached_pixmap_contains(&self, pixmap: Pixmap) -> bool {
        self.pixmap_pictures.contains_key(&pixmap)
    }

    pub unsafe fn set_drawable(&mut self, drawable: Drawable) -> Result<(), String> {
        // SAFETY: `display`/`format` stay live in this backend and `drawable` is
        // valid per the caller's contract; zero return is reported below.
        let replacement = unsafe {
            (self.api.create_picture)(self.display, drawable, self.format, 0, std::ptr::null())
        };
        if replacement == 0 {
            return Err("XRenderCreatePicture returned 0 while changing drawable".to_string());
        }
        if self.destination != 0 {
            // SAFETY: `destination` is a live picture owned by this backend.
            unsafe { (self.api.free_picture)(self.display, self.destination) };
        }
        self.destination = replacement;
        Ok(())
    }

    /// Restricts every later composite into the destination picture to `rect` (device
    /// pixels). XRender ignores the GC clip, so region painting sets this per frame and
    /// resets it to the full window before returning.
    pub(crate) unsafe fn set_destination_clip(
        &mut self,
        rect: crate::xlib::XRectangle,
    ) -> Result<(), String> {
        let Some(set_clip) = self.api.set_picture_clip else {
            return Err("XRenderSetPictureClipRectangles unavailable".to_string());
        };
        // SAFETY: `display`/`destination` are live for this backend; `rect` is one
        // element that outlives the call.
        unsafe { set_clip(self.display, self.destination, 0, 0, &rect, 1) };
        Ok(())
    }

    pub unsafe fn fill_rounded_rect(
        &mut self,
        rect: Rect,
        radius: f32,
        color: Color,
    ) -> Result<(), String> {
        let x = rect.x.round() as i32;
        let y = rect.y.round() as i32;
        let width = rect.width.round().max(0.0) as u32;
        let height = rect.height.round().max(0.0) as u32;
        if width == 0 || height == 0 || color.a == 0 {
            return Ok(());
        }
        let radius = radius.round().max(0.0).min((width.min(height) / 2) as f32) as u32;
        if radius <= 1 {
            // SAFETY: same validated rect/color inputs as `fill_rounded_rect`.
            return unsafe { self.fill_rect(x, y, width, height, color) };
        }

        // Paint each destination pixel at most once. With PictOpOver, overlapping
        // translucent rectangles compound alpha and create bright vertical/horizontal
        // strips. Keep the large middle band as one request, then rasterize only the
        // rounded corner rows. This also avoids one XRender request per pixel row on
        // large selection/snap rectangles during pointer motion.
        let middle_height = height.saturating_sub(radius * 2);
        if middle_height > 0 {
            // SAFETY: same validated inputs as `fill_rounded_rect`.
            unsafe { self.fill_rect(x, y + radius as i32, width, middle_height, color)? };
        }
        for row in 0..radius {
            let inset = rounded_row_inset(width, height, radius, row);
            let span = width.saturating_sub(inset * 2);
            if span == 0 {
                continue;
            }
            // SAFETY: same validated inputs as `fill_rounded_rect`.
            unsafe { self.fill_rect(x + inset as i32, y + row as i32, span, 1, color)? };
            let bottom_row = height - 1 - row;
            if bottom_row != row {
                // SAFETY: same validated inputs as `fill_rounded_rect`.
                unsafe { self.fill_rect(x + inset as i32, y + bottom_row as i32, span, 1, color)? };
            }
        }
        Ok(())
    }

    unsafe fn fill_rect(
        &mut self,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        color: Color,
    ) -> Result<(), String> {
        let source = unsafe { self.solid(color) }?;
        // SAFETY: `source`/`destination` are live pictures owned by this backend
        // and `display` stays live for the whole backend lifetime.
        unsafe {
            (self.api.composite)(
                self.display,
                PICT_OP_OVER,
                source,
                0,
                self.destination,
                0,
                0,
                0,
                0,
                x,
                y,
                width,
                height,
            )
        };
        Ok(())
    }

    /// ARGB32 premultiplied source blit with true alpha through PictOpOver.
    /// `source` must be a depth-32 pixmap holding premultiplied ARGB32.
    /// The source picture (standard ARGB32 format) is cached per pixmap and
    /// released when that pixmap is freed.
    ///
    /// # Safety
    ///
    /// `source` must be a live pixmap on this backend's display.
    pub unsafe fn blit_argb32_over(
        &mut self,
        source: Drawable,
        dx: i32,
        dy: i32,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        unsafe { self.blit_argb32_over_from(source, (0, 0), (dx, dy), width, height) }
    }

    /// Like `blit_argb32_over`, but copies the `width` x `height` source sub-rect at `src`
    /// (source pixels) to `dst` on the destination. A region repaint crops an image to its
    /// region this way instead of compositing the whole source.
    ///
    /// # Safety
    ///
    /// Same as `blit_argb32_over`.
    pub unsafe fn blit_argb32_over_from(
        &mut self,
        source: Drawable,
        src: (i32, i32),
        dst: (i32, i32),
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        if width == 0 || height == 0 {
            return Ok(());
        }
        let picture = unsafe { self.argb32_source_picture(source) }?;
        unsafe {
            (self.api.composite)(
                self.display,
                PICT_OP_OVER,
                picture,
                0,
                self.destination,
                src.0,
                src.1,
                0,
                0,
                dst.0,
                dst.1,
                width,
                height,
            )
        };
        Ok(())
    }

    unsafe fn solid(&mut self, color: Color) -> Result<Picture, String> {
        if let Some(picture) = self.solids.get(&color).copied() {
            return Ok(picture);
        }
        // Render protocol colors are premultiplied by alpha. Passing full RGB
        // channels with a low alpha makes translucent overlays appear far too
        // bright because PictOpOver treats the source channels as already
        // premultiplied. Convert 8-bit straight-alpha Color into the 16-bit
        // premultiplied representation expected by XRender.
        let value = XRenderColor {
            red: premultiplied_u16(color.r, color.a),
            green: premultiplied_u16(color.g, color.a),
            blue: premultiplied_u16(color.b, color.a),
            alpha: u16::from(color.a) * 257,
        };
        // SAFETY: `display` is live per the backend contract; `value` is a valid
        // stack reference for the call. A zero picture is reported below.
        let picture = unsafe { (self.api.create_solid_fill)(self.display, &value) };
        if picture == 0 {
            return Err("XRenderCreateSolidFill returned 0".to_string());
        }
        self.solids.insert(color, picture);
        Ok(picture)
    }
}

fn rounded_row_inset(width: u32, height: u32, radius: u32, row: u32) -> u32 {
    if radius <= 1 || width == 0 || height == 0 {
        return 0;
    }
    let corner_row = row.min(height.saturating_sub(1).saturating_sub(row));
    if corner_row >= radius {
        return 0;
    }
    let r = radius as f64;
    let dy = r - (corner_row as f64 + 0.5);
    let dx = (r * r - dy * dy).max(0.0).sqrt();
    (r - dx).floor().clamp(0.0, r) as u32
}

fn premultiplied_u16(channel: u8, alpha: u8) -> u16 {
    let straight = u32::from(channel) * 257;
    let a = u32::from(alpha) * 257;
    ((straight * a + 32767) / 65535) as u16
}

impl Drop for XRenderBackend {
    fn drop(&mut self) {
        // SAFETY: teardown only frees backend-owned pictures while `display` is
        // live; draining first guarantees each picture is released once.
        unsafe { self.release_resources() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xrender_solid_channels_are_premultiplied() {
        assert_eq!(premultiplied_u16(255, 51), 13107);
    }

    #[test]
    fn opaque_channel_is_unchanged() {
        assert_eq!(premultiplied_u16(200, 255), 51400);
    }

    /// A freed pixmap's cached source picture must not survive into a later blit,
    /// because its XID can be reused. Needs `FLAMEWM_TEST_DISPLAY` (skipped otherwise).
    #[test]
    fn freed_pixmap_drops_cached_source_picture() {
        use crate::native::xresource::free_pixmap_raw;
        use crate::xlib::{
            XCloseDisplay, XCreatePixmap, XDefaultScreen, XDefaultVisual, XOpenDisplay, XRootWindow,
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
            let mut backend = XRenderBackend::new(display, root, visual).expect("xrender");
            let first = XCreatePixmap(display, root, 8, 8, 32);
            let second = XCreatePixmap(display, root, 8, 8, 32);
            assert!(first != 0 && second != 0);
            backend
                .blit_argb32_over(first, 0, 0, 8, 8)
                .expect("blit first");
            backend
                .blit_argb32_over(first, 0, 0, 8, 8)
                .expect("blit first again");
            assert_eq!(
                backend.cached_pixmap_pictures(),
                1,
                "one picture per pixmap"
            );
            free_pixmap_raw(display, first);
            backend
                .blit_argb32_over(second, 0, 0, 8, 8)
                .expect("blit second");
            assert!(
                !backend.cached_pixmap_contains(first),
                "freed pixmap leaked"
            );
            assert!(backend.cached_pixmap_contains(second));
            assert_eq!(backend.cached_pixmap_pictures(), 1);
            free_pixmap_raw(display, second);
            // The XRender library must stay loaded through XCloseDisplay, as in production.
            let libraries = backend.into_deferred_libraries();
            XCloseDisplay(display);
            drop(libraries);
        }
    }
}
