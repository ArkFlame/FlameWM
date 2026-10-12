//! Transition overlay: one override-redirect window that shows a captured window.
//!
//! Three surfaces share the type:
//! - Magic Lamp warp on a 32-bit ARGB window ([`WarpOverlay::begin`] with an ARGB visual).
//! - Magic Lamp warp on the default visual when no ARGB visual exists. The window is opaque
//!   and first paints a root snapshot of the area beneath it, then the warped strips. The
//!   caller must hide the animated frame before `begin` so the snapshot shows the backdrop.
//!   Exposed areas outside the snapshot rect are not refreshed; they are accepted as stale.
//! - Window-open scale-and-fade on ARGB only ([`WarpOverlay::begin_fade`]). It never fakes a
//!   fade on an opaque visual.
//!
//! The capture is taken once by the caller and owned here. Raw Xlib stays inside render-x11;
//! the WM sees only [`WarpOverlay`]. The overlay must be dropped before the display it was
//! opened on is closed (the owning [`crate::ExternalDecorationRenderer`] outlives it).

use std::mem::MaybeUninit;
use std::os::raw::c_ulong;

use crate::backdrop::capture_drawable_rect;
use crate::warp::{self, WarpRect, WarpSource};
use crate::xcomposite::{CapturedImage, check_capture_size, rgba_to_argb, upload_argb_pixmap};
use crate::xlib::{
    CW_BACK_PIXEL, CW_COLORMAP, CW_OVERRIDE_REDIRECT, Colormap, Display, INPUT_OUTPUT_CLASS,
    Pixmap, TRANSPARENT_CLEAR_PIXEL, TRUE_COLOR_CLASS, Window, XCreateColormap, XCreateWindow,
    XDefaultDepth, XDefaultScreen, XDefaultVisual, XDestroyWindow, XDisplayHeight, XDisplayWidth,
    XCreatePixmap, XFlush, XFreeColormap, XFreePixmap, XMapWindow, XMatchVisualInfo, XRootWindow,
    XSetWindowAttributes, XSync, XVisualInfo, ALLOC_NONE, XInternAtom,
};
use crate::xrender_transform::{PICT_STANDARD_RGB_24, XRenderTransformApi};

/// Strips per frame: enough for a smooth genie curve at 60 Hz.
const STRIP_COUNT: usize = 32;
/// `CWBorderPixel` value bit (not exported by `xlib`).
const CW_BORDER_PIXEL: c_ulong = 1 << 3;
const ARGB_DEPTH: i32 = 32;
/// Window-open effect scales from this fraction of the window up to full size.
const FADE_START_SCALE: f64 = 0.85;

/// Which surface a Magic Lamp transition uses for the given visual support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarpSurface {
    /// Transparent 32-bit window; the real window beneath stays visible around the warp.
    Argb,
    /// Opaque default-visual window painted over a root snapshot.
    OpaqueBackdrop,
}

/// The surface used for a Magic Lamp transition. A transparent window is only shown through
/// when a compositing manager blends it; without one its transparent pixels display as black.
#[must_use]
pub fn warp_surface_for(argb_visual: bool, compositing_manager: bool) -> WarpSurface {
    if argb_visual && compositing_manager {
        WarpSurface::Argb
    } else {
        WarpSurface::OpaqueBackdrop
    }
}

#[link(name = "X11")]
unsafe extern "C" {
    fn XGetSelectionOwner(display: *mut Display, selection: c_ulong) -> c_ulong;
}

/// True when some client owns `_NET_WM_CM_S0`, the compositing-manager selection.
#[must_use]
pub fn compositing_manager_running(display: *mut Display) -> bool {
    if display.is_null() {
        return false;
    }
    // SAFETY: the display is live per the caller contract; the name is a static NUL-terminated
    // string and `only_if_exists` keeps the atom lookup from creating it.
    unsafe {
        let atom = XInternAtom(display, b"_NET_WM_CM_S0\0".as_ptr().cast(), 1);
        atom != 0 && XGetSelectionOwner(display, atom) != 0
    }
}

/// True when Magic Lamp may draw on a transparent window: a 32-bit visual and a compositing
/// manager. The WM hides the real frame only when this is false, so the snapshot stays visible.
#[must_use]
pub fn warp_argb_usable(display: *mut Display) -> bool {
    argb_visual_available(display) && compositing_manager_running(display)
}

/// Scale and alpha of the window-open effect at linear progress `t` (clamped).
/// Ease-out quadratic: both reach their end values exactly at `t = 1`.
#[must_use]
pub fn fade_frame(t: f64) -> (f64, f64) {
    let t = t.clamp(0.0, 1.0);
    let eased = 1.0 - (1.0 - t) * (1.0 - t);
    (FADE_START_SCALE + (1.0 - FADE_START_SCALE) * eased, eased)
}

/// True when the display has a 32-bit TrueColor visual. A null display has none.
#[must_use]
pub fn argb_visual_available(display: *mut Display) -> bool {
    if display.is_null() {
        return false;
    }
    let mut info = MaybeUninit::<XVisualInfo>::zeroed();
    // SAFETY: the display is live per the caller contract; `info` is a writable struct and
    // a zero return only means no matching visual.
    unsafe {
        XMatchVisualInfo(
            display,
            XDefaultScreen(display),
            ARGB_DEPTH,
            TRUE_COLOR_CLASS,
            info.as_mut_ptr(),
        ) != 0
    }
}

/// Root snapshot pixmap and its picture, both released by the owning overlay.
struct Backdrop {
    pixmap: Pixmap,
    picture: c_ulong,
}

/// A live transition window. Releases every X resource it created on drop.
pub struct WarpOverlay {
    display: *mut Display,
    api: XRenderTransformApi,
    capture: CapturedImage,
    /// Window size in overlay-local pixels.
    size: (u32, u32),
    /// Zero until the window is created; checked on drop so a partial `begin` cleans up.
    window: Window,
    colormap: Option<Colormap>,
    /// Destination picture on `window`. Zero until created.
    picture: c_ulong,
    backdrop: Option<Backdrop>,
    /// Opaque warp frame: backdrop and strips are drawn here, then copied to the window once per
    /// frame so the window never shows the intermediate black backdrop.
    offscreen: Option<Backdrop>,
    /// Window-open effect resources. `fade` selects the paint path.
    fade: bool,
    fade_pixmap: Pixmap,
    fade_source: c_ulong,
    fade_clear: c_ulong,
    /// Warp capture uploaded once on the first warp frame and reused until Drop.
    warp_source: Option<WarpSource>,
    /// Source and target in overlay-local coordinates.
    source: WarpRect,
    target: WarpRect,
    mapped: bool,
}

/// Smallest integer rect covering both `source` and `target` in root coordinates.
fn union_bounds(source: WarpRect, target: WarpRect) -> (i32, i32, u32, u32) {
    let x0 = source.x.min(target.x).floor();
    let y0 = source.y.min(target.y).floor();
    let x1 = (source.x + source.width).max(target.x + target.width).ceil();
    let y1 = (source.y + source.height).max(target.y + target.height).ceil();
    (x0 as i32, y0 as i32, (x1 - x0) as u32, (y1 - y0) as u32)
}

impl WarpOverlay {
    fn empty(
        display: *mut Display,
        api: XRenderTransformApi,
        capture: CapturedImage,
        size: (u32, u32),
        source: WarpRect,
        target: WarpRect,
    ) -> Self {
        Self {
            display,
            api,
            capture,
            size,
            window: 0,
            colormap: None,
            picture: 0,
            backdrop: None,
            offscreen: None,
            fade: false,
            fade_pixmap: 0,
            fade_source: 0,
            fade_clear: 0,
            source,
            target,
            warp_source: None,
            mapped: false,
        }
    }

    /// Opens a Magic Lamp warp covering both rects. `capture` must match `source`'s size.
    ///
    /// Uses an ARGB window when available. Otherwise it takes a root snapshot of the union
    /// rect now, so the caller must hide the animated frame before calling.
    /// Fails without leaving X resources behind when the transform extension, the capture
    /// size, or the snapshot is unavailable, so the caller skips the effect.
    pub fn begin(
        display: *mut Display,
        capture: CapturedImage,
        source: WarpRect,
        target: WarpRect,
    ) -> Result<Self, String> {
        let api = load_checked_api(display)?;
        if f64::from(capture.width) != source.width || f64::from(capture.height) != source.height {
            return Err("overlay: capture size does not match source rect".to_string());
        }
        let (bx, by, bw, bh) = union_bounds(source, target);
        let origin = WarpRect { x: f64::from(bx), y: f64::from(by), width: f64::from(bw), height: f64::from(bh) };
        let local = |r: WarpRect| WarpRect { x: r.x - origin.x, y: r.y - origin.y, ..r };
        let mut overlay = Self::empty(display, api, capture, (bw, bh), local(source), local(target));

        match warp_surface_for(argb_visual_available(display), compositing_manager_running(display)) {
            WarpSurface::Argb => {
                // SAFETY: `display` is live per the caller contract; resources are recorded
                // in `overlay` as soon as they exist, so Drop releases them on error.
                let (window, colormap) = unsafe { create_argb_window(display, bx, by, bw, bh)? };
                overlay.window = window;
                overlay.colormap = Some(colormap);
                // SAFETY: `window` is live and owned by `overlay`.
                overlay.picture = unsafe { overlay.api.create_argb_picture(display, window) }?;
            }
            WarpSurface::OpaqueBackdrop => {
                // SAFETY: `display` is live; the snapshot picture is owned by `overlay`.
                overlay.backdrop = Some(unsafe { capture_backdrop(display, &overlay.api, bx, by, bw, bh)? });
                // SAFETY: `display` is live; the window is recorded for Drop.
                let window = unsafe { create_default_window(display, bx, by, bw, bh)? };
                overlay.window = window;
                // SAFETY: `window` is live and owned by `overlay`.
                overlay.picture = unsafe {
                    overlay.api.create_standard_picture(display, window, PICT_STANDARD_RGB_24)
                }?;
                // SAFETY: `display` and `window` are live; the pixmap is recorded for Drop once created.
                let pixmap = unsafe {
                    XCreatePixmap(display, window, bw, bh, XDefaultDepth(display, XDefaultScreen(display)) as u32)
                };
                if pixmap == 0 {
                    return Err("overlay: offscreen pixmap returned 0".to_string());
                }
                overlay.offscreen = Some(Backdrop { pixmap, picture: 0 });
                // SAFETY: `pixmap` is live and owned by `overlay`.
                let picture = unsafe { overlay.api.create_standard_picture(display, pixmap, PICT_STANDARD_RGB_24) }?;
                if let Some(off) = overlay.offscreen.as_mut() {
                    off.picture = picture;
                }
            }
        }
        Ok(overlay)
    }

    /// Opens a window-open scale-and-fade on an ARGB window at root position `(x, y)`.
    /// The window is exactly the capture's size. Errors without an ARGB visual, so the effect
    /// is never drawn as a fade on an opaque surface.
    pub fn begin_fade(display: *mut Display, capture: CapturedImage, x: i32, y: i32) -> Result<Self, String> {
        let api = load_checked_api(display)?;
        if !argb_visual_available(display) {
            return Err("overlay: fade needs a 32-bit ARGB visual".to_string());
        }
        check_capture_size(capture.width, capture.height)?;
        let (w, h) = (capture.width, capture.height);
        let full = WarpRect { x: 0.0, y: 0.0, width: f64::from(w), height: f64::from(h) };
        let mut overlay = Self::empty(display, api, capture, (w, h), full, full);
        overlay.fade = true;
        // SAFETY: `display` is live per the caller contract; resources are recorded as created.
        let (window, colormap) = unsafe { create_argb_window(display, x, y, w, h)? };
        overlay.window = window;
        overlay.colormap = Some(colormap);
        // SAFETY: `window` is live and owned by `overlay`.
        overlay.picture = unsafe { overlay.api.create_argb_picture(display, window) }?;
        // SAFETY: `display` is live; the pixmap is recorded for Drop before any further use.
        overlay.fade_pixmap = unsafe { upload_argb_pixmap(display, &overlay.capture) }?;
        // SAFETY: `fade_pixmap` is live and owned by `overlay`.
        overlay.fade_source = unsafe { overlay.api.create_argb_picture(display, overlay.fade_pixmap) }?;
        if overlay.api.capabilities().picture_filter {
            // SAFETY: `fade_source` is live and owned by `overlay`.
            unsafe { overlay.api.set_bilinear_filter(display, overlay.fade_source) }?;
        }
        // SAFETY: `display` is live; the solid transparent picture is owned by `overlay`.
        overlay.fade_clear = unsafe { overlay.api.create_solid_fill(display, 0, 0, 0, 0) }?;
        Ok(overlay)
    }

    /// Draws the effect at `progress` and maps the window on first call.
    ///
    /// For a warp, `progress` 0 is the source and 1 the target. For a fade it is linear time
    /// 0..=1, eased by [`fade_frame`].
    pub fn paint(&mut self, progress: f64) -> Result<(), String> {
        // Map before the first draw: without backing store, pixels drawn into an unmapped window
        // are discarded, and a picture created before mapping does not clip to the visible window.
        // SAFETY: `display` is live; the window is ours until Drop.
        unsafe {
            if !self.mapped {
                XMapWindow(self.display, self.window);
                self.mapped = true;
            }
        }
        if self.fade {
            self.paint_fade(progress)?;
        } else {
            self.paint_warp(progress)?;
        }
        // SAFETY: `display` is live; the window is ours until Drop.
        unsafe { XFlush(self.display) };
        Ok(())
    }

    fn paint_warp(&mut self, progress: f64) -> Result<(), String> {
        let mesh = warp::magic_lamp_mesh(self.source, self.target, progress, STRIP_COUNT)?;
        let (w, h) = self.size;
        let dest = self.offscreen.as_ref().map_or(self.picture, |off| off.picture);
        if let Some(backdrop) = &self.backdrop {
            // SAFETY: both pictures are live on `display` until Drop; the snapshot covers the
            // whole window, so every pixel is opaque before the strips are drawn.
            unsafe {
                self.api.composite_src(self.display, backdrop.picture, dest, (0, 0, w, h));
            }
        }
        if self.warp_source.is_none() {
            // SAFETY: `display` is live until Drop; the capture never changes after `begin`, so
            // one upload serves every frame of this warp.
            self.warp_source = Some(unsafe { warp::upload_warp_source(self.display, &self.api, &self.capture) }?);
        }
        let Some(source) = self.warp_source.as_ref() else {
            return Err("overlay: warp source missing after upload".to_string());
        };
        // SAFETY: `display` and `dest` are live until Drop; `source` was uploaded on `display`.
        unsafe { warp::paint_warped(self.display, &self.api, source, &mesh, dest) }?;
        if dest != self.picture {
            // SAFETY: both pictures are live on `display`; one copy shows the finished frame.
            unsafe { self.api.composite_src(self.display, dest, self.picture, (0, 0, w, h)) };
        }
        Ok(())
    }

    fn paint_fade(&mut self, progress: f64) -> Result<(), String> {
        let (scale, alpha) = fade_frame(progress);
        let (w, h) = (f64::from(self.size.0), f64::from(self.size.1));
        let (cx, cy) = (w / 2.0, h / 2.0);
        // Destination of the scaled capture, centred in the window.
        let x0 = (cx - cx * scale).floor();
        let y0 = (cy - cy * scale).floor();
        let x1 = (cx + cx * scale).ceil();
        let y1 = (cy + cy * scale).ceil();
        let rect = (x0 as i32, y0 as i32, (x1 - x0) as u32, (y1 - y0) as u32);
        // Destination-to-source map: scale about the centre (inverse of the zoom).
        let inv = 1.0 / scale;
        let matrix = [[inv, 0.0, cx * (1.0 - inv)], [0.0, inv, cy * (1.0 - inv)], [0.0, 0.0, 1.0]];
        let (window_w, window_h) = self.size;
        let display = self.display;
        // SAFETY: all pictures are live on `display` until Drop; the mask is freed below.
        unsafe {
            self.api
                .composite_src(display, self.fade_clear, self.picture, (0, 0, window_w, window_h));
            if alpha > 0.0 {
                self.api.set_transform(display, self.fade_source, matrix)?;
                let mask_alpha = (alpha.clamp(0.0, 1.0) * f64::from(u16::MAX)).round() as u16;
                let mask = self.api.create_solid_fill(display, 0, 0, 0, mask_alpha)?;
                self.api
                    .composite_over_masked(display, self.fade_source, mask, self.picture, rect);
                self.api.free_picture(display, mask);
            }
        }
        Ok(())
    }

    /// Returns the capture for later reuse (for example a restore).
    #[must_use]
    pub fn finish(mut self) -> CapturedImage {
        std::mem::replace(
            &mut self.capture,
            CapturedImage {
                width: 0,
                height: 0,
                argb: Vec::new(),
            },
        )
    }
}

impl Drop for WarpOverlay {
    fn drop(&mut self) {
        // SAFETY: every id below was created by `begin`/`begin_fade` on `display` and is
        // released once; a zero id was never created and is skipped.
        unsafe {
            if let Some(source) = self.warp_source.take() {
                warp::free_warp_source(self.display, &self.api, source);
            }
            if self.fade {
                if self.fade_source != 0 {
                    self.api.free_picture(self.display, self.fade_source);
                }
                if self.fade_clear != 0 {
                    self.api.free_picture(self.display, self.fade_clear);
                }
                if self.fade_pixmap != 0 {
                    XFreePixmap(self.display, self.fade_pixmap);
                }
            }
            if self.picture != 0 {
                self.api.free_picture(self.display, self.picture);
            }
            if let Some(backdrop) = self.backdrop.take() {
                self.api.free_picture(self.display, backdrop.picture);
                XFreePixmap(self.display, backdrop.pixmap);
            }
            if let Some(off) = self.offscreen.take() {
                if off.picture != 0 {
                    self.api.free_picture(self.display, off.picture);
                }
                XFreePixmap(self.display, off.pixmap);
            }
            if self.window != 0 {
                XDestroyWindow(self.display, self.window);
            }
            if let Some(colormap) = self.colormap {
                XFreeColormap(self.display, colormap);
            }
            if !self.display.is_null() {
                XFlush(self.display);
            }
        }
    }
}

/// Checks the display and the XRender transform capability before any X resource exists.
fn load_checked_api(display: *mut Display) -> Result<XRenderTransformApi, String> {
    if display.is_null() {
        return Err("overlay: no display".to_string());
    }
    let api = XRenderTransformApi::load()?;
    if !api.capabilities().picture_transform {
        return Err("overlay: XRenderSetPictureTransform unavailable".to_string());
    }
    Ok(api)
}

/// Root snapshot of `(x, y, width, height)` as an ARGB pixmap and picture. Syncs first so the
/// frame hidden by the caller is no longer on screen. Rects that leave the screen are rejected,
/// because XGetImage on them would raise an X error.
///
/// # Safety
///
/// `display` must be live. The returned resources are owned by the caller.
unsafe fn capture_backdrop(
    display: *mut Display,
    api: &XRenderTransformApi,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<Backdrop, String> {
    // SAFETY: live display; the screen index is the default screen.
    let (root, screen_w, screen_h) = unsafe {
        let screen = XDefaultScreen(display);
        (
            u64::from(XRootWindow(display, screen)),
            XDisplayWidth(display, screen),
            XDisplayHeight(display, screen),
        )
    };
    let (x_end, y_end) = (i64::from(x) + i64::from(width), i64::from(y) + i64::from(height));
    if x < 0 || y < 0 || x_end > i64::from(screen_w) || y_end > i64::from(screen_h) {
        return Err("overlay: backdrop rect leaves the screen".to_string());
    }
    // SAFETY: live display; a zero-status sync only flushes and waits for the server.
    unsafe { XSync(display, 0) };
    let shot = capture_drawable_rect(display, root, (x, y, width, height))?;
    let captured = rgba_to_argb(shot.width, shot.height, &shot.pixels)?;
    // SAFETY: live display; the pixmap is freed below if the picture cannot be made.
    let pixmap = unsafe { upload_argb_pixmap(display, &captured) }?;
    // SAFETY: `pixmap` is live.
    match unsafe { api.create_argb_picture(display, pixmap) } {
        Ok(picture) => Ok(Backdrop { pixmap, picture }),
        Err(error) => {
            // SAFETY: `pixmap` was created above and is not used again.
            unsafe { XFreePixmap(display, pixmap) };
            Err(error)
        }
    }
}

/// # Safety
///
/// `display` must be live.
unsafe fn create_argb_window(
    display: *mut Display,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<(Window, Colormap), String> {
    // SAFETY: live display; screen index is valid for the default screen.
    let screen = unsafe { XDefaultScreen(display) };
    let mut info = MaybeUninit::<XVisualInfo>::zeroed();
    // SAFETY: `info` is a writable XVisualInfo; a zero return means no such visual.
    let found = unsafe { XMatchVisualInfo(display, screen, ARGB_DEPTH, TRUE_COLOR_CLASS, info.as_mut_ptr()) };
    if found == 0 {
        return Err("overlay: no 32-bit TrueColor visual".to_string());
    }
    // SAFETY: XMatchVisualInfo succeeded, so `info` is initialized.
    let info = unsafe { info.assume_init() };
    // SAFETY: live display; root and visual come from the same screen.
    let root = unsafe { XRootWindow(display, screen) };
    let colormap = unsafe { XCreateColormap(display, root, info.visual, ALLOC_NONE) };
    let mut attributes = MaybeUninit::<XSetWindowAttributes>::zeroed();
    // SAFETY: the zeroed struct is a valid XSetWindowAttributes; fields set below are the masked ones.
    let attributes_ptr = attributes.as_mut_ptr();
    unsafe {
        (*attributes_ptr).background_pixel = TRANSPARENT_CLEAR_PIXEL;
        (*attributes_ptr).border_pixel = 0;
        (*attributes_ptr).colormap = colormap;
        (*attributes_ptr).override_redirect = 1;
    }
    let mask = CW_BACK_PIXEL | CW_BORDER_PIXEL | CW_COLORMAP | CW_OVERRIDE_REDIRECT;
    // SAFETY: live display, a visual from XMatchVisualInfo, and a fully initialized attribute struct.
    let window = unsafe {
        XCreateWindow(
            display,
            root,
            x,
            y,
            width,
            height,
            0,
            ARGB_DEPTH,
            INPUT_OUTPUT_CLASS,
            info.visual,
            mask,
            attributes_ptr,
        )
    };
    if window == 0 {
        // SAFETY: colormap was created above and is not used again.
        unsafe { XFreeColormap(display, colormap) };
        return Err("overlay: XCreateWindow returned 0".to_string());
    }
    Ok((window, colormap))
}

/// Opaque override-redirect window on the default visual and depth (no colormap of its own).
///
/// # Safety
///
/// `display` must be live.
unsafe fn create_default_window(
    display: *mut Display,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<Window, String> {
    // SAFETY: live display; default screen queries have no preconditions beyond that.
    let (root, visual, depth) = unsafe {
        let screen = XDefaultScreen(display);
        (
            XRootWindow(display, screen),
            XDefaultVisual(display, screen),
            XDefaultDepth(display, screen),
        )
    };
    if visual.is_null() {
        return Err("overlay: XDefaultVisual returned NULL".to_string());
    }
    let mut attributes = MaybeUninit::<XSetWindowAttributes>::zeroed();
    // SAFETY: the zeroed struct is a valid XSetWindowAttributes; only the masked field is set.
    let attributes_ptr = attributes.as_mut_ptr();
    unsafe { (*attributes_ptr).override_redirect = 1 };
    // SAFETY: live display, the default visual of `screen`, and an initialized attribute struct.
    let window = unsafe {
        XCreateWindow(
            display,
            root,
            x,
            y,
            width,
            height,
            0,
            depth,
            INPUT_OUTPUT_CLASS,
            visual,
            CW_OVERRIDE_REDIRECT,
            attributes_ptr,
        )
    };
    if window == 0 {
        return Err("overlay: XCreateWindow (default visual) returned 0".to_string());
    }
    Ok(window)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: WarpRect = WarpRect { x: 100.0, y: 50.0, width: 400.0, height: 300.0 };

    #[test]
    fn bounds_cover_source_and_target_with_integer_edges() {
        let target = WarpRect { x: 20.5, y: 600.2, width: 60.0, height: 40.0 };
        let (x, y, w, h) = union_bounds(SOURCE, target);
        assert_eq!((x, y), (20, 50));
        assert_eq!((w, h), (480, 591));
        assert!(f64::from(x) <= target.x && f64::from(y) <= SOURCE.y);
        assert!(f64::from(x + w as i32) >= SOURCE.x + SOURCE.width);
    }

    #[test]
    fn begin_fails_closed_without_display() {
        let capture = CapturedImage { width: 400, height: 300, argb: vec![0; 400 * 300 * 4] };
        assert!(WarpOverlay::begin(std::ptr::null_mut(), capture, SOURCE, SOURCE).is_err());
    }

    #[test]
    fn backdrop_surface_is_chosen_only_without_argb() {
        assert_eq!(warp_surface_for(true, true), WarpSurface::Argb);
        // Without a compositing manager the ARGB window would show black, so the opaque
        // root-snapshot path must take over even when a 32-bit visual exists.
        assert_eq!(warp_surface_for(true, false), WarpSurface::OpaqueBackdrop);
        assert_eq!(warp_surface_for(false, true), WarpSurface::OpaqueBackdrop);
        assert_eq!(warp_surface_for(false, false), WarpSurface::OpaqueBackdrop);
    }

    #[test]
    fn argb_probe_is_false_for_null_display() {
        assert!(!argb_visual_available(std::ptr::null_mut()));
        assert!(!warp_argb_usable(std::ptr::null_mut()));
        assert!(!compositing_manager_running(std::ptr::null_mut()));
    }

    #[test]
    fn fade_runs_from_small_transparent_to_full_opaque() {
        assert_eq!(fade_frame(0.0), (FADE_START_SCALE, 0.0));
        assert_eq!(fade_frame(1.0), (1.0, 1.0));
        let (mid_scale, mid_alpha) = fade_frame(0.5);
        assert!(mid_scale > FADE_START_SCALE && mid_scale < 1.0);
        assert!(mid_alpha > 0.5, "ease-out leads the linear midpoint");
        let mut last = (0.0, 0.0);
        for step in 0..=10 {
            let frame = fade_frame(f64::from(step) / 10.0);
            assert!(frame.0 >= last.0 && frame.1 >= last.1, "monotone at step {step}");
            last = frame;
        }
        assert_eq!(fade_frame(-3.0), fade_frame(0.0));
        assert_eq!(fade_frame(9.0), fade_frame(1.0));
    }

    #[test]
    fn fade_fails_closed_without_display() {
        let capture = CapturedImage { width: 40, height: 30, argb: vec![0; 40 * 30 * 4] };
        assert!(WarpOverlay::begin_fade(std::ptr::null_mut(), capture, 0, 0).is_err());
    }
}
