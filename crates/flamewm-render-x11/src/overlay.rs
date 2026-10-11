//! Magic Lamp overlay: one override-redirect ARGB window that shows a captured
//! window warped from its source rect toward a target rect.
//!
//! The capture is taken once by the caller and owned here. Each `paint` rebuilds
//! the Magic Lamp mesh and draws only the strips, so areas outside the mesh are
//! never painted. Raw Xlib stays inside render-x11; the WM sees only
//! [`WarpOverlay`]. The overlay must be dropped before the display it was opened on
//! is closed (the owning [`crate::ExternalDecorationRenderer`] outlives it).

use std::mem::MaybeUninit;
use std::os::raw::c_ulong;

use crate::warp::{self, WarpRect};
use crate::xcomposite::CapturedImage;
use crate::xlib::{
    CW_BACK_PIXEL, CW_COLORMAP, CW_OVERRIDE_REDIRECT, Colormap, Display, INPUT_OUTPUT_CLASS,
    TRANSPARENT_CLEAR_PIXEL, TRUE_COLOR_CLASS, Window, XCreateColormap, XCreateWindow,
    XDefaultScreen, XDestroyWindow, XFlush, XFreeColormap, XMapWindow, XMatchVisualInfo,
    XRootWindow, XSetWindowAttributes, XVisualInfo, ALLOC_NONE,
};
use crate::xrender_transform::XRenderTransformApi;

/// Strips per frame: enough for a smooth genie curve at 60 Hz.
const STRIP_COUNT: usize = 32;
/// `CWBorderPixel` value bit (not exported by `xlib`).
const CW_BORDER_PIXEL: c_ulong = 1 << 3;
const ARGB_DEPTH: i32 = 32;

/// A live Magic Lamp transition window. Destroys its window, colormap and picture on drop.
pub struct WarpOverlay {
    display: *mut Display,
    api: XRenderTransformApi,
    capture: CapturedImage,
    window: Window,
    colormap: Colormap,
    picture: c_ulong,
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
    /// Opens the overlay window covering both rects. `capture` must match `source`'s size.
    ///
    /// Fails without creating any X resource when the XRender transform extension,
    /// an ARGB32 visual, or the capture size is missing, so the caller skips the effect.
    pub fn begin(
        display: *mut Display,
        capture: CapturedImage,
        source: WarpRect,
        target: WarpRect,
    ) -> Result<Self, String> {
        if display.is_null() {
            return Err("overlay: no display".to_string());
        }
        let api = XRenderTransformApi::load()?;
        if !api.capabilities().picture_transform {
            return Err("overlay: XRenderSetPictureTransform unavailable".to_string());
        }
        if f64::from(capture.width) != source.width || f64::from(capture.height) != source.height {
            return Err("overlay: capture size does not match source rect".to_string());
        }
        let (bx, by, bw, bh) = union_bounds(source, target);
        let origin = WarpRect { x: f64::from(bx), y: f64::from(by), width: f64::from(bw), height: f64::from(bh) };
        let local = |r: WarpRect| WarpRect { x: r.x - origin.x, y: r.y - origin.y, ..r };

        // SAFETY: `display` is live per the caller contract; out-pointers are valid.
        let (window, colormap) = unsafe { create_argb_window(display, bx, by, bw, bh)? };
        // SAFETY: `window` is a live ARGB window on `display`; released in Drop.
        let picture = match unsafe { api.create_argb_picture(display, window) } {
            Ok(picture) => picture,
            Err(error) => {
                // SAFETY: created above and not used again.
                unsafe { destroy_window(display, window, colormap) };
                return Err(error);
            }
        };
        Ok(Self {
            display,
            api,
            capture,
            window,
            colormap,
            picture,
            source: local(source),
            target: local(target),
            mapped: false,
        })
    }

    /// Draws the warp at `progress` (0 = source, 1 = target) and maps the window on first call.
    pub fn paint(&mut self, progress: f64) -> Result<(), String> {
        let mesh = warp::magic_lamp_mesh(self.source, self.target, progress, STRIP_COUNT)?;
        // SAFETY: `display` and `picture` are live until Drop; capture is owned here.
        unsafe { warp::paint_warped(self.display, &self.api, &self.capture, &mesh, self.picture) }?;
        // SAFETY: `display` is live; the window is ours until Drop.
        unsafe {
            if !self.mapped {
                XMapWindow(self.display, self.window);
                self.mapped = true;
            }
            XFlush(self.display);
        }
        Ok(())
    }

    /// Tears the overlay down and returns its capture for later reuse (for example a restore).
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
        // SAFETY: `picture`, `window` and `colormap` were created in `begin` and are released once.
        unsafe {
            self.api.free_picture(self.display, self.picture);
            destroy_window(self.display, self.window, self.colormap);
            XFlush(self.display);
        }
    }
}

/// # Safety
///
/// `display` must be live.
unsafe fn destroy_window(display: *mut Display, window: Window, colormap: Colormap) {
    // SAFETY: live display and ids created by `create_argb_window`.
    unsafe {
        XDestroyWindow(display, window);
        XFreeColormap(display, colormap);
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
}
