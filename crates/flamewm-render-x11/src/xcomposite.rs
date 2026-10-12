//! Minimal XComposite bridge and bounded window capture for transitions.
//!
//! libXcomposite is loaded with `dlopen` like the other extensions. Nothing
//! here redirects windows, so no compositing manager is displaced (`_NET_WM_CM_Sn`
//! is never touched). `capture_window` reads the window with XGetImage; this
//! is the only capture per transition. `name_window_pixmap` is valid only for
//! a window that is already redirected by another compositor; it is not used
//! to capture an ordinary mapped window.

use std::os::raw::{c_char, c_int, c_uint, c_void};

use crate::backdrop::{BackdropCapture, capture_drawable_rect};
use crate::ffi::dynamic_library::DynamicLibrary;
use crate::xlib::{
    Display, Pixmap, Window, XCreateGC, XCreateImage, XCreatePixmap, XDefaultScreen,
    XDefaultVisual, XDestroyImage, XFreeGC, XFreePixmap, XPutImage, XRootWindow, ZPIXMAP, malloc,
};
use crate::xlib_ext::{XGetGeometry, free};

/// Largest capture accepted (4K), bounding the copy and upload cost.
pub const MAX_CAPTURE_PIXELS: u64 = 3840 * 2160;

/// Premultiplied native-endian ARGB32 pixels, `width * height * 4` bytes, in the
/// layout XRender and XPutImage consume for depth 32.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedImage {
    pub width: u32,
    pub height: u32,
    pub argb: Vec<u8>,
}

type QueryExtensionFn = unsafe extern "C" fn(*mut Display, *mut c_int, *mut c_int) -> c_int;
type QueryVersionFn = unsafe extern "C" fn(*mut Display, *mut c_int, *mut c_int) -> c_int;
type NameWindowPixmapFn = unsafe extern "C" fn(*mut Display, Window) -> Pixmap;

pub struct XCompositeApi {
    _library: DynamicLibrary,
    query_extension: QueryExtensionFn,
    query_version: QueryVersionFn,
    name_window_pixmap: NameWindowPixmapFn,
}

impl XCompositeApi {
    pub fn load() -> Result<Self, String> {
        let library = DynamicLibrary::open(&["libXcomposite.so.1", "libXcomposite.so"])?;
        // SAFETY: names are static NUL-terminated; `T` matches the libXcomposite C ABI.
        let query_extension = unsafe { library.symbol(b"XCompositeQueryExtension\0") }?;
        // SAFETY: as above.
        let query_version = unsafe { library.symbol(b"XCompositeQueryVersion\0") }?;
        // SAFETY: as above.
        let name_window_pixmap = unsafe { library.symbol(b"XCompositeNameWindowPixmap\0") }?;
        Ok(Self {
            _library: library,
            query_extension,
            query_version,
            name_window_pixmap,
        })
    }

    /// Server Composite version as `(major, minor)`.
    ///
    /// # Safety
    ///
    /// `display` must be a live Xlib display.
    pub unsafe fn version(&self, display: *mut Display) -> Result<(i32, i32), String> {
        if display.is_null() {
            return Err("XComposite: null display".to_string());
        }
        let (mut event_base, mut error_base) = (0, 0);
        // SAFETY: live display and valid out-pointers per contract.
        if unsafe { (self.query_extension)(display, &mut event_base, &mut error_base) } == 0 {
            return Err("XComposite extension not present".to_string());
        }
        let (mut major, mut minor) = (0, 0);
        // SAFETY: live display and valid out-pointers per contract.
        if unsafe { (self.query_version)(display, &mut major, &mut minor) } == 0 {
            return Err("XCompositeQueryVersion failed".to_string());
        }
        Ok((major, minor))
    }

    /// Pixmap naming the backing store of a window that is already redirected.
    /// Does not redirect; the caller frees the pixmap.
    ///
    /// # Safety
    ///
    /// `display` must be live and `window` a window on it that is redirected.
    pub unsafe fn name_window_pixmap(
        &self,
        display: *mut Display,
        window: Window,
    ) -> Result<Pixmap, String> {
        if display.is_null() || window == 0 {
            return Err("XCompositeNameWindowPixmap: no display/window".to_string());
        }
        // SAFETY: caller contract; failure is reported by a zero pixmap.
        let pixmap = unsafe { (self.name_window_pixmap)(display, window) };
        if pixmap == 0 {
            return Err("XCompositeNameWindowPixmap returned 0".to_string());
        }
        Ok(pixmap)
    }
}

/// Rejects empty and oversize captures before any allocation.
pub fn check_capture_size(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("capture: empty size".to_string());
    }
    if u64::from(width) * u64::from(height) > MAX_CAPTURE_PIXELS {
        return Err(format!("capture: {width}x{height} exceeds {MAX_CAPTURE_PIXELS} pixels"));
    }
    Ok(())
}

fn premultiply(channel: u8, alpha: u8) -> u32 {
    (u32::from(channel) * u32::from(alpha) + 127) / 255
}

/// Converts straight RGBA8 (`BackdropCapture` layout) to premultiplied ARGB32.
pub fn rgba_to_argb(width: u32, height: u32, rgba: &[u8]) -> Result<CapturedImage, String> {
    check_capture_size(width, height)?;
    let pixel_count = width as usize * height as usize;
    if rgba.len() != pixel_count * 4 {
        return Err("capture: RGBA buffer length mismatch".to_string());
    }
    let mut argb = Vec::with_capacity(pixel_count * 4);
    for px in rgba.chunks_exact(4) {
        let a = px[3];
        let packed = (u32::from(a) << 24)
            | (premultiply(px[0], a) << 16)
            | (premultiply(px[1], a) << 8)
            | premultiply(px[2], a);
        argb.extend_from_slice(&packed.to_ne_bytes());
    }
    Ok(CapturedImage {
        width,
        height,
        argb,
    })
}

fn window_size(display: *mut Display, window: Window) -> Result<(u32, u32), String> {
    let (mut root, mut x, mut y) = (0, 0 as c_int, 0 as c_int);
    let (mut width, mut height, mut border, mut depth) = (0 as c_uint, 0 as c_uint, 0 as c_uint, 0 as c_uint);
    // SAFETY: live display and valid out-pointers for the duration of the call.
    let status = unsafe {
        XGetGeometry(
            display,
            window,
            &mut root,
            &mut x,
            &mut y,
            &mut width,
            &mut height,
            &mut border,
            &mut depth,
        )
    };
    if status == 0 {
        return Err("XGetGeometry failed for capture".to_string());
    }
    Ok((width, height))
}

/// Captures the full contents of `window` as premultiplied ARGB32.
///
/// Uses one XGetImage on the window, so an obscured or unmapped window yields
/// undefined pixels; callers capture while the window is mapped and visible.
pub fn capture_window(display: *mut Display, window: Window) -> Result<CapturedImage, String> {
    if display.is_null() || window == 0 {
        return Err("capture_window: no display/window".to_string());
    }
    let (width, height) = window_size(display, window)?;
    check_capture_size(width, height)?;
    let BackdropCapture {
        width,
        height,
        pixels,
    } = capture_drawable_rect(display, window, (0, 0, width, height))?;
    rgba_to_argb(width, height, &pixels)
}

/// Uploads `capture` into a new depth-32 pixmap on the default screen.
/// The caller frees the returned pixmap with `XFreePixmap`.
///
/// # Safety
///
/// `display` must be a live Xlib display.
pub(crate) unsafe fn upload_argb_pixmap(
    display: *mut Display,
    capture: &CapturedImage,
) -> Result<Pixmap, String> {
    // SAFETY: forwarded contract.
    unsafe { upload_pixmap_depth(display, capture, 32) }
}

/// Uploads `capture` into a new pixmap of `depth` on the default screen (32 for ARGB, the screen
/// depth for opaque sources). Pixels are the same native 32-bit words either way.
///
/// # Safety
///
/// `display` must be a live Xlib display.
pub(crate) unsafe fn upload_pixmap_depth(
    display: *mut Display,
    capture: &CapturedImage,
    depth: c_uint,
) -> Result<Pixmap, String> {
    if display.is_null() {
        return Err("upload: null display".to_string());
    }
    check_capture_size(capture.width, capture.height)?;
    let len = capture.width as usize * capture.height as usize * 4;
    if capture.argb.len() != len {
        return Err("upload: ARGB buffer length mismatch".to_string());
    }
    let screen = unsafe { XDefaultScreen(display) };
    let root = unsafe { XRootWindow(display, screen) };
    let visual = unsafe { XDefaultVisual(display, screen) };
    let pixmap = unsafe { XCreatePixmap(display, root, capture.width, capture.height, depth) };
    if pixmap == 0 {
        return Err(format!("XCreatePixmap(depth {depth}) returned 0"));
    }
    let gc = unsafe { XCreateGC(display, pixmap, 0, std::ptr::null_mut()) };
    if gc.is_null() {
        unsafe { XFreePixmap(display, pixmap) };
        return Err("XCreateGC returned NULL".to_string());
    }
    // XDestroyImage frees the data with the Xlib allocator, so it must be malloc'd.
    let data = unsafe { malloc(len) } as *mut c_char;
    if data.is_null() {
        unsafe { XFreeGC(display, gc) };
        unsafe { XFreePixmap(display, pixmap) };
        return Err("upload: malloc failed".to_string());
    }
    unsafe { std::ptr::copy_nonoverlapping(capture.argb.as_ptr(), data as *mut u8, len) };
    let image = unsafe {
        XCreateImage(
            display,
            visual,
            depth,
            ZPIXMAP,
            0,
            data,
            capture.width,
            capture.height,
            32,
            (capture.width * 4) as c_int,
        )
    };
    if image.is_null() {
        unsafe { free(data as *mut c_void) };
        unsafe { XFreeGC(display, gc) };
        unsafe { XFreePixmap(display, pixmap) };
        return Err("XCreateImage returned NULL".to_string());
    }
    unsafe {
        XPutImage(
            display,
            pixmap,
            gc,
            image,
            0,
            0,
            0,
            0,
            capture.width,
            capture.height,
        )
    };
    unsafe { XDestroyImage(image) };
    unsafe { XFreeGC(display, gc) };
    Ok(pixmap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_cap_rejects_empty_and_oversize() {
        assert!(check_capture_size(0, 10).is_err());
        assert!(check_capture_size(10, 0).is_err());
        assert!(check_capture_size(3840, 2160).is_ok());
        assert!(check_capture_size(3841, 2160).is_err());
    }

    #[test]
    fn rgba_becomes_premultiplied_native_argb() {
        // Opaque red, then half-transparent green (alpha 0x80, premultiplied green 0x80).
        let rgba = [255, 0, 0, 255, 0, 255, 0, 128];
        let image = rgba_to_argb(2, 1, &rgba).expect("valid buffer");
        assert_eq!(image.argb.len(), 8);
        let first = u32::from_ne_bytes(image.argb[0..4].try_into().unwrap());
        let second = u32::from_ne_bytes(image.argb[4..8].try_into().unwrap());
        assert_eq!(first, 0xff_ff_00_00);
        assert_eq!(second, 0x80_00_80_00);
    }

    #[test]
    fn rgba_rejects_length_mismatch() {
        assert!(rgba_to_argb(2, 2, &[0; 4]).is_err());
    }

    #[test]
    fn capture_window_rejects_null_display_without_x() {
        assert!(capture_window(std::ptr::null_mut(), 1).is_err());
        assert!(capture_window(std::ptr::null_mut(), 0).is_err());
    }

    #[test]
    fn xcomposite_load_never_panics() {
        // Either outcome is valid depending on the host libraries.
        if let Ok(api) = XCompositeApi::load() {
            let version = unsafe { api.version(std::ptr::null_mut()) };
            assert!(version.is_err());
        }
    }
}
