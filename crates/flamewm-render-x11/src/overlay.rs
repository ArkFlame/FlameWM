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
//! - Crossfade ([`TransitionStyle::Fade`] via [`WarpOverlay::begin_styled`]). Draws the capture
//!   with an alpha mask over the root backdrop (opaque) or over clear (ARGB with a compositor),
//!   so it works with or without a compositing manager.
//!
//! The capture is taken once by the caller and owned here. Raw Xlib stays inside render-x11;
//! the WM sees only [`WarpOverlay`]. The overlay must be dropped before the display it was
//! opened on is closed (the owning [`crate::ExternalDecorationRenderer`] outlives it).

use std::mem::MaybeUninit;
use std::os::raw::c_ulong;

use crate::backdrop::capture_drawable_rect;
use crate::lamp::{self, LampBand, PanelEdge};
use crate::warp::{self, WarpRect, WarpSource};
use crate::xcomposite::{CapturedImage, check_capture_size, rgba_to_argb_owned, upload_argb_pixmap};
use crate::xlib::{
    CW_BACK_PIXEL, CW_COLORMAP, CW_OVERRIDE_REDIRECT, Colormap, Display, INPUT_OUTPUT_CLASS,
    Pixmap, TRANSPARENT_CLEAR_PIXEL, TRUE_COLOR_CLASS, Window, XCreateColormap, XCreateWindow,
    XDefaultDepth, XDefaultScreen, XDefaultVisual, XDestroyWindow, XDisplayHeight, XDisplayWidth,
    XCreatePixmap, XFlush, XFreeColormap, XFreePixmap, XMapWindow, XMatchVisualInfo, XRaiseWindow, XRootWindow,
    XSetWindowAttributes, XSync, XVisualInfo, ALLOC_NONE, XInternAtom,
};
use crate::xrender_transform::{PICT_STANDARD_RGB_24, XRenderTransformApi};

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

/// Alpha a rect-to-rect crossfade starts from and ends at 1 (maximize/unmaximize).
const MOVE_MIN_ALPHA: f64 = 0.35;

/// Transition the overlay draws. `MagicLamp` bends the capture toward the panel icon; `Fade`
/// crossfades the capture over the root backdrop, and moves it rect-to-rect when the rects differ.
/// `Stretch` moves the whole capture rect-to-rect with one affine transform per frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionStyle {
    MagicLamp,
    Fade,
    Stretch,
}

/// Whether the window appears (`In`) or disappears (`Out`). Magic Lamp ignores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionDirection {
    In,
    Out,
}

/// Ease-out quadratic of linear progress `t` (clamped). Exact 0 at `t = 0` and 1 at `t = 1`.
fn ease_out_quad(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t) * (1.0 - t)
}

/// Opacity of a crossfade at linear progress `progress` (clamped): `In` rises 0 to 1, `Out` falls
/// 1 to 0. Both end values are exact.
#[must_use]
pub fn fade_alpha(progress: f64, direction: TransitionDirection) -> f64 {
    let eased = ease_out_quad(progress);
    match direction {
        TransitionDirection::In => eased,
        TransitionDirection::Out => 1.0 - eased,
    }
}

/// Opacity of a crossfade frame. A rect-to-rect move never fades to zero: it runs
/// `MOVE_MIN_ALPHA` to 1 so the old content stays visible through the move.
#[must_use]
fn crossfade_alpha(progress: f64, direction: TransitionDirection, moving: bool) -> f64 {
    if moving {
        MOVE_MIN_ALPHA + (1.0 - MOVE_MIN_ALPHA) * ease_out_quad(progress)
    } else {
        fade_alpha(progress, direction)
    }
}

/// Destination rect of a `Stretch` frame at linear `progress` (clamped), ease-out from `source` to
/// `target`. Exact `source` at 0 and exact `target` at 1.
#[must_use]
fn stretch_frame(source: WarpRect, target: WarpRect, progress: f64) -> WarpRect {
    let eased = ease_out_quad(progress);
    if eased >= 1.0 {
        target
    } else if eased <= 0.0 {
        source
    } else {
        lerp_rect(source, target, eased)
    }
}

/// Linear blend of two rects by `t` (`t` in 0..=1, eased by the caller).
fn lerp_rect(a: WarpRect, b: WarpRect, t: f64) -> WarpRect {
    let mix = |p: f64, q: f64| p + (q - p) * t;
    WarpRect {
        x: mix(a.x, b.x),
        y: mix(a.y, b.y),
        width: mix(a.width, b.width),
        height: mix(a.height, b.height),
    }
}

/// Scale and alpha of the window-open effect at linear progress `t` (clamped).
/// Ease-out quadratic: both reach their end values exactly at `t = 1`.
#[must_use]
pub fn fade_frame(t: f64) -> (f64, f64) {
    let eased = ease_out_quad(t);
    (FADE_START_SCALE + (1.0 - FADE_START_SCALE) * eased, eased)
}

/// True when the display has a 32-bit TrueColor visual. A null display has none.
#[must_use]
pub fn argb_visual_available(display: *mut Display) -> bool {
    !display.is_null() && match_argb_visual(display).is_some()
}

/// The 32-bit TrueColor visual of the default screen, if the display has one.
fn match_argb_visual(display: *mut Display) -> Option<XVisualInfo> {
    let mut info = MaybeUninit::<XVisualInfo>::zeroed();
    // SAFETY: the display is live per the caller contract; `info` is writable and is read
    // only when the match succeeds.
    let found = unsafe {
        XMatchVisualInfo(display, XDefaultScreen(display), ARGB_DEPTH, TRUE_COLOR_CLASS, info.as_mut_ptr())
    };
    // SAFETY: a nonzero return means XMatchVisualInfo initialized `info`.
    (found != 0).then(|| unsafe { info.assume_init() })
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
    /// Crossfade direction when the overlay was opened with `TransitionStyle::Fade`. Uses
    /// `fade_source` (the capture as ARGB) and `window_root`/`icon_root` as source/target rects.
    crossfade: Option<TransitionDirection>,
    /// `Stretch` style: the whole capture moves rect-to-rect through one band per frame.
    stretch: bool,
    /// Warp capture uploaded once on the first warp frame and reused until Drop.
    warp_source: Option<WarpSource>,
    /// Window and taskbar icon in root coordinates, and the panel edge the bend pulls toward.
    window_root: WarpRect,
    icon_root: WarpRect,
    edge: PanelEdge,
    /// Root position of the overlay window's (0,0).
    origin: (i32, i32),
    /// Transparent solid used to clear the ARGB window before each warp frame.
    clear: c_ulong,
    mapped: bool,
}

/// Smallest integer rect covering both `source` and `target` in root coordinates.
fn union_bounds(source: WarpRect, target: WarpRect) -> (i32, i32, u32, u32) {
    enclose(
        source.x.min(target.x),
        source.y.min(target.y),
        (source.x + source.width).max(target.x + target.width),
        (source.y + source.height).max(target.y + target.height),
    )
}

/// Smallest integer rect `(x, y, width, height)` covering the float corners `(x0, y0)`-`(x1, y1)`.
fn enclose(x0: f64, y0: f64, x1: f64, y1: f64) -> (i32, i32, u32, u32) {
    let (x, y) = (x0.floor(), y0.floor());
    let (ex, ey) = (x1.ceil(), y1.ceil());
    (x as i32, y as i32, (ex - x) as u32, (ey - y) as u32)
}

impl WarpOverlay {
    fn empty(
        display: *mut Display,
        api: XRenderTransformApi,
        capture: CapturedImage,
        size: (u32, u32),
        origin: (i32, i32),
        window_root: WarpRect,
        icon_root: WarpRect,
        edge: PanelEdge,
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
            crossfade: None,
            stretch: false,
            window_root,
            icon_root,
            edge,
            origin,
            clear: 0,
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
        let edge = lamp::edge_for(source, target, screen_rect(display));
        Self::begin_styled(display, capture, source, target, edge, TransitionStyle::MagicLamp, TransitionDirection::In)
    }

    /// Opens a transition of `style` between `source` and `target` (root rects). `direction`
    /// picks appear (`In`) or disappear (`Out`); Magic Lamp ignores it. `capture` must match
    /// `source`'s size. Same failure and root-snapshot rules as [`WarpOverlay::begin`].
    pub fn begin_styled(
        display: *mut Display,
        capture: CapturedImage,
        source: WarpRect,
        target: WarpRect,
        edge: PanelEdge,
        style: TransitionStyle,
        direction: TransitionDirection,
    ) -> Result<Self, String> {
        let api = load_checked_api(display)?;
        if f64::from(capture.width) != source.width || f64::from(capture.height) != source.height {
            return Err("overlay: capture size does not match source rect".to_string());
        }
        let (bx, by, bw, bh) = union_bounds(source, target);
        let mut overlay = Self::empty(display, api, capture, (bw, bh), (bx, by), source, target, edge);
        overlay.create_surface(bx, by, bw, bh)?;
        match style {
            TransitionStyle::MagicLamp => {}
            TransitionStyle::Stretch => overlay.stretch = true,
            TransitionStyle::Fade => {
                overlay.crossfade = Some(direction);
                overlay.upload_fade_source()?;
            }
        }
        Ok(overlay)
    }

    /// Creates the window and its picture for the surface `warp_surface_for` picks. Each resource
    /// is recorded in `self` once it exists, so Drop releases it when a later step fails.
    fn create_surface(&mut self, bx: i32, by: i32, bw: u32, bh: u32) -> Result<(), String> {
        let display = self.display;
        match warp_surface_for(argb_visual_available(display), compositing_manager_running(display)) {
            WarpSurface::Argb => {
                // SAFETY: `display` is live per the caller contract.
                let (window, colormap) = unsafe { create_argb_window(display, bx, by, bw, bh)? };
                self.window = window;
                self.colormap = Some(colormap);
                // SAFETY: `window` is live and owned by `self`.
                self.picture = unsafe { self.api.create_argb_picture(display, window) }?;
                // SAFETY: `display` is live; the solid is recorded in `self` for Drop.
                self.clear = unsafe { self.api.create_solid_fill(display, 0, 0, 0, 0) }?;
            }
            WarpSurface::OpaqueBackdrop => {
                // SAFETY: `display` is live; the snapshot picture is owned by `self`.
                self.backdrop = Some(unsafe { capture_backdrop(display, &self.api, bx, by, bw, bh)? });
                // SAFETY: `display` is live; the window is recorded for Drop.
                let window = unsafe { create_default_window(display, bx, by, bw, bh)? };
                self.window = window;
                // SAFETY: `window` is live and owned by `self`.
                self.picture = unsafe {
                    self.api.create_standard_picture(display, window, PICT_STANDARD_RGB_24)
                }?;
                // SAFETY: `display` and `window` are live; the pixmap is recorded for Drop once created.
                let pixmap = unsafe {
                    XCreatePixmap(display, window, bw, bh, XDefaultDepth(display, XDefaultScreen(display)) as u32)
                };
                if pixmap == 0 {
                    return Err("overlay: offscreen pixmap returned 0".to_string());
                }
                self.offscreen = Some(Backdrop { pixmap, picture: 0 });
                // SAFETY: `pixmap` is live and owned by `self`.
                let picture = unsafe { self.api.create_standard_picture(display, pixmap, PICT_STANDARD_RGB_24) }?;
                if let Some(off) = self.offscreen.as_mut() {
                    off.picture = picture;
                }
            }
        }
        Ok(())
    }

    /// Uploads the capture as an ARGB pixmap and picture (`fade_pixmap`, `fade_source`), with a
    /// bilinear filter when available. Both are recorded before any further fallible call.
    fn upload_fade_source(&mut self) -> Result<(), String> {
        let display = self.display;
        // SAFETY: `display` is live per the caller contract; the pixmap is recorded for Drop.
        self.fade_pixmap = unsafe { upload_argb_pixmap(display, &self.capture) }?;
        // SAFETY: `fade_pixmap` is live and owned by `self`.
        self.fade_source = unsafe { self.api.create_argb_picture(display, self.fade_pixmap) }?;
        if self.api.capabilities().picture_filter {
            // SAFETY: `fade_source` is live and owned by `self`.
            unsafe { self.api.set_bilinear_filter(display, self.fade_source) }?;
        }
        Ok(())
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
        let mut overlay = Self::empty(display, api, capture, (w, h), (x, y), full, full, PanelEdge::Bottom);
        overlay.fade = true;
        // SAFETY: `display` is live per the caller contract; resources are recorded as created.
        let (window, colormap) = unsafe { create_argb_window(display, x, y, w, h)? };
        overlay.window = window;
        overlay.colormap = Some(colormap);
        // SAFETY: `window` is live and owned by `overlay`.
        overlay.picture = unsafe { overlay.api.create_argb_picture(display, window) }?;
        overlay.upload_fade_source()?;
        // SAFETY: `display` is live; the solid transparent picture is owned by `overlay`.
        overlay.fade_clear = unsafe { overlay.api.create_solid_fill(display, 0, 0, 0, 0) }?;
        Ok(overlay)
    }

    /// Draws the effect at `progress` and maps the window on first call.
    ///
    /// For a warp, `progress` 0 is the source and 1 the target. For a fade it is linear time
    /// 0..=1, eased by [`fade_frame`].
    /// Stacks the overlay above application windows (a flight starts over the popup).
    pub fn raise(&self) {
        // SAFETY: `display` is live; the window is ours until Drop.
        unsafe { XRaiseWindow(self.display, self.window) };
    }

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
        } else if self.stretch {
            self.paint_stretch(progress)?;
        } else if self.crossfade.is_some() {
            self.paint_crossfade(progress)?;
        } else {
            self.paint_warp(progress)?;
        }
        // SAFETY: `display` is live; the window is ours until Drop.
        unsafe { XFlush(self.display) };
        Ok(())
    }

    fn paint_warp(&mut self, progress: f64) -> Result<(), String> {
        let extent = match self.edge {
            PanelEdge::Bottom | PanelEdge::Top => self.window_root.height,
            PanelEdge::Left | PanelEdge::Right => self.window_root.width,
        };
        let bands = lamp::lamp_bands(self.window_root, self.icon_root, self.edge, progress, extent)?;
        self.paint_bands(&bands)
    }

    /// Stretch frame: the whole capture at the eased rect, one band, one affine transform.
    fn paint_stretch(&mut self, progress: f64) -> Result<(), String> {
        let rect = stretch_frame(self.window_root, self.icon_root, progress);
        if !(rect.width > 0.0 && rect.height > 0.0) {
            return Err("overlay: stretch rect has no area".to_string());
        }
        let band = LampBand {
            src: (0.0, 0.0, f64::from(self.capture.width), f64::from(self.capture.height)),
            dst: (rect.x, rect.y, rect.width, rect.height),
        };
        self.paint_bands(&[band])
    }

    /// Clears or snapshots the surface, then paints `bands` of the capture onto it.
    fn paint_bands(&mut self, bands: &[LampBand]) -> Result<(), String> {
        let (w, h) = self.size;
        let origin = (f64::from(self.origin.0), f64::from(self.origin.1));
        let dest = self.offscreen.as_ref().map_or(self.picture, |off| off.picture);
        // SAFETY: `dest` and the fill picture are live until Drop; the snapshot covers the whole
        // window, so every pixel is opaque before the bands are drawn.
        unsafe { self.begin_frame(dest) };
        if self.warp_source.is_none() {
            // SAFETY: `display` is live until Drop; the capture never changes after `begin`, so
            // one upload serves every frame of this warp.
            self.warp_source = Some(unsafe { warp::upload_warp_source(self.display, &self.api, &self.capture) }?);
        }
        let Some(source) = self.warp_source.as_ref() else {
            return Err("overlay: warp source missing after upload".to_string());
        };
        // SAFETY: `display` and `dest` are live until Drop; `source` was uploaded on `display`.
        unsafe { warp::paint_warped(self.display, &self.api, source, bands, origin, dest) }?;
        if dest != self.picture {
            // SAFETY: both pictures are live on `display`; one copy shows the finished frame.
            unsafe { self.api.composite_src(self.display, dest, self.picture, (0, 0, w, h)) };
        }
        Ok(())
    }

    /// Fills `dest` with the root snapshot when opaque, else the transparent clear.
    ///
    /// # Safety
    ///
    /// `dest` and the fill picture must be live on `self.display` until Drop.
    unsafe fn begin_frame(&self, dest: c_ulong) {
        let fill = self.backdrop.as_ref().map_or(self.clear, |backdrop| backdrop.picture);
        let (w, h) = self.size;
        // SAFETY: forwarded contract; `fill` and `dest` are live pictures on `self.display`.
        unsafe { self.api.composite_src(self.display, fill, dest, (0, 0, w, h)) };
    }

    /// Composites the fade source OVER `dest` at `rect` through a solid mask of `alpha` (0..=1).
    ///
    /// # Safety
    ///
    /// `display`, the fade source and `dest` must be live pictures on the same display.
    unsafe fn composite_alpha(
        &self,
        display: *mut Display,
        dest: c_ulong,
        rect: (i32, i32, u32, u32),
        alpha: f64,
    ) -> Result<(), String> {
        let mask_alpha = (alpha.clamp(0.0, 1.0) * f64::from(u16::MAX)).round() as u16;
        // SAFETY: forwarded contract; the mask is freed before returning.
        unsafe {
            let mask = self.api.create_solid_fill(display, 0, 0, 0, mask_alpha)?;
            self.api
                .composite_over_masked(display, self.fade_source, mask, dest, rect);
            self.api.free_picture(display, mask);
        }
        Ok(())
    }

    fn paint_fade(&mut self, progress: f64) -> Result<(), String> {
        let (scale, alpha) = fade_frame(progress);
        let (w, h) = (f64::from(self.size.0), f64::from(self.size.1));
        let (cx, cy) = (w / 2.0, h / 2.0);
        // Destination of the scaled capture, centred in the window.
        let rect = enclose(cx - cx * scale, cy - cy * scale, cx + cx * scale, cy + cy * scale);
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
                self.composite_alpha(display, self.picture, rect, alpha)?;
            }
        }
        Ok(())
    }

    /// Crossfade frame: the capture at the eased rect (`window_root` to `icon_root`, in overlay
    /// pixels) composited over the backdrop, or over clear on ARGB, at the alpha of
    /// [`crossfade_alpha`]. One mask picture per frame carries the alpha.
    fn paint_crossfade(&mut self, progress: f64) -> Result<(), String> {
        let Some(direction) = self.crossfade else {
            return Err("overlay: crossfade direction missing".to_string());
        };
        let origin = (f64::from(self.origin.0), f64::from(self.origin.1));
        let local = |r: WarpRect| WarpRect { x: r.x - origin.0, y: r.y - origin.1, ..r };
        let (from, to) = (local(self.window_root), local(self.icon_root));
        let moving = from != to;
        let eased = ease_out_quad(progress);
        let rect = lerp_rect(from, to, eased);
        if !(rect.width > 0.0 && rect.height > 0.0) {
            return Err("overlay: crossfade rect has no area".to_string());
        }
        let alpha = crossfade_alpha(progress, direction, moving);
        // Destination pixels to capture pixels: the capture spans the window's source size.
        let scale_x = f64::from(self.capture.width) / rect.width;
        let scale_y = f64::from(self.capture.height) / rect.height;
        let matrix = [[scale_x, 0.0, -rect.x * scale_x], [0.0, scale_y, -rect.y * scale_y], [0.0, 0.0, 1.0]];
        let dst_rect = enclose(rect.x, rect.y, rect.x + rect.width, rect.y + rect.height);
        let (w, h) = self.size;
        let dest = self.offscreen.as_ref().map_or(self.picture, |off| off.picture);
        let display = self.display;
        // SAFETY: all pictures are live on `display` until Drop; the mask is freed below.
        unsafe {
            self.begin_frame(dest);
            if alpha > 0.0 {
                self.api.set_transform(display, self.fade_source, matrix)?;
                self.composite_alpha(display, dest, dst_rect, alpha)?;
            }
            if dest != self.picture {
                // One copy shows the finished frame, never the intermediate backdrop.
                self.api.composite_src(display, dest, self.picture, (0, 0, w, h));
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
            let free = |picture| {
                if picture != 0 {
                    self.api.free_picture(self.display, picture);
                }
            };
            free(self.clear);
            // Fade and crossfade share the capture picture; zero ids were never created.
            free(self.fade_source);
            free(self.fade_clear);
            if self.fade_pixmap != 0 {
                XFreePixmap(self.display, self.fade_pixmap);
            }
            free(self.picture);
            let release = |b: Backdrop| {
                free(b.picture);
                XFreePixmap(self.display, b.pixmap);
            };
            self.backdrop.take().into_iter().chain(self.offscreen.take()).for_each(release);
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

/// Root screen rect of `display`'s default screen; zero size for a null display.
fn screen_rect(display: *mut Display) -> WarpRect {
    if display.is_null() {
        return WarpRect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 };
    }
    // SAFETY: live display per the caller contract; default screen queries have no other preconditions.
    let (w, h) = unsafe {
        let screen = XDefaultScreen(display);
        (XDisplayWidth(display, screen), XDisplayHeight(display, screen))
    };
    WarpRect { x: 0.0, y: 0.0, width: f64::from(w), height: f64::from(h) }
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
    let captured = rgba_to_argb_owned(shot.width, shot.height, shot.pixels)?;
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
    let info = match_argb_visual(display).ok_or("overlay: no 32-bit TrueColor visual")?;
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
    fn begin_paths_fail_closed_without_display() {
        let capture = CapturedImage { width: 400, height: 300, argb: vec![0; 400 * 300 * 4] };
        assert!(WarpOverlay::begin(std::ptr::null_mut(), capture, SOURCE, SOURCE).is_err());
        let capture = CapturedImage { width: 40, height: 30, argb: vec![0; 40 * 30 * 4] };
        assert!(WarpOverlay::begin_fade(std::ptr::null_mut(), capture, 0, 0).is_err());
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
    fn crossfade_alpha_has_exact_endpoints_per_direction() {
        assert_eq!(fade_alpha(0.0, TransitionDirection::In), 0.0);
        assert_eq!(fade_alpha(1.0, TransitionDirection::In), 1.0);
        assert_eq!(fade_alpha(0.0, TransitionDirection::Out), 1.0);
        assert_eq!(fade_alpha(1.0, TransitionDirection::Out), 0.0);
        assert_eq!(fade_alpha(-3.0, TransitionDirection::In), 0.0);
        assert_eq!(fade_alpha(9.0, TransitionDirection::In), 1.0);
        assert_eq!(fade_alpha(9.0, TransitionDirection::Out), 0.0);
    }

    #[test]
    fn crossfade_alpha_is_eased_and_monotone_in_its_direction() {
        // Ease-out leads the linear midpoint: In rises fast, Out falls fast.
        assert_eq!(fade_alpha(0.5, TransitionDirection::In), 0.75);
        assert_eq!(fade_alpha(0.5, TransitionDirection::Out), 0.25);
        let (mut last_in, mut last_out) = (-1.0, 2.0);
        for step in 0..=20 {
            let t = f64::from(step) / 20.0;
            let (a_in, a_out) = (fade_alpha(t, TransitionDirection::In), fade_alpha(t, TransitionDirection::Out));
            assert!(a_in >= last_in && a_out <= last_out, "monotone at step {step}");
            assert!((0.0..=1.0).contains(&a_in) && (0.0..=1.0).contains(&a_out));
            assert!((a_in + a_out - 1.0).abs() < 1e-12, "Out is the mirror of In");
            (last_in, last_out) = (a_in, a_out);
        }
    }

    #[test]
    fn rect_move_never_fades_below_minimum_alpha() {
        assert_eq!(crossfade_alpha(0.0, TransitionDirection::Out, true), MOVE_MIN_ALPHA);
        assert_eq!(crossfade_alpha(1.0, TransitionDirection::In, true), 1.0);
        // Without a rect move the plain fade applies.
        assert_eq!(crossfade_alpha(1.0, TransitionDirection::Out, false), 0.0);
    }

    #[test]
    fn lerp_rect_hits_both_rects_at_the_ends() {
        let a = WarpRect { x: 10.0, y: 20.0, width: 100.0, height: 50.0 };
        let b = WarpRect { x: 0.0, y: 0.0, width: 400.0, height: 300.0 };
        assert_eq!(lerp_rect(a, b, 0.0), a);
        assert_eq!(lerp_rect(a, b, 1.0), b);
        assert_eq!(lerp_rect(a, a, 0.5), a);
    }

    #[test]
    fn styled_begin_fails_closed_without_display_for_each_style() {
        for style in [TransitionStyle::MagicLamp, TransitionStyle::Fade, TransitionStyle::Stretch] {
            for direction in [TransitionDirection::In, TransitionDirection::Out] {
                let capture = CapturedImage { width: 400, height: 300, argb: vec![0; 400 * 300 * 4] };
                let result = WarpOverlay::begin_styled(
                    std::ptr::null_mut(),
                    capture,
                    SOURCE,
                    SOURCE,
                    PanelEdge::Bottom,
                    style,
                    direction,
                );
                assert!(result.is_err(), "{style:?} {direction:?} must fail closed");
            }
        }
    }

    #[test]
    fn stretch_frame_hits_source_and_target_exactly() {
        let a = WarpRect { x: 0.0, y: 489.0, width: 1278.0, height: 300.0 };
        let b = WarpRect { x: 0.0, y: 0.0, width: 1278.0, height: 1024.0 };
        assert_eq!(stretch_frame(a, b, 0.0), a);
        assert_eq!(stretch_frame(a, b, 1.0), b);
        assert_eq!(stretch_frame(a, b, 9.0), b);
        assert_eq!(stretch_frame(b, a, 0.0), b);
        assert_eq!(stretch_frame(b, a, 1.0), a);
    }

    #[test]
    fn stretch_frame_is_monotone_between_the_rects() {
        let a = WarpRect { x: 200.0, y: 300.0, width: 100.0, height: 80.0 };
        let b = WarpRect { x: 0.0, y: 0.0, width: 1278.0, height: 1024.0 };
        let mut last = a;
        for step in 1..=20 {
            let r = stretch_frame(a, b, f64::from(step) / 20.0);
            assert!(r.x <= last.x + 1e-9 && r.y <= last.y + 1e-9, "origin moves toward target at {step}");
            assert!(r.width >= last.width - 1e-9 && r.height >= last.height - 1e-9, "size grows at {step}");
            last = r;
        }
    }

    #[test]
    fn stretch_frame_has_no_nan_for_degenerate_rects() {
        let zero = WarpRect { x: 5.0, y: 5.0, width: 0.0, height: 0.0 };
        let wide = WarpRect { x: 0.0, y: 0.0, width: 800.0, height: 600.0 };
        for (a, b) in [(zero, wide), (wide, zero), (zero, zero)] {
            for step in 0..=10 {
                let r = stretch_frame(a, b, f64::from(step) / 10.0);
                assert!(r.x.is_finite() && r.y.is_finite() && r.width.is_finite() && r.height.is_finite());
            }
        }
    }
}
