use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::CString;
use std::mem::MaybeUninit;
use std::ptr;
use std::rc::Rc;

use flamewm_render_core::{
    ActionEvent, ActionPhase, Color, CursorKind, InteractionState, LayoutEngine, LayoutResult,
    PaintCommand, PointerButton, Rect, RuntimeDocument, SurfaceDamage,
    build_paint_commands_with_scroll,
};

use std::os::raw::{c_int, c_ulong};

use super::config::{X11Config, X11WindowRole};
use super::native::cursor::{DeferredNativeLibraryHandles, NativeCursorSession, XcursorBackend};
use super::native::surface_format::{SurfaceAlphaMode, fallback_mode, validate_argb32_visual};
use super::native::target::{
    GeometryCommit, NativeSurfaceTarget, PresentationState, presenter_error,
};
use super::native::xresource::{
    X11ResourceAllocator, refresh_backbuffer_gauge, refresh_image_gauge,
};
use super::xft::XftBackend;
use super::xlib::*;
use super::xrender::XRenderBackend;
use super::xshape::XShapeBridge;
use super::*;
use crate::external_drawable::match_argb32_visual;
use crate::surface_controller::{RawPressSelection, xi_raw_press_opcode};
use cursor::cursor_shape;
use cursor_theme::{SurfaceCursor, share_surface_cursor};

mod clip;
// Region planner and counters; the live redraw path adopts them once region clipping is enabled.
mod backbuffer_pool;
mod color;
mod cursor;
mod cursor_theme;
#[cfg_attr(not(test), allow(dead_code))]
mod damage_plan;
mod events;
mod geometry;
mod images;
#[cfg(test)]
mod region_equivalence_tests;
mod xdnd;
pub(crate) use images::upload_argb32_image;
mod input;
mod layout_helpers;
mod primitives;
mod raw_press;
mod render;
mod scroll;
mod surface_backdrop;
mod window_backdrop;

pub(crate) use input::{pointer_button_from_raw, slider_value_from_pointer, wheel_scroll_delta};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ImageCacheKey {
    node: u32,
    asset: u16,
    width: u32,
    height: u32,
    revision: u64,
    treatment: u8,
    fg_r: u8,
    fg_g: u8,
    fg_b: u8,
    fg_a: u8,
}

#[derive(Clone, Copy)]
struct CachedImage {
    pixmap: Pixmap,
    picture: u64,
    mask: Pixmap,
    mask_bytes: usize,
    bytes: usize,
}

/// Retained paint cache entry: commands rendered once per layout
/// revision; frames borrow without cloning.
/// Inputs that determine retained layout geometry. Paint-only edits (colors) keep
/// `layout_revision` unchanged, so they reuse layout; size, scale or any layout edit do not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LayoutKey {
    pub(crate) layout_revision: u64,
    /// Isolated geometry moves: the retained layout is patched in place, not recomputed.
    pub(crate) geometry_revision: u64,
    pub(crate) viewport: (u32, u32),
    pub(crate) scale_bits: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct CachedPaint {
    pub(crate) commands: Vec<PaintCommand>,
    pub(crate) revision: u64,
    pub(crate) interaction: InteractionState,
    pub(crate) scroll_fingerprint: u64,
}

unsafe fn free_cached_image(
    display: *mut Display,
    allocator: &mut X11ResourceAllocator,
    image: CachedImage,
) {
    unsafe {
        let _ = image.picture;
        allocator.free_pixmap(
            display,
            image.pixmap,
            image.bytes.saturating_sub(image.mask_bytes),
        );
        if image.mask != 0 {
            allocator.free_pixmap(display, image.mask, image.mask_bytes);
        }
    }
}

/// Pure SymbolicForeground math: eff_a = round(sa*fa/255),
/// premul channel = round(fg*eff_a/255). Testable without X.
pub(crate) fn symbolic_foreground_argb32(
    pixels: &[u8],
    src_w: u32,
    src_h: u32,
    width: u32,
    height: u32,
    fg: Color,
) -> Vec<u32> {
    let (sw, sh, dw, dh) = (src_w.max(1), src_h.max(1), width.max(1), height.max(1));
    let mut out = Vec::with_capacity((dw as usize) * (dh as usize));
    for y in 0..dh {
        let sy = ((y as u64 * src_h as u64) / dh as u64).min((sh - 1) as u64) as usize;
        for x in 0..dw {
            let sx = ((x as u64 * src_w as u64) / dw as u64).min((sw - 1) as u64) as usize;
            let off = (sy * sw as usize + sx) * 4;
            let sa = u32::from(pixels[off + 3]);
            let eff_a = (sa * u32::from(fg.a) + 127) / 255;
            let prem = |c: u32| ((c * eff_a + 127) / 255);
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

pub(crate) struct X11App {
    pub(crate) display: *mut Display,
    pub(crate) screen: i32,
    pub(crate) depth: i32,
    pub(crate) window: Window,
    /// Native window role decides whether a background click should own focus.
    pub(crate) role: X11WindowRole,
    pub(crate) backbuffer: Pixmap,
    pub(crate) gc: GC,
    pub(crate) colormap: Colormap,
    pub(crate) colors: HashMap<Color, u64>,
    images: HashMap<ImageCacheKey, CachedImage>,
    pub(crate) allocator: X11ResourceAllocator,
    backbuffer_bytes: usize,
    /// Pixel capacity of `backbuffer` (>= window size); see `backbuffer_pool`.
    pub(crate) backbuffer_cap: (u32, u32),
    pub(crate) cursors: HashMap<CursorKind, Cursor>,
    pub(crate) current_cursor: Option<CursorKind>,
    pub(crate) xcursor: Option<Rc<RefCell<SurfaceCursor>>>,
    pub(crate) fonts: HashMap<u8, *mut XFontStructHead>,
    pub(crate) xft: Option<XftBackend>,
    pub(crate) xrender: Option<XRenderBackend>,
    pub(crate) xshape: Option<XShapeBridge>,
    /// Floating dock corner radius in window px. Non-zero keeps the coverage refresh from
    /// replacing the rounded dock mask; zero leaves the coverage mask in charge.
    pub(crate) dock_corner_radius: u32,
    /// Border-only ring (`border_px`, `radius_px`) in window px. When set, the coverage refresh
    /// keeps only the ring so the interior stays see-through.
    pub(crate) ring_mask: Option<(u32, u32)>,
    /// Per-pixel alpha silhouette for an XShape-backed overlay without an ARGB compositor.
    pub(crate) shape_override: Option<Vec<(u32, u32, u32)>>,
    #[allow(dead_code)]
    pub(crate) visual: *mut Visual,
    pub(crate) alpha_mode: SurfaceAlphaMode,
    pub(crate) surface: Option<NativeSurfaceTarget>,
    pub(crate) wm_delete: Atom,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) presentation: PresentationState,
    pub(crate) clip_stack: Vec<Rect>,
    /// Last clip applied by `apply_clip_stack` (`Some(None)` = cleared); `None` = unknown.
    pub(crate) gc_clip: Option<Option<(i16, i16, u16, u16)>>,
    pub(crate) range_drag: Option<(u32, f32, f32, f32, f32)>,
    pub(crate) interaction: InteractionState,
    /// Retained layout+paint cache: reuse while (revision, viewport,
    /// interaction, scroll fingerprint) is unchanged. No frame clones.
    pub(crate) layout: Option<LayoutResult>,
    pub(crate) cached_paint: Option<CachedPaint>,
    /// Per-node paint memo behind `cached_paint`; re-emits only nodes whose inputs changed.
    pub(crate) paint_nodes: scroll::PaintNodeCache,
    pub(crate) cached_layout_key: Option<LayoutKey>,
    /// Hash of the bounding-shape spans last applied to `window` (None = unknown/clipped).
    pub(crate) shape_applied: std::cell::Cell<Option<u64>>,
    pub(crate) pending_damage: SurfaceDamage,
    /// Region painting for the opaque subset. **Opt-in** (`FLAMEWM_RENDER_REGION=1`): the live path
    /// paints Full, plus `RedrawPlan::Skip` when nothing changed. The incremental path is not safe
    /// for real content yet — Xft has no XRender clip, so text is cropped by hand, and a stale region
    /// leaves partial windows. Enabling it by default silently activated that path for every app and
    /// dropped terminal updates, clipped windows and left selection rectangles behind.
    pub(crate) region_paint: bool,
    /// A Full damage latch was consumed into the next redraw.
    pub(crate) region_force_full: bool,
    /// The backbuffer holds the last fully presented frame (region base).
    pub(crate) region_base_valid: bool,
    /// This redraw was requested by input alone: no document damage and no revision change, so
    /// interaction is the only thing that can differ. Only then may an unchanged command list skip.
    pub(crate) skip_eligible: bool,
    /// Region being repainted: Xft text is clipped to it. None on the Full path.
    pub(crate) region_text_clip: Option<damage_plan::PixelRegion>,
    /// An input event was handled and the next redraw must run through the planner.
    pub(crate) input_repaint: bool,
    pub(crate) render_counters: damage_plan::RedrawCounters,
    /// Pixmap currently bound via XSetWindowBackgroundPixmap (0 = none).
    /// Only ever a fully painted backbuffer; never an unpainted replacement.
    pub(crate) background_bound: Pixmap,
    /// Window size of the last frame presented from the bound backbuffer.
    pub(crate) presented_size: Option<(u32, u32)>,
    /// Desktop surfaces only: the frame last published on the root.
    pub(crate) published_desktop_frame: Option<PublishedDesktopFrame>,
    /// Interned [`crate::backdrop::DESKTOP_FRAME_PROPERTY`]; 0 until first use.
    pub(crate) desktop_frame_atom: Atom,
    /// Stale backbuffer kept alive across resize until the replacement is
    /// painted and bound. (pixmap, bytes).
    pub(crate) retired_backbuffer: Option<(Pixmap, usize)>,
    /// Pseudo-transparency snapshot laid under translucent fills (None = transparent clear).
    pub(crate) surface_backdrop: Option<surface_backdrop::SurfaceBackdrop>,
    /// Whole-window wallpaper snapshot for a Normal window with translucent fills.
    pub(crate) window_backdrop_pixmap: Option<surface_backdrop::SurfaceBackdrop>,
    pub(crate) window_backdrop: Option<window_backdrop::WindowBackdropState>,
    /// Latest client size from ConfigureNotify, reported once the X queue drains.
    pub(crate) pending_window_resize: Option<(u32, u32)>,
    /// Window backdrop generation the backbuffer was last painted over (`window_backdrop`).
    pub(crate) window_backdrop_painted: Option<u64>,
    pub(crate) node_backdrops: surface_backdrop::NodeBackdropCache,
    pub(crate) pointer_button: Option<u32>,
    pub(crate) pointer_grabbed: bool,
    /// Input focus displaced when this popup took it (window, revert_to).
    pub(crate) saved_input_focus: Option<(Window, c_int)>,
    pub(crate) super_chord_used: bool,
    pub(crate) last_title_press: Option<(u32, u64)>,
    pub(crate) single_event: bool,
    pub(crate) close_requested: bool,
    /// Root-press observer selection; armed only while the app asks for it.
    pub(crate) press_selection: RawPressSelection,
    press_observed: bool,
    raw_presses: Vec<RawPress>,
    /// XDND state; `Some` only for windows created with `X11Config::file_drag`.
    pub(crate) xdnd: Option<xdnd::XdndState>,
}
impl X11App {
    /// # Safety
    ///
    /// `display` must be a live Xlib display owned by the caller and remain
    /// open until the returned app has been dropped.
    pub(crate) unsafe fn new(display: *mut Display, config: &X11Config) -> Result<Self, String> {
        unsafe { Self::new_surface(display, config, config.x, config.y, true) }
    }

    /// # Safety
    ///
    /// `display` must be a live Xlib display owned by the caller and remain
    /// open until the returned app has been dropped.
    pub(crate) unsafe fn new_surface(
        display: *mut Display,
        config: &X11Config,
        x: i32,
        y: i32,
        initially_visible: bool,
    ) -> Result<Self, String> {
        // Every app display gets the non-fatal protocol handler: a BadWindow from a reply to a
        // vanished peer (XDND source) must not exit the process. Idempotent per process.
        install_x_protocol_error_handler();
        crate::xlib::return_large_frees_to_system();
        let screen = unsafe { XDefaultScreen(display) };
        let root = unsafe { XRootWindow(display, screen) };
        // A requested size larger than the screen (a small or scaled display) would only fail the
        // root-bounded backbuffer below, so the window starts at most screen-sized.
        let (root_w, root_h) = display_extent(display, screen);
        let width = config.width.clamp(1, root_w.max(1));
        let height = config.height.clamp(1, root_h.max(1));
        // Canonical scene creation: prefer a depth-32 TrueColor ARGB visual
        // validated by native/surface_format; fall back to the default visual
        // (opaque or shape-backed) only when no real alpha channel exists.
        if let Some(argb_visual) = match_argb32_visual(display, screen) {
            let alpha_mask = unsafe { visual_alpha_mask(display, argb_visual) };
            match validate_argb32_visual(32, TRUE_COLOR_CLASS, alpha_mask) {
                // No compositor is claimed in the default Xephyr path, so a
                // depth-32 window cannot blend fractionally with the desktop.
                // Keep the ARGB32 scene for in-scene alpha, but present via
                // the shape-backed mode unless a compositing manager owns its
                // selection. Never claim fractional desktop translucency here.
                Ok(_) => {
                    eprintln!("FLAMEWM_RENDER_SURFACE_FORMAT shape-backed (no compositor claim)")
                }
                Err(error) => eprintln!("FLAMEWM_RENDER_SURFACE_FORMAT degraded reason={error}"),
            }
        }
        let (visual, depth, alpha_mode) = unsafe {
            (
                XDefaultVisual(display, screen),
                XDefaultDepth(display, screen),
                fallback_mode(shape_available(display)),
            )
        };
        if visual.is_null() {
            return Err("no usable visual for native surface".to_string());
        }
        let owns_argb_colormap = depth == 32 && alpha_mode == SurfaceAlphaMode::CompositedArgb32;
        let colormap = if owns_argb_colormap {
            let cmap = unsafe { XCreateColormap(display, root, visual, ALLOC_NONE) };
            if cmap == 0 {
                return Err("XCreateColormap failed for ARGB32 surface".to_string());
            }
            cmap
        } else {
            unsafe { XDefaultColormap(display, screen) }
        };
        let mut attributes = XSetWindowAttributes {
            background_pixmap: 0,
            background_pixel: TRANSPARENT_CLEAR_PIXEL,
            border_pixmap: 0,
            border_pixel: 0,
            bit_gravity: 0,
            win_gravity: 0,
            backing_store: 0,
            backing_planes: 0,
            backing_pixel: 0,
            save_under: 0,
            event_mask: 0,
            do_not_propagate_mask: 0,
            override_redirect: 0,
            colormap,
            cursor: 0,
        };
        let mut valuemask = CW_BACK_PIXEL | CW_COLORMAP;
        let override_redirect = matches!(
            config.role,
            X11WindowRole::PopupMenu | X11WindowRole::DropdownMenu | X11WindowRole::Overlay
        );
        if override_redirect {
            attributes.override_redirect = 1;
            valuemask |= CW_OVERRIDE_REDIRECT;
        }
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
                valuemask,
                &mut attributes,
            )
        };
        if window == 0 {
            if owns_argb_colormap {
                unsafe { XFreeColormap(display, colormap) };
            }
            return Err("XCreateWindow failed for native surface".to_string());
        }
        let event_mask = EXPOSURE_MASK
            | STRUCTURE_NOTIFY_MASK
            | POINTER_MOTION_MASK
            | LEAVE_WINDOW_MASK
            | BUTTON_PRESS_MASK
            | BUTTON_RELEASE_MASK
            | KEY_PRESS_MASK
            | KEY_RELEASE_MASK;
        unsafe {
            XSelectInput(display, window, event_mask);
        }
        let title = CString::new(config.title.as_str())
            .map_err(|_| "window title contains NUL".to_string())?;
        unsafe {
            XStoreName(display, window, title.as_ptr());
        }
        set_window_identity(
            display,
            window,
            config.title.as_str(),
            config.identity.as_ref(),
        );
        set_window_role(display, window, config.role);
        if config.role == X11WindowRole::Dock {
            let (root_w, root_h) = display_extent(display, screen);
            apply_dock_strut(display, window, root_w, root_h, None, x, y, width, height);
        }
        let gc = unsafe { XCreateGC(display, window, 0, ptr::null_mut()) };
        if gc.is_null() {
            if owns_argb_colormap {
                unsafe { XFreeColormap(display, colormap) };
            }
            unsafe {
                XDestroyWindow(display, window);
            }
            return Err("XCreateGC failed".to_string());
        }
        // Image painting uses many XCopyArea requests. The Xlib default is to
        // request GraphicsExpose/NoExpose events for each copy, which creates
        // useless event-queue traffic for a retained-mode UI renderer.
        unsafe {
            XSetGraphicsExposures(display, gc, 0);
        }
        let backbuffer = {
            let (root_w, root_h) = display_extent(display, screen);
            let mut owned = X11ResourceAllocator::new();
            let created = unsafe {
                owned.create_pixmap(display, window, width, height, depth as u32, root_w, root_h)
            };
            match created {
                Ok((pixmap, bytes)) => {
                    refresh_backbuffer_gauge(bytes as u64);
                    (owned, pixmap, bytes)
                }
                Err(error) => {
                    unsafe {
                        XFreeGC(display, gc);
                        if owns_argb_colormap {
                            XFreeColormap(display, colormap);
                        }
                        XDestroyWindow(display, window);
                    }
                    return Err(format!("retained backbuffer: {error}"));
                }
            }
        };
        let (allocator, backbuffer, backbuffer_bytes) = backbuffer;
        let mut fonts = HashMap::new();
        for (bucket, name) in [
            (10u8, "6x10"),
            (13u8, "6x13"),
            (15u8, "9x15"),
            (20u8, "10x20"),
        ] {
            let name = CString::new(name).expect("static font name contains no NUL");
            let font = unsafe { XLoadQueryFont(display, name.as_ptr()) };
            if !font.is_null() {
                fonts.insert(bucket, font);
            }
        }
        if fonts.is_empty() {
            let fixed = CString::new("fixed").expect("static font name contains no NUL");
            let font = unsafe { XLoadQueryFont(display, fixed.as_ptr()) };
            if !font.is_null() {
                fonts.insert(13, font);
            }
        }
        if let Some(font) = fonts.values().next().copied() {
            unsafe { XSetFont(display, gc, (*font).fid) };
        }
        let xft = match unsafe { XftBackend::new(display, screen, backbuffer, visual, colormap) } {
            Ok(backend) => {
                eprintln!("FLAMEWM_RENDER_TEXT_BACKEND xft preferred=IBM_Plex_Sans");
                Some(backend)
            }
            Err(error) => {
                eprintln!("FLAMEWM_RENDER_TEXT_BACKEND core-x11 reason={error}");
                None
            }
        };
        let xrender = match unsafe { XRenderBackend::new(display, backbuffer, visual) } {
            Ok(backend) => {
                eprintln!("FLAMEWM_RENDER_ALPHA_BACKEND xrender");
                Some(backend)
            }
            Err(error) => {
                eprintln!("FLAMEWM_RENDER_ALPHA_BACKEND opaque-fallback reason={error}");
                None
            }
        };
        // Shape bridge is render-owned: popup/menu rounded corners. Failure is
        // explicit degraded-rectangle mode, never a silent transparency claim.
        let xshape = match unsafe { XShapeBridge::new(display) } {
            Ok(bridge) => Some(bridge),
            Err(error) => {
                eprintln!("FLAMEWM_RENDER_SHAPE_BACKEND fallback-rectangle reason={error}");
                None
            }
        };
        let wm_delete = intern_atom(display, "WM_DELETE_WINDOW");
        if wm_delete != 0 {
            let mut protocol = wm_delete;
            unsafe {
                XSetWMProtocols(display, window, &mut protocol, 1);
            }
        }
        let xdnd = config
            .file_drag
            .clone()
            .map(|slot| unsafe { xdnd::XdndState::setup(display, window, slot) });

        let mut cursors = HashMap::new();
        for kind in [
            CursorKind::Default,
            CursorKind::Pointer,
            CursorKind::Text,
            CursorKind::Move,
            CursorKind::ResizeHorizontal,
            CursorKind::ResizeVertical,
            CursorKind::ResizeNorthWestSouthEast,
            CursorKind::ResizeNorthEastSouthWest,
        ] {
            let cursor = unsafe { XCreateFontCursor(display, cursor_shape(kind)) };
            if cursor != 0 {
                cursors.insert(kind, cursor);
            }
        }
        if let Some(cursor) = cursors.get(&CursorKind::Default).copied() {
            unsafe {
                XDefineCursor(display, window, cursor);
            }
        }
        let xcursor = {
            let theme = std::env::var("XCURSOR_THEME").ok();
            let size = std::env::var("XCURSOR_SIZE")
                .ok()
                .and_then(|value| value.parse::<std::os::raw::c_int>().ok());
            let backend = unsafe { XcursorBackend::with_theme(display, theme.as_deref(), size) };
            let uses_xcursor = backend.uses_xcursor();
            let session = NativeCursorSession::wrap(backend);
            eprintln!(
                "FLAMEWM_RENDER_CURSOR_THEME theme={} size={} backend={}",
                theme.as_deref().unwrap_or("unset"),
                size.map(|size| size.to_string())
                    .unwrap_or_else(|| "unset".to_string()),
                if uses_xcursor {
                    "xcursor"
                } else {
                    "core-font-cursor"
                },
            );
            share_surface_cursor(
                SurfaceCursor {
                    session,
                    window,
                    kind: CursorKind::Default,
                },
                theme,
            )
        };
        let presentation = if initially_visible {
            PresentationState::Projected
        } else {
            PresentationState::CreatedHidden
        };

        unsafe {
            if initially_visible {
                XMapWindow(display, window);
            }
            XFlush(display);
        }
        let picture =
            u64::from(alpha_mode == SurfaceAlphaMode::CompositedArgb32 && xrender.is_some());
        let mut surface = NativeSurfaceTarget::create_plan(
            window,
            visual as u64,
            depth as u32,
            alpha_mode,
            GeometryCommit::new(width, height).map_err(|e| presenter_error(&e))?,
        );
        surface
            .bind_handles(colormap, backbuffer, gc as u64, picture)
            .map_err(|e| presenter_error(&e))?;
        // XRender backend owns the live destination picture; the plan records
        // presence (1) only when composited + xrender is live.

        Ok(Self {
            display,
            screen,
            depth,
            window,
            role: config.role,
            backbuffer,
            gc,
            colormap,
            colors: HashMap::new(),
            images: HashMap::new(),
            allocator,
            backbuffer_bytes,
            backbuffer_cap: (width, height),
            cursors,
            current_cursor: None,
            xcursor: Some(xcursor),
            fonts,
            xft,
            xrender,
            xshape,
            dock_corner_radius: 0,
            ring_mask: None,
            shape_override: None,
            visual,
            alpha_mode,
            surface: Some(surface),
            wm_delete,
            width,
            height,
            presentation,
            clip_stack: Vec::new(),
            gc_clip: None,
            range_drag: None,
            interaction: InteractionState::default(),
            layout: None,
            cached_paint: None,
            paint_nodes: Default::default(),
            cached_layout_key: None,
            shape_applied: std::cell::Cell::new(None),
            pending_damage: SurfaceDamage::Full,
            // Opt-in: the incremental path needs the Xft/clip work before it is safe for real
            // content (see the field docs). FLAMEWM_RENDER_REGION=1 enables it for evaluation.
            region_paint: std::env::var_os("FLAMEWM_RENDER_REGION").is_some_and(|v| v == "1"),
            region_force_full: false,
            region_base_valid: false,
            skip_eligible: false,
            region_text_clip: None,
            input_repaint: false,
            render_counters: damage_plan::RedrawCounters::default(),
            background_bound: 0,
            presented_size: None,
            published_desktop_frame: None,
            desktop_frame_atom: 0,
            retired_backbuffer: None,
            surface_backdrop: None,
            window_backdrop_pixmap: None,
            pending_window_resize: None,
            window_backdrop_painted: None,
            window_backdrop: config
                .backdrop
                .clone()
                .map(window_backdrop::WindowBackdropState::new),
            node_backdrops: Default::default(),
            pointer_button: None,
            pointer_grabbed: false,
            saved_input_focus: None,
            super_chord_used: false,
            last_title_press: None,
            single_event: false,
            close_requested: false,
            press_selection: RawPressSelection::new(unsafe { xi_raw_press_opcode(display) }),
            press_observed: false,
            raw_presses: Vec::new(),
            xdnd,
        })
    }

    pub(crate) fn root_extent(&self) -> (u32, u32) {
        display_extent(self.display, self.screen)
    }

    /// Transfer all dynamically loaded backend handles to the display owner.
    /// Backend X resources are released while the display is live; the typed
    /// owner must be retained until after `XCloseDisplay`.
    pub(crate) unsafe fn take_deferred_native_libraries(&mut self) -> DeferredNativeLibraryHandles {
        // The registry holds only a weak handle, so the app's strong handle is unique here.
        // A shared handle is leaked rather than dropped, keeping the cursor library alive.
        let xcursor = self
            .xcursor
            .take()
            .and_then(|live| match Rc::try_unwrap(live) {
                Ok(cell) => Some(cell.into_inner().into_session()),
                Err(shared) => {
                    std::mem::forget(shared);
                    None
                }
            });
        unsafe {
            DeferredNativeLibraryHandles::from_backends(
                xcursor,
                self.xft.take(),
                self.xrender.take(),
                self.xshape.take(),
            )
        }
    }
}

fn shape_available(display: *mut Display) -> bool {
    unsafe { XShapeBridge::new(display).is_ok() }
}

/// Root window size of `screen`, clamped to at least 1x1.
fn display_extent(display: *mut Display, screen: i32) -> (u32, u32) {
    unsafe {
        (
            XDisplayWidth(display, screen).max(1) as u32,
            XDisplayHeight(display, screen).max(1) as u32,
        )
    }
}

/// Interns a static atom name; `0` when the server refuses it.
fn intern_atom(display: *mut Display, name: &str) -> Atom {
    let name = CString::new(name).expect("static atom contains no NUL");
    unsafe { XInternAtom(display, name.as_ptr(), 0) }
}

/// Replaces `property` on `window` with `nelements` items of `format` bits (PROP_MODE_REPLACE).
///
/// # Safety
///
/// `data` must point to `nelements * format / 8` readable bytes, and `display` must be live.
unsafe fn replace_property(
    display: *mut Display,
    window: Window,
    property: Atom,
    type_: Atom,
    format: c_int,
    data: *const u8,
    nelements: c_int,
) {
    unsafe {
        XChangeProperty(
            display,
            window,
            property,
            type_,
            format,
            PROP_MODE_REPLACE,
            data,
            nelements,
        );
    }
}

#[repr(C)]
struct XRenderDirectProbe {
    red: u16,
    red_mask: u16,
    green: u16,
    green_mask: u16,
    blue: u16,
    blue_mask: u16,
    alpha: u16,
    alpha_mask: u16,
}

#[repr(C)]
struct XRenderFormatProbe {
    pict_type: i32,
    depth: i32,
    direct: XRenderDirectProbe,
    colormap: c_ulong,
}

unsafe extern "C" {
    fn dlopen(filename: *const i8, flag: c_int) -> *mut std::ffi::c_void;
    fn dlsym(handle: *mut std::ffi::c_void, symbol: *const i8) -> *mut std::ffi::c_void;
    fn dlclose(handle: *mut std::ffi::c_void) -> c_int;
}

unsafe fn visual_alpha_mask(display: *mut Display, visual: *mut Visual) -> u32 {
    // RTLD_NODELETE keeps libXrender mapped after the dlclose below. Calling
    // XRenderFindVisualFormat registers an XESetCloseDisplay hook on `display`
    // that jumps into libXrender, so unmapping it before XCloseDisplay SIGSEGVs.
    const RTLD_LAZY: c_int = 1;
    const RTLD_NODELETE: c_int = 0x1000;
    let names: [&[u8]; 2] = [b"libXrender.so.1\0", b"libXrender.so\0"];
    let sym = b"XRenderFindVisualFormat\0";
    for name in names {
        let handle = unsafe { dlopen(name.as_ptr() as *const i8, RTLD_LAZY | RTLD_NODELETE) };
        if handle.is_null() {
            continue;
        }
        type FindFn = unsafe extern "C" fn(*mut Display, *mut Visual) -> *mut XRenderFormatProbe;
        let func: FindFn = unsafe { std::mem::transmute(dlsym(handle, sym.as_ptr() as *const i8)) };
        let addr: *mut std::ffi::c_void = func as *mut std::ffi::c_void;
        if addr.is_null() {
            unsafe { dlclose(handle) };
            continue;
        }
        let format = unsafe { func(display, visual) };
        let mask = if format.is_null() {
            0
        } else {
            unsafe { u32::from((*format).direct.alpha_mask) }
        };
        unsafe { dlclose(handle) };
        return mask;
    }
    0
}

/// Dock strut owner: sets `_NET_WM_STRUT_PARTIAL` (authority) +
/// `_NET_WM_STRUT` for dock windows from a root-space panel rect, or
/// deletes both when the rect touches no root edge. Must run before the
/// first map; callers re-apply on move/resize.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_dock_strut(
    display: *mut Display,
    window: Window,
    root_w: u32,
    root_h: u32,
    edge: Option<crate::config::DockEdge>,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
) {
    let partial = intern_atom(display, "_NET_WM_STRUT_PARTIAL");
    let strut = intern_atom(display, "_NET_WM_STRUT");
    if partial == 0 || strut == 0 {
        return;
    }
    let reserved = match edge {
        Some(edge) => crate::config::DockStrut::from_edge_rect(root_w, root_h, edge, x, y, w, h),
        None => crate::config::DockStrut::from_root_and_rect(root_w, root_h, x, y, w, h),
    };
    match reserved {
        Some(reserved) => unsafe {
            let (p12, p4) = (reserved.partial12(), reserved.strut4());
            replace_property(
                display,
                window,
                partial,
                XA_CARDINAL,
                32,
                p12.as_ptr() as *const u8,
                12,
            );
            replace_property(
                display,
                window,
                strut,
                XA_CARDINAL,
                32,
                p4.as_ptr() as *const u8,
                4,
            );
        },
        None => unsafe {
            XDeleteProperty(display, window, partial);
            XDeleteProperty(display, window, strut);
        },
    }
}

/// Publishes the pager cell rectangles on a panel window as the
/// `FLAMEWM_PAGER_CELLS` CARDINAL list: four values (x, y, width, height)
/// per cell, window-relative, in cell order. Empty input deletes it.
pub(crate) fn apply_pager_cells(display: *mut Display, window: Window, cells: &[[u64; 4]]) {
    let property = intern_atom(display, "FLAMEWM_PAGER_CELLS");
    if property == 0 {
        return;
    }
    if cells.is_empty() {
        unsafe { XDeleteProperty(display, window, property) };
        return;
    }
    let flat: Vec<u64> = cells.iter().flat_map(|cell| cell.iter().copied()).collect();
    let data = flat.as_ptr() as *const u8;
    unsafe {
        replace_property(
            display,
            window,
            property,
            XA_CARDINAL,
            32,
            data,
            flat.len() as c_int,
        )
    };
}

/// Writes (`Some`) or deletes (`None`) `_NET_WM_ICON_GEOMETRY` on a client
/// window. `rect` is root-space `[x, y, width, height]` as 4 CARDINAL values.
/// A vanished client raises BadWindow, which the process protocol handler
/// ignores, so this never aborts the shell.
pub(crate) fn apply_window_icon_geometry(
    display: *mut Display,
    window: Window,
    property: Atom,
    rect: Option<[u64; 4]>,
) {
    if property == 0 {
        return;
    }
    match rect {
        Some(values) => unsafe {
            replace_property(
                display,
                window,
                property,
                XA_CARDINAL,
                32,
                values.as_ptr() as *const u8,
                values.len() as c_int,
            )
        },
        None => unsafe {
            XDeleteProperty(display, window, property);
        },
    }
}

/// ICCCM WM_CLASS (instance\0class\0, STRING) and EWMH `_NET_WM_NAME`
/// (UTF8_STRING). WM_CLASS is written only when an identity is configured.
fn set_window_identity(
    display: *mut Display,
    window: Window,
    title: &str,
    identity: Option<&super::config::X11WindowIdentity>,
) {
    const XA_STRING: Atom = 31;
    if let Some(identity) = identity {
        let wm_class = intern_atom(display, "WM_CLASS");
        if wm_class != 0 {
            let mut bytes = Vec::with_capacity(identity.instance.len() + identity.class.len() + 2);
            bytes.extend_from_slice(identity.instance.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(identity.class.as_bytes());
            bytes.push(0);
            unsafe {
                replace_property(
                    display,
                    window,
                    wm_class,
                    XA_STRING,
                    8,
                    bytes.as_ptr(),
                    bytes.len() as c_int,
                )
            };
        }
    }
    let net_wm_name = intern_atom(display, "_NET_WM_NAME");
    let utf8_string = intern_atom(display, "UTF8_STRING");
    if net_wm_name != 0 && utf8_string != 0 {
        unsafe {
            replace_property(
                display,
                window,
                net_wm_name,
                utf8_string,
                8,
                title.as_ptr(),
                title.len() as c_int,
            )
        };
    }
}

fn set_window_role(display: *mut Display, window: Window, role: X11WindowRole) {
    let role_name = crate::config::overlay_window_type_name(match role {
        X11WindowRole::Normal => crate::config::SurfaceRole::Normal,
        X11WindowRole::Desktop => crate::config::SurfaceRole::Desktop,
        X11WindowRole::Dock => crate::config::SurfaceRole::Dock,
        X11WindowRole::PopupMenu => crate::config::SurfaceRole::PopupMenu,
        X11WindowRole::DropdownMenu => crate::config::SurfaceRole::DropdownMenu,
        X11WindowRole::Overlay => crate::config::SurfaceRole::Overlay,
    });
    let property = intern_atom(display, "_NET_WM_WINDOW_TYPE");
    let role_atom = intern_atom(display, role_name);
    if property != 0 && role_atom != 0 {
        unsafe {
            replace_property(
                display,
                window,
                property,
                XA_ATOM,
                32,
                &role_atom as *const Atom as *const u8,
                1,
            )
        };
    }
}

/// A desktop frame published on the root: `(backbuffer, width, height)`, the window's root origin
/// and the present serial.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PublishedDesktopFrame {
    pub(crate) frame: (Pixmap, u32, u32),
    pub(crate) origin: (i32, i32),
    pub(crate) serial: u32,
}

impl Drop for X11App {
    fn drop(&mut self) {
        if x_io_broken() {
            // Broken-connection backstop mirrors SurfaceController::drop:
            // leak X-backed backends instead of issuing protocol calls
            // on a dead Display. The normal path below is unchanged.
            let xft = self.xft.take();
            let xrender = self.xrender.take();
            std::mem::forget(xft);
            std::mem::forget(xrender);
            return;
        }
        // Release backend-owned X resources before core X resources. Normal
        // display owners call `take_deferred_native_libraries` first and keep
        // its dynamic handles alive through XCloseDisplay.
        let _deferred = unsafe { self.take_deferred_native_libraries() };
        if self.published_desktop_frame.is_some() {
            unsafe { self.withdraw_desktop_frame() };
        }
        self.clear_surface_backdrop();
        self.clear_window_backdrop();
        self.clear_node_backdrops();
        unsafe {
            if self.pointer_grabbed {
                XUngrabPointer(self.display, CURRENT_TIME);
                crate::surface_controller::popup_restore_focus(self.display, self);
                self.pointer_grabbed = false;
            }
            for image in self.images.values().copied() {
                free_cached_image(self.display, &mut self.allocator, image);
            }
            refresh_image_gauge(0);
            for cursor in self.cursors.values().copied() {
                XFreeCursor(self.display, cursor);
            }
            for font in self.fonts.values().copied() {
                XFreeFont(self.display, font);
            }
            if self.backbuffer != 0 {
                self.allocator
                    .free_pixmap(self.display, self.backbuffer, self.backbuffer_bytes);
                refresh_backbuffer_gauge(0);
                self.backbuffer = 0;
            }
            if let Some((pixmap, bytes)) = self.retired_backbuffer.take() {
                if pixmap != 0 && pixmap != self.background_bound {
                    self.allocator.free_pixmap(self.display, pixmap, bytes);
                }
            }
            if !self.gc.is_null() {
                XFreeGC(self.display, self.gc);
            }
            if self.window != 0 {
                XDestroyWindow(self.display, self.window);
            }
        }
    }
}
