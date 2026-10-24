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
#[cfg(test)]
use crate::ffi::dynamic_library::DynamicLibrary;
use crate::xlib::{
    Display, Pixmap, Window, XCreateGC, XCreateImage, XCreatePixmap, XDefaultScreen,
    XDefaultVisual, XDestroyImage, XFreeGC, XPutImage, XRootWindow, ZPIXMAP, malloc,
};
use crate::xlib_ext::{XGetGeometry, free};

/// Largest capture accepted (4K), bounding the copy and upload cost.
pub const MAX_CAPTURE_PIXELS: u64 = 3840 * 2160;
/// Bound transient XPutImage upload staging without changing retained captures.
/// At 4K, eight strips replace one additional 31.6 MiB full-frame copy.
const MAX_UPLOAD_STAGING_BYTES: usize = 4 * 1024 * 1024;

fn upload_strip_height(width: u32, height: u32) -> u32 {
    let bytes_per_row = width as usize * 4;
    (MAX_UPLOAD_STAGING_BYTES / bytes_per_row)
        .max(1)
        .min(height as usize) as u32
}

/// Premultiplied native-endian ARGB32 pixels, `width * height * 4` bytes, in the
/// layout XRender and XPutImage consume for depth 32.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedImage {
    pub width: u32,
    pub height: u32,
    pub argb: Vec<u8>,
}

#[cfg(test)]
type QueryExtensionFn = unsafe extern "C" fn(*mut Display, *mut c_int, *mut c_int) -> c_int;
#[cfg(test)]
type QueryVersionFn = unsafe extern "C" fn(*mut Display, *mut c_int, *mut c_int) -> c_int;

#[cfg(test)]
pub struct XCompositeApi {
    _library: DynamicLibrary,
    query_extension: QueryExtensionFn,
    query_version: QueryVersionFn,
}

#[cfg(test)]
impl XCompositeApi {
    pub fn load() -> Result<Self, String> {
        let library = DynamicLibrary::open(&["libXcomposite.so.1", "libXcomposite.so"])?;
        // SAFETY: names are static NUL-terminated; `T` matches the libXcomposite C ABI.
        let query_extension = unsafe { library.symbol(b"XCompositeQueryExtension\0") }?;
        // SAFETY: as above.
        let query_version = unsafe { library.symbol(b"XCompositeQueryVersion\0") }?;
        Ok(Self {
            _library: library,
            query_extension,
            query_version,
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
}

/// Rejects empty and oversize captures before any allocation.
pub fn check_capture_size(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("capture: empty size".to_string());
    }
    if u64::from(width) * u64::from(height) > MAX_CAPTURE_PIXELS {
        return Err(format!(
            "capture: {width}x{height} exceeds {MAX_CAPTURE_PIXELS} pixels"
        ));
    }
    Ok(())
}

fn premultiply(channel: u8, alpha: u8) -> u32 {
    (u32::from(channel) * u32::from(alpha) + 127) / 255
}

/// Converts straight RGBA8 (`BackdropCapture` layout) to premultiplied ARGB32.
/// Test oracle for the borrowed entry point; production callers use [`rgba_to_argb_owned`].
#[cfg(test)]
fn rgba_to_argb(width: u32, height: u32, rgba: &[u8]) -> Result<CapturedImage, String> {
    rgba_to_argb_owned(width, height, rgba.to_vec())
}

/// Same conversion as [`rgba_to_argb`], reusing the caller's buffer instead of allocating
/// a second one. Each pixel is rewritten from its own four bytes, so in-place is exact.
pub fn rgba_to_argb_owned(
    width: u32,
    height: u32,
    mut buf: Vec<u8>,
) -> Result<CapturedImage, String> {
    check_capture_size(width, height)?;
    let pixel_count = width as usize * height as usize;
    if buf.len() != pixel_count * 4 {
        return Err("capture: RGBA buffer length mismatch".to_string());
    }
    for px in buf.chunks_exact_mut(4) {
        let a = px[3];
        let packed = (u32::from(a) << 24)
            | (premultiply(px[0], a) << 16)
            | (premultiply(px[1], a) << 8)
            | premultiply(px[2], a);
        px.copy_from_slice(&packed.to_ne_bytes());
    }
    Ok(CapturedImage {
        width,
        height,
        argb: buf,
    })
}

fn window_size(display: *mut Display, window: Window) -> Result<(u32, u32), String> {
    let (mut root, mut x, mut y) = (0, 0 as c_int, 0 as c_int);
    let (mut width, mut height, mut border, mut depth) =
        (0 as c_uint, 0 as c_uint, 0 as c_uint, 0 as c_uint);
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
    match capture_drawable_rect(display, window, (0, 0, width, height)) {
        Ok(BackdropCapture {
            width,
            height,
            pixels,
        }) => rgba_to_argb_owned(width, height, pixels),
        // A window hanging past the screen edge (a maximized frame's border) is refused by the
        // server; capture what is on screen and pad the rest so the flight still has pixels.
        Err(first) => capture_clipped(display, window, width, height).ok_or(first),
    }
}

/// Visible part of a `w x h` window whose origin is at root `(ox, oy)` on an `sw x sh` screen,
/// as window-local `(x, y, width, height)`; `None` when nothing is on screen.
fn visible_subrect(
    ox: i32,
    oy: i32,
    w: u32,
    h: u32,
    sw: i32,
    sh: i32,
) -> Option<(i32, i32, u32, u32)> {
    let x0 = (-ox).max(0);
    let y0 = (-oy).max(0);
    let x1 = (sw - ox).min(w as i32);
    let y1 = (sh - oy).min(h as i32);
    (x1 > x0 && y1 > y0).then(|| (x0, y0, (x1 - x0) as u32, (y1 - y0) as u32))
}

fn capture_clipped(
    display: *mut Display,
    window: Window,
    width: u32,
    height: u32,
) -> Option<CapturedImage> {
    use crate::xlib::{XDisplayHeight, XDisplayWidth};
    use crate::xlib_ext::XTranslateCoordinates;
    let screen = unsafe { XDefaultScreen(display) };
    let root = unsafe { XRootWindow(display, screen) };
    let (mut ox, mut oy, mut child) = (0 as c_int, 0 as c_int, 0 as Window);
    // SAFETY: display/window/root are live; the out-pointers are valid locals.
    let ok =
        unsafe { XTranslateCoordinates(display, window, root, 0, 0, &mut ox, &mut oy, &mut child) };
    if ok == 0 {
        return None;
    }
    let (sw, sh) = unsafe {
        (
            XDisplayWidth(display, screen),
            XDisplayHeight(display, screen),
        )
    };
    let (vx, vy, vw, vh) = visible_subrect(ox, oy, width, height, sw, sh)?;
    let BackdropCapture {
        width: cw,
        height: ch,
        pixels,
    } = capture_drawable_rect(display, window, (vx, vy, vw, vh)).ok()?;
    embed_subrect_argb(width, height, (vx as u32, vy as u32), (cw, ch), &pixels)
}

/// Places a `cw x ch` straight RGBA capture at `(vx, vy)` inside a `width x height`
/// premultiplied ARGB frame. Only the captured sub-rect is converted; the rest is zero,
/// which is transparent in premultiplied form. No intermediate full-frame RGBA buffer.
fn embed_subrect_argb(
    width: u32,
    height: u32,
    (vx, vy): (u32, u32),
    (cw, ch): (u32, u32),
    pixels: &[u8],
) -> Option<CapturedImage> {
    check_capture_size(width, height).ok()?;
    let row_bytes = cw as usize * 4;
    if pixels.len() != row_bytes * ch as usize {
        return None;
    }
    let mut argb = vec![0u8; width as usize * height as usize * 4];
    for (row, src) in pixels.chunks_exact(row_bytes).enumerate() {
        let dst_at = ((vy as usize + row) * width as usize + vx as usize) * 4;
        let dst = argb.get_mut(dst_at..dst_at + row_bytes)?;
        for (out, px) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            let a = px[3];
            let packed = (u32::from(a) << 24)
                | (premultiply(px[0], a) << 16)
                | (premultiply(px[1], a) << 8)
                | premultiply(px[2], a);
            out.copy_from_slice(&packed.to_ne_bytes());
        }
    }
    Some(CapturedImage {
        width,
        height,
        argb,
    })
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
        unsafe { crate::native::xresource::free_pixmap_raw(display, pixmap) };
        return Err("XCreateGC returned NULL".to_string());
    }
    // XDestroyImage frees XImage.data, so the reusable staging buffer must
    // use the Xlib allocator. Keep it to a few MiB instead of duplicating the
    // whole capture. XPutImage serializes each strip before we reuse its data.
    let row_bytes = capture.width as usize * 4;
    let strip_height = upload_strip_height(capture.width, capture.height);
    let strip_bytes = row_bytes * strip_height as usize;
    let data = unsafe { malloc(strip_bytes) } as *mut c_char;
    if data.is_null() {
        unsafe { XFreeGC(display, gc) };
        unsafe { crate::native::xresource::free_pixmap_raw(display, pixmap) };
        return Err("upload: malloc failed".to_string());
    }
    let image = unsafe {
        XCreateImage(
            display,
            visual,
            depth,
            ZPIXMAP,
            0,
            data,
            capture.width,
            strip_height,
            32,
            row_bytes as c_int,
        )
    };
    if image.is_null() {
        unsafe { free(data as *mut c_void) };
        unsafe { XFreeGC(display, gc) };
        unsafe { crate::native::xresource::free_pixmap_raw(display, pixmap) };
        return Err("XCreateImage returned NULL".to_string());
    }
    let mut y = 0;
    while y < capture.height {
        let rows = strip_height.min(capture.height - y);
        let start = y as usize * row_bytes;
        let bytes = rows as usize * row_bytes;
        unsafe {
            std::ptr::copy_nonoverlapping(
                capture.argb[start..start + bytes].as_ptr(),
                data.cast(),
                bytes,
            );
            XPutImage(
                display,
                pixmap,
                gc,
                image,
                0,
                0,
                0,
                y as c_int,
                capture.width,
                rows,
            );
        }
        y += rows;
    }
    unsafe { XDestroyImage(image) };
    unsafe { XFreeGC(display, gc) };
    Ok(pixmap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_staging_strips_cover_capture_without_truncating_last_row() {
        assert_eq!(upload_strip_height(3840, 2160), 273);
        assert_eq!(upload_strip_height(1, 1), 1);
        assert_eq!(upload_strip_height(8_294_400, 1), 1);
        let (width, height) = (17u32, 19u32);
        let stride = width as usize * 4;
        let pixels: Vec<u8> = (0..stride * height as usize)
            .map(|i| (i % 251) as u8)
            .collect();
        let mut uploaded = Vec::with_capacity(pixels.len());
        let mut y = 0;
        let rows_per_strip = upload_strip_height(width, height);
        while y < height {
            let rows = rows_per_strip.min(height - y);
            let start = y as usize * stride;
            uploaded.extend_from_slice(&pixels[start..start + rows as usize * stride]);
            y += rows;
        }
        assert_eq!(uploaded, pixels);
        // The final shorter strip must not read beyond its source.
        assert_eq!(upload_strip_height(1_048_576, 3), 1);
        assert_eq!(upload_strip_height(524_288, 5), 2);
        let mut positions = Vec::new();
        let mut start_row = 0;
        while start_row < 5 {
            let rows = upload_strip_height(524_288, 5).min(5 - start_row);
            positions.push((start_row, rows));
            start_row += rows;
        }
        assert_eq!(positions, [(0, 2), (2, 2), (4, 1)]);
    }

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
    fn embed_subrect_full_frame_matches_full_convert() {
        let (width, height) = (5u32, 3u32);
        let frame: Vec<u8> = (0..(width * height * 4))
            .map(|i| (i * 7 % 251) as u8)
            .collect();
        let fast = embed_subrect_argb(width, height, (0, 0), (width, height), &frame)
            .expect("full frame");
        assert_eq!(fast.argb, rgba_to_argb(width, height, &frame).unwrap().argb);
    }

    #[test]
    fn embed_subrect_partial_matches_zeroed_frame_convert() {
        // 2x2 capture placed at (1, 1) inside a 4x3 frame; the rest stays transparent.
        let capture = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let mut zeroed = vec![0u8; 4 * 3 * 4];
        for row in 0..2usize {
            let dst = ((1 + row) * 4 + 1) * 4;
            zeroed[dst..dst + 8].copy_from_slice(&capture[row * 8..row * 8 + 8]);
        }
        let out = embed_subrect_argb(4, 3, (1, 1), (2, 2), &capture).expect("in range");
        assert_eq!(out.argb, rgba_to_argb(4, 3, &zeroed).unwrap().argb);
        assert!(embed_subrect_argb(4, 3, (1, 1), (2, 2), &[0; 4]).is_none());
    }

    #[test]
    fn rgba_rejects_length_mismatch() {
        assert!(rgba_to_argb(2, 2, &[0; 4]).is_err());
        assert!(rgba_to_argb_owned(2, 2, vec![0; 4]).is_err());
    }

    /// The pre-optimization conversion, kept as the bit-exact oracle.
    fn reference_rgba_to_argb(rgba: &[u8]) -> Vec<u8> {
        let mut argb = Vec::with_capacity(rgba.len());
        for px in rgba.chunks_exact(4) {
            let a = px[3];
            let packed = (u32::from(a) << 24)
                | (premultiply(px[0], a) << 16)
                | (premultiply(px[1], a) << 8)
                | premultiply(px[2], a);
            argb.extend_from_slice(&packed.to_ne_bytes());
        }
        argb
    }

    #[test]
    fn owned_conversion_is_bit_identical_to_reference() {
        const EDGES: [u8; 8] = [0, 1, 127, 128, 254, 255, 2, 253];
        // Deterministic LCG so the random coverage is reproducible without a rand dependency.
        let mut state: u32 = 0x1234_5678;
        let mut next = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8
        };
        let mut rgba = Vec::new();
        for i in 0..(64 * 64) {
            for c in 0..4 {
                let v = match c {
                    3 => {
                        if i % 3 == 0 {
                            EDGES[i % EDGES.len()]
                        } else {
                            next()
                        }
                    }
                    _ => {
                        if i % 5 == 0 {
                            EDGES[(i + c) % EDGES.len()]
                        } else {
                            next()
                        }
                    }
                };
                rgba.push(v);
            }
        }
        // Every (channel, alpha) pair over the edge set, exhaustively.
        for &a in &EDGES {
            for &ch in &EDGES {
                rgba.extend_from_slice(&[ch, 255 - ch, ch / 2, a]);
            }
        }
        let expected = reference_rgba_to_argb(&rgba);
        let copied = rgba_to_argb(rgba.len() as u32 / 4, 1, &rgba).expect("valid buffer");
        assert_eq!(copied.argb, expected);
        let owned =
            rgba_to_argb_owned(rgba.len() as u32 / 4, 1, rgba.clone()).expect("valid buffer");
        assert_eq!(owned.argb, expected);
    }

    #[test]
    fn capture_window_rejects_null_display_without_x() {
        assert!(capture_window(std::ptr::null_mut(), 1).is_err());
        assert!(capture_window(std::ptr::null_mut(), 0).is_err());
    }

    #[test]
    fn visible_subrect_clips_to_the_screen_and_rejects_offscreen() {
        // Maximized 1280x756 frame interior at root (1,1) on a 1280x800 screen: one column short.
        assert_eq!(
            visible_subrect(1, 1, 1280, 756, 1280, 800),
            Some((0, 0, 1279, 756))
        );
        assert_eq!(
            visible_subrect(-4, -2, 100, 50, 1280, 800),
            Some((4, 2, 96, 48))
        );
        assert_eq!(
            visible_subrect(0, 0, 10, 10, 1280, 800),
            Some((0, 0, 10, 10))
        );
        assert_eq!(visible_subrect(1280, 0, 10, 10, 1280, 800), None);
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
