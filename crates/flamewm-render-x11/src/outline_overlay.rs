//! Compositor-gated ARGB ring window: the window outline painted over the frame's final pixels.
//!
//! The frame child cannot paint over its client (the client is a child window of the frame), and an
//! X server without a compositing manager ignores a window's alpha channel. So the outline has two
//! hosts:
//!
//! * no compositing manager: [`crate::frame`]-side callers paint the ring into the frame's own
//!   pixels (`flamewm_wm_x11::frame::chrome::outline_blits`), which needs no blending;
//! * a compositing manager owns `_NET_WM_CM_S0` and a 32-bit TrueColor visual exists: this module
//!   creates one ARGB child of the frame, stacked above the client, shaped to the 1px ring annulus,
//!   with an empty input region. Its pixels are the shared `flamewm_render_core::outline` strips,
//!   so the compositor blends the ring over whatever the client actually drew.
//!
//! The shape is what keeps the fallback honest: a compositor that does not blend still shows a
//! ring (hard edges) rather than a black window, and the empty input region keeps the frame's
//! resize strips below reachable.

use std::os::raw::{c_int, c_ulong};
use std::ptr;

use crate::external_drawable::match_argb32_visual;
use crate::xlib::{
    ALLOC_NONE, Colormap, Display, GC, INPUT_OUTPUT_CLASS, Window, XCreateColormap, XCreateGC,
    XCreateImage, XCreateWindow, XDefaultScreen, XDestroyImage, XDestroyWindow, XFreeColormap,
    XFreeGC, XMapWindow, XMoveResizeWindow, XPutImage, XRootWindow, XSetWindowAttributes,
    XUnmapWindow, ZPIXMAP,
};
use crate::xshape::{XShapeBridge, rounded_mask_spans};

/// `CWBackPixel | CWBorderPixel | CWColormap | CWOverrideRedirect`: the attributes filled in below.
const CW_BACK_PIXEL: c_ulong = 1 << 1;
const CW_BORDER_PIXEL: c_ulong = 1 << 3;
const CW_COLORMAP: c_ulong = 1 << 13;
const CW_OVERRIDE_REDIRECT: c_ulong = 1 << 9;
const ARGB_DEPTH: i32 = 32;

unsafe extern "C" {
    /// `LSBFirst` when the server stores 32-bit ZPixmap pixels little-endian.
    fn XImageByteOrder(display: *mut Display) -> c_int;
}

/// `LSBFirst`: the byte order this server stores 32-bit ZPixmap pixels in.
const LSB_FIRST: c_int = 0;

/// Whether the X11 session can host the overlay at all: a compositing manager is running and the
/// screen offers a 32-bit TrueColor visual to put an alpha channel in.
#[must_use]
pub fn available(display: *mut Display) -> bool {
    crate::overlay::warp_argb_usable(display)
}

/// Row spans (`y`, `x_start`, `x_end_exclusive`) of a 1px ring around a `width` x `height` rounded
/// rect: the outer silhouette minus the inner rounded rect inset by one pixel.
#[must_use]
pub(crate) fn ring_rows(width: u32, height: u32, radius: u32) -> Vec<(u32, u32, u32)> {
    let outer = rounded_mask_spans(width, height, radius);
    if width < 3 || height < 3 {
        return outer;
    }
    let inner = rounded_mask_spans(width - 2, height - 2, radius.saturating_sub(1));
    let mut rows = Vec::with_capacity(outer.len() * 2);
    for (row, start, end) in outer {
        // The inner rect starts one row and one column in, so its spans shift by one.
        let cut = inner
            .get(row.saturating_sub(1) as usize)
            .filter(|_| row > 0 && row + 1 < height)
            .map(|&(_, inner_start, inner_end)| (inner_start + 1, inner_end + 1))
            .unwrap_or((end, end));
        for (from, to) in [(start, end.min(cut.0)), (start.max(cut.1), end)] {
            if to > from {
                rows.push((row, from, to));
            }
        }
    }
    rows
}

/// One ARGB ring window covering a frame.
pub struct OutlineOverlay {
    display: *mut Display,
    window: Window,
    colormap: Colormap,
    gc: GC,
    origin: (i32, i32),
    size: (u32, u32),
    radius: u32,
    lsb_first: bool,
}

impl OutlineOverlay {
    /// Creates and maps the ring window as a child of `parent` covering `size` at `origin`.
    ///
    /// # Safety
    ///
    /// `display` must be a live Xlib display and `parent` a live window on it.
    pub unsafe fn create(
        display: *mut Display,
        parent: Window,
        origin: (i32, i32),
        size: (u32, u32),
        radius: u32,
    ) -> Result<Self, String> {
        if display.is_null() {
            return Err("outline overlay requires a live display".to_string());
        }
        if parent == 0 {
            return Err("outline overlay requires a live parent window".to_string());
        }
        let (width, height) = (size.0.max(1), size.1.max(1));
        // SAFETY: the display is live per the caller contract; screen/visual come from it.
        let screen = unsafe { XDefaultScreen(display) };
        let visual = match_argb32_visual(display, screen)
            .ok_or("outline overlay requires a 32-bit TrueColor visual")?;
        // SAFETY: same screen as the parent window; ALLOC_NONE creates no colour cells.
        let root = unsafe { XRootWindow(display, screen) };
        let colormap = unsafe { XCreateColormap(display, root, visual, ALLOC_NONE) };
        if colormap == 0 {
            return Err("outline overlay: XCreateColormap returned 0".to_string());
        }
        let mut attributes: XSetWindowAttributes = unsafe { std::mem::zeroed() };
        attributes.background_pixel = 0;
        attributes.border_pixel = 0;
        attributes.colormap = colormap;
        // Override-redirect keeps the ring out of the WM's own MapRequest stream: the frame's
        // subtree is redirected, and a managed ring window would spawn a frame for itself.
        attributes.override_redirect = 1;
        // SAFETY: live display, a real visual and a fully initialised attribute struct.
        let window = unsafe {
            XCreateWindow(
                display,
                parent,
                origin.0,
                origin.1,
                width,
                height,
                0,
                ARGB_DEPTH,
                INPUT_OUTPUT_CLASS,
                visual,
                CW_BACK_PIXEL | CW_BORDER_PIXEL | CW_COLORMAP | CW_OVERRIDE_REDIRECT,
                &mut attributes,
            )
        };
        if window == 0 {
            unsafe { XFreeColormap(display, colormap) };
            return Err("outline overlay: XCreateWindow returned 0".to_string());
        }
        let overlay = Self {
            display,
            window,
            colormap,
            gc: unsafe { XCreateGC(display, window, 0, ptr::null_mut()) },
            origin,
            size: (width, height),
            radius,
            lsb_first: unsafe { XImageByteOrder(display) } == LSB_FIRST,
        };
        // A failed shape leaves the ring window opaque over the whole frame, so drop it instead.
        if overlay.shape_ring().is_err() {
            return Err("outline overlay: XShape ring mask unavailable".to_string());
        }
        unsafe { XMapWindow(display, window) };
        Ok(overlay)
    }

    /// The ring window's XID, used to raise/maintain it from the owning WM.
    #[must_use]
    pub fn window(&self) -> Window {
        self.window
    }

    /// Moves/resizes the ring to a new frame rect, reshaping only when the silhouette changed.
    ///
    /// The overlay owns a display pointer whose lifetime the creating renderer guarantees; a null
    /// display fails the call instead of touching X.
    pub fn configure(
        &mut self,
        origin: (i32, i32),
        size: (u32, u32),
        radius: u32,
    ) -> Result<(), String> {
        if self.display.is_null() {
            return Err("outline overlay has no display".to_string());
        }
        let (width, height) = (size.0.max(1), size.1.max(1));
        let geometry_changed = (width, height, radius) != (self.size.0, self.size.1, self.radius);
        if (width, height) != self.size || origin != self.origin {
            // SAFETY: the display is live (checked above) and both windows belong to it.
            unsafe {
                XMoveResizeWindow(self.display, self.window, origin.0, origin.1, width, height)
            };
        }
        self.origin = origin;
        self.size = (width, height);
        if geometry_changed {
            self.radius = radius;
            self.shape_ring()?;
        }
        Ok(())
    }

    /// Whether the overlay currently covers `origin`/`size` (the WM skips redundant moves).
    #[must_use]
    pub fn rect(&self) -> ((i32, i32), (u32, u32)) {
        (self.origin, self.size)
    }

    /// Writes the shared ring strips (`(x, y, width, height, straight RGBA)`) into the window.
    pub fn present(&self, strips: &[(u32, u32, u32, u32, Vec<u8>)]) -> Result<(), String> {
        if self.display.is_null() || self.window == 0 {
            return Err("outline overlay is not attached to a display".to_string());
        }
        for (x, y, width, height, rgba) in strips {
            let (w, h) = (*width, *height);
            if w == 0 || h == 0 {
                continue;
            }
            let mut bytes = premultiplied_pixels(rgba, self.lsb_first)?;
            let expected = w as usize * h as usize * 4;
            if bytes.len() != expected {
                return Err("outline overlay: strip length does not match its size".to_string());
            }
            // SAFETY: the image borrows `bytes` for the duration of `XPutImage` only; the pointer
            // is cleared before `XDestroyImage` so libX11 never frees the Rust buffer.
            unsafe {
                let image = XCreateImage(
                    self.display,
                    ptr::null_mut(),
                    ARGB_DEPTH as u32,
                    ZPIXMAP,
                    0,
                    bytes.as_mut_ptr().cast(),
                    w,
                    h,
                    32,
                    0,
                );
                if image.is_null() {
                    return Err("outline overlay: XCreateImage returned NULL".to_string());
                }
                XPutImage(
                    self.display,
                    self.window,
                    self.gc,
                    image,
                    0,
                    0,
                    i32::try_from(*x).unwrap_or(0),
                    i32::try_from(*y).unwrap_or(0),
                    w,
                    h,
                );
                // XDestroyImage frees the image's data pointer, which is this Rust buffer:
                // clear it first so libX11 never frees memory it does not own.
                (*(image.cast::<crate::xlib::XImageHead>())).data = ptr::null_mut();
                XDestroyImage(image);
            }
        }
        Ok(())
    }

    /// Hides the ring (an unmapped frame keeps no outline on screen).
    pub fn hide(&self) {
        if self.display.is_null() || self.window == 0 {
            return;
        }
        // SAFETY: the display/window pair is live for the overlay's lifetime.
        unsafe { XUnmapWindow(self.display, self.window) };
    }

    fn shape_ring(&self) -> Result<(), String> {
        // SAFETY: the display is live for the window's lifetime (see the module docs).
        let bridge = unsafe { XShapeBridge::new(self.display)? };
        let rows = ring_rows(self.size.0, self.size.1, self.radius);
        // SAFETY: `self.window` is a live window on this display.
        unsafe { bridge.apply_rounded_mask(self.window, &rows)? };
        // The ring band belongs to the frame's resize strips and the client, never to the overlay.
        unsafe { bridge.apply_input_mask(self.window, &[])? };
        Ok(())
    }
}

impl Drop for OutlineOverlay {
    fn drop(&mut self) {
        if self.display.is_null() || self.window == 0 {
            return;
        }
        // SAFETY: every resource was created on this display and is released once.
        unsafe {
            XDestroyWindow(self.display, self.window);
            if !self.gc.is_null() {
                XFreeGC(self.display, self.gc);
            }
            if self.colormap != 0 {
                XFreeColormap(self.display, self.colormap);
            }
        }
        self.window = 0;
    }
}

/// Premultiplied 32-bit pixels in the server's byte order, ready for `XPutImage`.
///
/// A compositing manager blends ARGB window contents as premultiplied alpha, so the straight-alpha
/// ring strips must be premultiplied here; the alpha byte itself is carried unchanged.
fn premultiplied_pixels(rgba: &[u8], lsb_first: bool) -> Result<Vec<u8>, String> {
    if rgba.chunks_exact(4).remainder().len() != 0 {
        return Err("outline overlay: pixel buffer is not RGBA8".to_string());
    }
    let mut out = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        let alpha = u32::from(px[3]);
        let scale = |channel: u8| ((u32::from(channel) * alpha + 127) / 255) as u8;
        let (r, g, b) = (scale(px[0]), scale(px[1]), scale(px[2]));
        if lsb_first {
            out.extend_from_slice(&[b, g, r, px[3]]);
        } else {
            out.extend_from_slice(&[px[3], r, g, b]);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn covered(rows: &[(u32, u32, u32)], x: u32, y: u32) -> bool {
        rows.iter()
            .any(|(row, start, end)| *row == y && x >= *start && x < *end)
    }

    #[test]
    fn ring_rows_keep_one_pixel_of_the_silhouette_and_nothing_inside() {
        let (w, h, r) = (120u32, 100u32, 12u32);
        let rows = ring_rows(w, h, r);
        assert!(!rows.is_empty());
        // Straight edges: one pixel in from each border.
        assert!(covered(&rows, 0, 50));
        assert!(covered(&rows, w - 1, 50));
        assert!(covered(&rows, 60, h - 1));
        assert!(!covered(&rows, 1, 50), "second column is interior");
        assert!(!covered(&rows, 60, h - 2), "second-last row is interior");
        // Corners: the outer silhouette row keeps only the arc, so the extreme corner pixel is
        // absent while the arc arm on that row is present.
        assert!(!covered(&rows, 0, 0), "outside the corner arc");
        assert!(covered(&rows, 11, 0), "arc arm on the top row");
        assert!(covered(&rows, 0, 11), "arc arm on the left column");
        // Interior is never part of the ring.
        assert!(!covered(&rows, w / 2, h / 2));
    }

    #[test]
    fn ring_is_one_pixel_thick_on_straight_edges_and_follows_the_arcs() {
        let (w, h, r) = (40u32, 30u32, 8u32);
        let rows = ring_rows(w, h, r);
        // Straight edges: exactly the outermost pixel, nothing one pixel further in.
        for y in r..h - r {
            assert!(covered(&rows, 0, y) && covered(&rows, w - 1, y), "row {y}");
            assert!(
                !covered(&rows, 1, y) && !covered(&rows, w - 2, y),
                "row {y} interior"
            );
        }
        for x in r..w - r {
            assert!(
                covered(&rows, x, 0) && covered(&rows, x, h - 1),
                "column {x}"
            );
            assert!(
                !covered(&rows, x, 1) && !covered(&rows, x, h - 2),
                "column {x} interior"
            );
        }
        // Corners: the extreme pixel is outside the arc, both arc arms on that row and column are
        // inside it, and the whole ring stays within the silhouette.
        assert!(!covered(&rows, 0, 0) && !covered(&rows, w - 1, 0));
        assert!(covered(&rows, r - 1, 0), "top row keeps the arc arm");
        assert!(covered(&rows, 0, r - 1), "left column keeps the arc arm");
        for (row, start, end) in &rows {
            assert!(start < end && *end <= w && *row < h);
        }
        // The left column survives the integer silhouette (rows whose inset is still 0) but never
        // the extreme top row.
        assert!(covered(&rows, 0, h / 2));
        assert!(!covered(&rows, 0, 0));
    }

    #[test]
    fn tiny_rectangles_fall_back_to_the_outer_silhouette() {
        assert_eq!(ring_rows(2, 2, 4).len(), 2);
        assert!(ring_rows(0, 0, 4).is_empty());
    }

    #[test]
    fn premultiplied_pixels_follow_the_alpha_and_the_server_byte_order() {
        let rgba = [255, 128, 0, 128, 10, 20, 30, 0];
        let little = premultiplied_pixels(&rgba, true).expect("valid rgba");
        // 128/255 of 255 = 128, of 128 = 64, of 0 = 0; byte order B, G, R, A.
        assert_eq!(&little[0..4], &[0, 64, 128, 128]);
        assert_eq!(
            &little[4..8],
            &[0, 0, 0, 0],
            "zero alpha premultiplies to 0"
        );
        let big = premultiplied_pixels(&rgba, false).expect("valid rgba");
        assert_eq!(&big[0..4], &[128, 128, 64, 0]);
        assert!(premultiplied_pixels(&[1, 2, 3], true).is_err());
    }
}
