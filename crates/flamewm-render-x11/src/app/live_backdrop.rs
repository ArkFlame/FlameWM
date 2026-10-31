//! Live window backdrop: the desktop and the windows stacked behind a translucent window,
//! composed on the X server.
//!
//! The WM redirects its frames automatically through Composite (they still show on screen as
//! usual) and lists them on the root ([`crate::backdrop::REDIRECTED_FRAMES_PROPERTY`]), so each
//! frame's contents stay valid off screen even where other windows cover it. The root is never
//! read: the backdrop is the published desktop frame plus the redirected frames below this
//! window, scaled down and composited by XRender into a small pixmap. Only that small image is
//! read back, blurred and upscaled, so a refresh moves about 1/16 of the window's pixels.
//!
//! Windows above this one are never part of it, and every frame below is read as it is now, so
//! a window that moves away or closes leaves nothing behind. Refreshes follow root stacking and
//! geometry changes and XDamage on the frames below, at most every [`LIVE_INTERVAL`].

use std::collections::HashMap;
use std::os::raw::{c_int, c_ulong};
use std::time::{Duration, Instant};

use crate::backdrop::{BLUR_DOWNSCALE, BlurredCanvas, blur_in_place_slice, blurred_argb_words};
use crate::ffi::dynamic_library::DynamicLibrary;
use crate::xlib::*;
use crate::xlib_ext::{IS_VIEWABLE, XGetWindowAttributes, XQueryTree, XWindowAttributes};
use crate::xrender_transform::{PICT_STANDARD_RGB_24, XRenderTransformApi};

/// Shortest spacing between two live refreshes while what lies behind keeps changing.
pub(crate) const LIVE_INTERVAL: Duration = Duration::from_millis(50);

/// `DamageReportNonEmpty`: one notify per damaged frame until the damage is subtracted.
const DAMAGE_REPORT_NON_EMPTY: c_int = 3;
const SUBSTRUCTURE_NOTIFY_MASK: std::os::raw::c_long = 1 << 19;
const DESTROY_NOTIFY: c_int = 17;
const UNMAP_NOTIFY: c_int = 18;
const REPARENT_NOTIFY: c_int = 21;
const GRAVITY_NOTIFY: c_int = 24;
const CIRCULATE_NOTIFY: c_int = 26;
/// Flat layer used where neither the desktop frame nor a frame covers the window.
const FALLBACK_RGB: (u16, u16, u16) = (0x2323, 0x2626, 0x2929);

type NameWindowPixmapFn = unsafe extern "C" fn(*mut Display, Window) -> Pixmap;
type DamageQueryExtensionFn = unsafe extern "C" fn(*mut Display, *mut c_int, *mut c_int) -> Bool;
type DamageCreateFn = unsafe extern "C" fn(*mut Display, Drawable, c_int) -> c_ulong;
type DamageDestroyFn = unsafe extern "C" fn(*mut Display, c_ulong);
type DamageSubtractFn = unsafe extern "C" fn(*mut Display, c_ulong, c_ulong, c_ulong);

/// Composite and Damage entry points. The libraries stay loaded for the process: their close-
/// display hooks must outlive every display they touched.
struct Extensions {
    name_window_pixmap: NameWindowPixmapFn,
    damage_create: DamageCreateFn,
    damage_destroy: DamageDestroyFn,
    damage_subtract: DamageSubtractFn,
    damage_event_base: c_int,
}

impl Extensions {
    /// # Safety
    ///
    /// `display` must be live.
    unsafe fn load(display: *mut Display) -> Result<Self, String> {
        let composite = DynamicLibrary::open(&["libXcomposite.so.1", "libXcomposite.so"])?;
        let damage = DynamicLibrary::open(&["libXdamage.so.1", "libXdamage.so"])?;
        // SAFETY: static NUL-terminated names; the types match the C ABI of each symbol.
        let extensions = unsafe {
            let query: DamageQueryExtensionFn = damage.symbol(b"XDamageQueryExtension\0")?;
            let (mut event_base, mut error_base) = (0, 0);
            if query(display, &mut event_base, &mut error_base) == 0 {
                return Err("XDamage extension not present".to_string());
            }
            Self {
                name_window_pixmap: composite.symbol(b"XCompositeNameWindowPixmap\0")?,
                damage_create: damage.symbol(b"XDamageCreate\0")?,
                damage_destroy: damage.symbol(b"XDamageDestroy\0")?,
                damage_subtract: damage.symbol(b"XDamageSubtract\0")?,
                damage_event_base: event_base,
            }
        };
        std::mem::forget(composite);
        std::mem::forget(damage);
        Ok(extensions)
    }
}

/// A frame's named off-screen pixmap and the picture reading it, valid for one frame size.
struct NamedFrame {
    size: (u32, u32),
    pixmap: Pixmap,
    picture: c_ulong,
}

/// The small render target the scene is composed into.
struct Target {
    size: (u32, u32),
    pixmap: Pixmap,
    picture: c_ulong,
}

/// One frame behind the window this refresh: its outer root rect.
#[derive(Clone, Copy)]
struct Behind {
    window: Window,
    rect: (i32, i32, u32, u32),
}

/// Live backdrop state of one translucent window.
pub(crate) struct LiveScene {
    api: XRenderTransformApi,
    extensions: Extensions,
    root: Window,
    frames_atom: Atom,
    /// This window's top-level ancestor (its WM frame); 0 until found.
    own_top: Window,
    target: Option<Target>,
    named: HashMap<Window, NamedFrame>,
    damages: HashMap<Window, c_ulong>,
    /// Picture on the published desktop frame pixmap.
    desktop: Option<(Pixmap, c_ulong)>,
    /// Root rects of the frames composed by the last refresh, to filter root events.
    behind: Vec<Behind>,
    /// Redirected frames listed at the last refresh. Root events of other windows (the move
    /// outline, popups) never change the backdrop and are ignored.
    frames: Vec<Window>,
    /// Root rect the last refresh was composed for.
    rect: (i32, i32, u32, u32),
    pub(crate) last_refresh: Option<Instant>,
}

/// Whether two root rects overlap.
fn overlaps(a: (i32, i32, u32, u32), b: (i32, i32, u32, u32)) -> bool {
    let (ax1, ay1) = (
        i64::from(a.0) + i64::from(a.2),
        i64::from(a.1) + i64::from(a.3),
    );
    let (bx1, by1) = (
        i64::from(b.0) + i64::from(b.2),
        i64::from(b.1) + i64::from(b.3),
    );
    i64::from(a.0) < bx1 && i64::from(b.0) < ax1 && i64::from(a.1) < by1 && i64::from(b.1) < ay1
}

/// Small-target rect `(x, y, w, h)` covering the part of root rect `area` inside `rect`, at
/// `factor` root pixels per target pixel. `None` when they do not overlap.
fn target_rect(
    rect: (i32, i32, u32, u32),
    area: (i32, i32, u32, u32),
    factor: u32,
    target: (u32, u32),
) -> Option<(i32, i32, u32, u32)> {
    if !overlaps(rect, area) {
        return None;
    }
    let f = i64::from(factor);
    let x0 = (i64::from(area.0.max(rect.0)) - i64::from(rect.0)).div_euclid(f);
    let y0 = (i64::from(area.1.max(rect.1)) - i64::from(rect.1)).div_euclid(f);
    let right = (i64::from(area.0) + i64::from(area.2)).min(i64::from(rect.0) + i64::from(rect.2));
    let bottom = (i64::from(area.1) + i64::from(area.3)).min(i64::from(rect.1) + i64::from(rect.3));
    let x1 = ((right - i64::from(rect.0)) + f - 1)
        .div_euclid(f)
        .min(i64::from(target.0));
    let y1 = ((bottom - i64::from(rect.1)) + f - 1)
        .div_euclid(f)
        .min(i64::from(target.1));
    (x1 > x0 && y1 > y0).then_some((x0 as i32, y0 as i32, (x1 - x0) as u32, (y1 - y0) as u32))
}

/// Whether `event` belongs to the live backdrop (a frame below changed its contents, or a root
/// child was mapped, unmapped, moved or restacked). Returns `(consumed, relevant)`: consumed
/// events never reach the window's own handlers; relevant ones need a refresh.
///
/// Every event addressed to the root is consumed. Selecting SubstructureNotify on the root also
/// delivers every client message other clients send there (EWMH `_NET_ACTIVE_WINDOW`,
/// `_NET_WM_STATE`, `_NET_WM_MOVERESIZE`, ...); none of them is for this window, and an event left
/// unconsumed here is one the window's own handlers would misread as theirs.
fn classify_event(
    event: &XEvent,
    root: Window,
    damage_event_base: c_int,
    own_top: Window,
    frames: &[Window],
    behind: &[Behind],
    rect: (i32, i32, u32, u32),
) -> (bool, bool) {
    // SAFETY: type_ is the common discriminator at the start of every XEvent variant.
    let kind = unsafe { event.type_ };
    if kind == damage_event_base {
        return (true, true);
    }
    // SAFETY: xany is the common prefix of every event.
    if unsafe { event.xany.window } != root {
        return (false, false);
    }
    match kind {
        CONFIGURE_NOTIFY => {
            // SAFETY: the discriminator selects XConfigureEvent.
            let configure = unsafe { event.xconfigure };
            if configure.window == own_top || !frames.contains(&configure.window) {
                return (true, false);
            }
            let moved = (
                configure.x,
                configure.y,
                (configure.width + 2 * configure.border_width).max(0) as u32,
                (configure.height + 2 * configure.border_width).max(0) as u32,
            );
            let was_behind = behind.iter().any(|b| b.window == configure.window);
            (true, was_behind || overlaps(moved, rect))
        }
        // A frame mapped for the first time is not listed yet, so every map counts.
        MAP_NOTIFY => (true, true),
        UNMAP_NOTIFY | DESTROY_NOTIFY | REPARENT_NOTIFY | GRAVITY_NOTIFY | CIRCULATE_NOTIFY => {
            // SAFETY: Unmap, Destroy, Reparent, Gravity and Circulate events share
            // XConfigureEvent's prefix (`type`, `serial`, `send_event`, `display`, `event`, `window`).
            let window = unsafe { event.xconfigure.window };
            (true, frames.contains(&window))
        }
        // CreateNotify (an unmapped child shows nothing), client messages and anything else.
        _ => (true, false),
    }
}

/// Children of `window` in stacking order (bottom first) and its parent.
///
/// # Safety
///
/// `display` must be live.
unsafe fn query_tree(display: *mut Display, window: Window) -> (Vec<Window>, Window) {
    let (mut root, mut parent, mut children, mut count) = (0, 0, std::ptr::null_mut(), 0);
    // SAFETY: live display; out-pointers are valid locals; the list is freed below.
    let ok = unsafe {
        XQueryTree(
            display,
            window,
            &mut root,
            &mut parent,
            &mut children,
            &mut count,
        )
    };
    let list = if ok != 0 && !children.is_null() {
        // SAFETY: XQueryTree returned `count` windows at `children`.
        unsafe { std::slice::from_raw_parts(children, count as usize) }.to_vec()
    } else {
        Vec::new()
    };
    if !children.is_null() {
        unsafe { XFree(children.cast()) };
    }
    (list, parent)
}

impl LiveScene {
    /// Prepares the live backdrop and subscribes to root stacking changes. `None` when the
    /// Composite, Damage or XRender transform support is missing; the caller keeps the desktop
    /// layer then.
    ///
    /// # Safety
    ///
    /// `display` must be live and outlive the scene.
    pub(crate) unsafe fn new(display: *mut Display, screen: c_int) -> Option<Self> {
        let api = XRenderTransformApi::load().ok()?;
        if !api.capabilities().picture_transform {
            return None;
        }
        let extensions = unsafe { Extensions::load(display) }.ok()?;
        let root = unsafe { XRootWindow(display, screen) };
        let name = std::ffi::CString::new(crate::backdrop::REDIRECTED_FRAMES_PROPERTY).ok()?;
        let frames_atom = unsafe { XInternAtom(display, name.as_ptr(), 0) };
        // SAFETY: live display; SubstructureNotify on the root is shared by every client.
        unsafe { XSelectInput(display, root, SUBSTRUCTURE_NOTIFY_MASK) };
        Some(Self {
            api,
            extensions,
            root,
            frames_atom,
            own_top: 0,
            target: None,
            named: HashMap::new(),
            damages: HashMap::new(),
            desktop: None,
            behind: Vec::new(),
            frames: Vec::new(),
            rect: (0, 0, 0, 0),
            last_refresh: None,
        })
    }

    /// Whether `event` belongs to the live backdrop. See [`classify_event`].
    pub(crate) fn classify(&self, event: &XEvent) -> (bool, bool) {
        classify_event(
            event,
            self.root,
            self.extensions.damage_event_base,
            self.own_top,
            &self.frames,
            &self.behind,
            self.rect,
        )
    }

    /// Composes what lies behind `window` at root `rect` and returns it as opaque ARGB32 words of
    /// the window's size, blurred per `blur_percent`. `None` when the WM publishes no redirected
    /// frames (the caller falls back to the desktop layer).
    ///
    /// # Safety
    ///
    /// `display` must be live; `window` is this app's window on it.
    pub(crate) unsafe fn compose(
        &mut self,
        display: *mut Display,
        screen: c_int,
        window: Window,
        rect: (i32, i32, u32, u32),
        blur_percent: u8,
    ) -> Option<Vec<u32>> {
        let frames = unsafe { self.redirected_frames(display) }?;
        let (width, height) = (rect.2, rect.3);
        if width == 0 || height == 0 {
            return None;
        }
        let radius = crate::backdrop::blur_radius_px(0, blur_percent);
        let factor = if radius >= 2.0 * BLUR_DOWNSCALE as f32 {
            BLUR_DOWNSCALE
        } else {
            1
        };
        let size = (width.div_ceil(factor), height.div_ceil(factor));
        let target = unsafe { self.ensure_target(display, screen, size) }?;
        self.rect = rect;
        self.last_refresh = Some(Instant::now());
        // SAFETY: every picture below lives on `display`; solid fills are freed after use.
        unsafe {
            if let Ok(fill) = self.api.create_solid_fill(
                display,
                FALLBACK_RGB.0,
                FALLBACK_RGB.1,
                FALLBACK_RGB.2,
                0xffff,
            ) {
                self.api
                    .composite_src(display, fill, target, (0, 0, size.0, size.1));
                self.api.free_picture(display, fill);
            }
            self.compose_desktop(display, target, rect, factor, size);
        }
        let behind = unsafe { self.frames_behind(display, window, &frames, rect) };
        for frame in &behind {
            unsafe { self.compose_frame(display, target, *frame, rect, factor, size) };
        }
        unsafe { self.track_damage(display, &behind) };
        self.behind = behind;
        self.frames = frames;
        let pixmap = self.target.as_ref()?.pixmap;
        let small =
            crate::backdrop::capture_drawable_rect(display, pixmap as u64, (0, 0, size.0, size.1))
                .ok()?;
        if factor == 1 {
            return Some(blurred_argb_words(
                width,
                height,
                small.pixels,
                0,
                blur_percent,
            ));
        }
        let mut pixels = small.pixels;
        blur_in_place_slice(&mut pixels, size.0, size.1, radius / factor as f32);
        Some(BlurredCanvas::from_scaled(width, height, pixels)?.crop((0, 0), (width, height)))
    }

    /// Frames the WM redirects, from the root property; `None` without one.
    unsafe fn redirected_frames(&self, display: *mut Display) -> Option<Vec<Window>> {
        let (mut actual_type, mut actual_format, mut items, mut after) = (0, 0, 0, 0);
        let mut data: *mut u8 = std::ptr::null_mut();
        // SAFETY: out-pointers reference live locals; Xlib allocates `data`, freed below.
        let status = unsafe {
            crate::xlib_ext::XGetWindowProperty(
                display,
                self.root,
                self.frames_atom,
                0,
                4096,
                0,
                XA_WINDOW,
                &mut actual_type,
                &mut actual_format,
                &mut items,
                &mut after,
                &mut data,
            )
        };
        if status != 0 || data.is_null() {
            return None;
        }
        let frames = (actual_format == 32).then(|| {
            // SAFETY: format-32 properties come back as `items` C longs at `data`.
            unsafe {
                std::slice::from_raw_parts(data.cast::<std::os::raw::c_long>(), items as usize)
            }
            .iter()
            .map(|&window| window as Window)
            .collect()
        });
        unsafe { XFree(data.cast()) };
        frames
    }

    unsafe fn ensure_target(
        &mut self,
        display: *mut Display,
        screen: c_int,
        size: (u32, u32),
    ) -> Option<c_ulong> {
        if let Some(target) = self.target.as_ref().filter(|target| target.size == size) {
            return Some(target.picture);
        }
        unsafe { self.free_target(display) };
        let depth = unsafe { XDefaultDepth(display, screen) } as u32;
        if depth != 24 {
            return None;
        }
        let pixmap = unsafe { XCreatePixmap(display, self.root, size.0, size.1, depth) };
        if pixmap == 0 {
            return None;
        }
        let picture = match unsafe {
            self.api
                .create_standard_picture(display, pixmap, PICT_STANDARD_RGB_24)
        } {
            Ok(picture) => picture,
            Err(_) => {
                unsafe { crate::native::xresource::free_pixmap_raw(display, pixmap) };
                return None;
            }
        };
        self.target = Some(Target {
            size,
            pixmap,
            picture,
        });
        Some(picture)
    }

    /// Scales the source picture `picture` (root pixel `origin` at its (0, 0)) into the target.
    unsafe fn scale_into(
        &self,
        display: *mut Display,
        picture: c_ulong,
        target: c_ulong,
        offset: (i32, i32),
        factor: u32,
        dest: (i32, i32, u32, u32),
    ) {
        let f = f64::from(factor);
        let matrix = [
            [f, 0.0, f64::from(offset.0)],
            [0.0, f, f64::from(offset.1)],
            [0.0, 0.0, 1.0],
        ];
        unsafe {
            if self.api.set_transform(display, picture, matrix).is_err() {
                return;
            }
            if factor > 1 {
                let _ = self.api.set_bilinear_filter(display, picture);
            }
            self.api.composite_src(display, picture, target, dest);
        }
    }

    unsafe fn compose_desktop(
        &mut self,
        display: *mut Display,
        target: c_ulong,
        rect: (i32, i32, u32, u32),
        factor: u32,
        size: (u32, u32),
    ) {
        let Some(frame) = (unsafe { crate::backdrop::published_desktop_frame(display) }) else {
            return;
        };
        if self
            .desktop
            .is_none_or(|(pixmap, _)| pixmap != frame.pixmap)
        {
            if let Some((_, picture)) = self.desktop.take() {
                unsafe { self.api.free_picture(display, picture) };
            }
            let (mut root, mut x, mut y, mut w, mut h, mut border, mut depth) =
                (0, 0, 0, 0, 0, 0, 0);
            // SAFETY: live display; a pixmap freed since its publish fails here.
            let ok = unsafe {
                crate::xlib_ext::XGetGeometry(
                    display,
                    frame.pixmap,
                    &mut root,
                    &mut x,
                    &mut y,
                    &mut w,
                    &mut h,
                    &mut border,
                    &mut depth,
                )
            };
            let format = match depth {
                24 => PICT_STANDARD_RGB_24,
                32 => crate::xrender_transform::PICT_STANDARD_ARGB_32,
                _ => return,
            };
            if ok == 0 {
                return;
            }
            let Ok(picture) = (unsafe {
                self.api
                    .create_standard_picture(display, frame.pixmap, format)
            }) else {
                return;
            };
            self.desktop = Some((frame.pixmap, picture));
        }
        let Some((_, picture)) = self.desktop else {
            return;
        };
        unsafe {
            self.scale_into(
                display,
                picture,
                target,
                (rect.0 - frame.x, rect.1 - frame.y),
                factor,
                (0, 0, size.0, size.1),
            );
        }
    }

    /// Viewable redirected frames stacked below `window`'s top-level ancestor that overlap
    /// `rect`, bottom first.
    unsafe fn frames_behind(
        &mut self,
        display: *mut Display,
        window: Window,
        frames: &[Window],
        rect: (i32, i32, u32, u32),
    ) -> Vec<Behind> {
        let (stack, _) = unsafe { query_tree(display, self.root) };
        if !stack.contains(&self.own_top) {
            self.own_top = window;
            loop {
                let (_, parent) = unsafe { query_tree(display, self.own_top) };
                if parent == 0 || parent == self.root {
                    break;
                }
                self.own_top = parent;
            }
        }
        let Some(position) = stack.iter().position(|w| *w == self.own_top) else {
            return Vec::new();
        };
        let mut behind = Vec::new();
        for &candidate in stack[..position].iter().filter(|w| frames.contains(w)) {
            let mut attributes = std::mem::MaybeUninit::<XWindowAttributes>::zeroed();
            // SAFETY: XGetWindowAttributes fills the struct on success; zeroed is a valid fallback.
            if unsafe { XGetWindowAttributes(display, candidate, attributes.as_mut_ptr()) } == 0 {
                continue;
            }
            // SAFETY: zero-initialised and possibly filled above.
            let attributes = unsafe { attributes.assume_init() };
            if attributes.map_state != IS_VIEWABLE || attributes.depth != 24 {
                continue;
            }
            let outer = (
                attributes.x,
                attributes.y,
                (attributes.width + 2 * attributes.border_width).max(0) as u32,
                (attributes.height + 2 * attributes.border_width).max(0) as u32,
            );
            if overlaps(outer, rect) {
                behind.push(Behind {
                    window: candidate,
                    rect: outer,
                });
            }
        }
        behind
    }

    unsafe fn compose_frame(
        &mut self,
        display: *mut Display,
        target: c_ulong,
        frame: Behind,
        rect: (i32, i32, u32, u32),
        factor: u32,
        size: (u32, u32),
    ) {
        let Some(dest) = target_rect(rect, frame.rect, factor, size) else {
            return;
        };
        let frame_size = (frame.rect.2, frame.rect.3);
        if self
            .named
            .get(&frame.window)
            .is_none_or(|named| named.size != frame_size)
        {
            if let Some(old) = self.named.remove(&frame.window) {
                unsafe { self.free_named(display, old) };
            }
            // SAFETY: the WM redirects every listed frame, so naming its pixmap is valid; a frame
            // destroyed meanwhile yields an X error that the protocol handler ignores.
            let pixmap = unsafe { (self.extensions.name_window_pixmap)(display, frame.window) };
            if pixmap == 0 {
                return;
            }
            let Ok(picture) = (unsafe {
                self.api
                    .create_standard_picture(display, pixmap, PICT_STANDARD_RGB_24)
            }) else {
                unsafe { crate::native::xresource::free_pixmap_raw(display, pixmap) };
                return;
            };
            self.named.insert(
                frame.window,
                NamedFrame {
                    size: frame_size,
                    pixmap,
                    picture,
                },
            );
        }
        let Some(picture) = self.named.get(&frame.window).map(|named| named.picture) else {
            return;
        };
        unsafe {
            self.scale_into(
                display,
                picture,
                target,
                (rect.0 - frame.rect.0, rect.1 - frame.rect.1),
                factor,
                dest,
            );
        }
    }

    /// Watches exactly the frames composed this refresh, re-armed for the next change.
    unsafe fn track_damage(&mut self, display: *mut Display, behind: &[Behind]) {
        let gone: Vec<Window> = self
            .damages
            .keys()
            .copied()
            .filter(|window| !behind.iter().any(|b| b.window == *window))
            .collect();
        for window in gone {
            if let Some(damage) = self.damages.remove(&window) {
                unsafe { (self.extensions.damage_destroy)(display, damage) };
            }
            if let Some(named) = self.named.remove(&window) {
                unsafe { self.free_named(display, named) };
            }
        }
        for frame in behind {
            match self.damages.get(&frame.window) {
                Some(&damage) => unsafe {
                    (self.extensions.damage_subtract)(display, damage, 0, 0)
                },
                None => {
                    let damage = unsafe {
                        (self.extensions.damage_create)(
                            display,
                            frame.window,
                            DAMAGE_REPORT_NON_EMPTY,
                        )
                    };
                    if damage != 0 {
                        self.damages.insert(frame.window, damage);
                    }
                }
            }
        }
    }

    unsafe fn free_named(&self, display: *mut Display, named: NamedFrame) {
        unsafe {
            self.api.free_picture(display, named.picture);
            crate::native::xresource::free_pixmap_raw(display, named.pixmap);
        }
    }

    unsafe fn free_target(&mut self, display: *mut Display) {
        if let Some(target) = self.target.take() {
            unsafe {
                self.api.free_picture(display, target.picture);
                crate::native::xresource::free_pixmap_raw(display, target.pixmap);
            }
        }
    }

    /// Releases every X resource the scene created. Idempotent.
    ///
    /// # Safety
    ///
    /// `display` must be live.
    pub(crate) unsafe fn release(&mut self, display: *mut Display) {
        unsafe {
            self.free_target(display);
            for (_, named) in std::mem::take(&mut self.named) {
                self.free_named(display, named);
            }
            for (_, damage) in std::mem::take(&mut self.damages) {
                (self.extensions.damage_destroy)(display, damage);
            }
            if let Some((_, picture)) = self.desktop.take() {
                self.api.free_picture(display, picture);
            }
            // SAFETY: live display; this scene made the only root selection of this client.
            XSelectInput(display, self.root, 0);
        }
        self.behind.clear();
    }
}

impl std::fmt::Debug for LiveScene {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveScene")
            .field("frames_behind", &self.behind.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_rect_covers_the_overlap_at_the_scale() {
        let window = (100, 50, 400, 200);
        assert_eq!(
            target_rect(window, (0, 0, 1000, 1000), 4, (100, 50)),
            Some((0, 0, 100, 50))
        );
        assert_eq!(
            target_rect(window, (300, 100, 50, 50), 4, (100, 50)),
            Some((50, 12, 13, 13))
        );
        assert_eq!(target_rect(window, (600, 0, 10, 10), 4, (100, 50)), None);
        assert_eq!(
            target_rect(window, (90, 40, 20, 20), 1, (400, 200)),
            Some((0, 0, 10, 10))
        );
    }

    const ROOT: Window = 0x2cf;
    const DAMAGE_BASE: c_int = 91;
    const PROPERTY_NOTIFY: c_int = 28;

    fn root_event(kind: c_int, window: Window) -> XEvent {
        // SAFETY: an all-zero XEvent is a valid value of every variant.
        let mut event: XEvent = unsafe { std::mem::zeroed() };
        let configure = XConfigureEvent {
            type_: kind,
            serial: 0,
            send_event: 0,
            display: std::ptr::null_mut(),
            event: ROOT,
            window,
            x: 0,
            y: 0,
            width: 10,
            height: 10,
            border_width: 0,
            above: 0,
            override_redirect: 0,
        };
        event.xconfigure = configure;
        event
    }

    fn classify_root(kind: c_int, window: Window) -> (bool, bool) {
        classify_event(
            &root_event(kind, window),
            ROOT,
            DAMAGE_BASE,
            0x500,
            &[0x400],
            &[],
            (0, 0, 100, 100),
        )
    }

    #[test]
    fn every_event_addressed_to_the_root_is_consumed() {
        for kind in 2..=35 {
            assert!(
                classify_root(kind, 0x777).0,
                "root event type {kind} must never reach the window's handlers"
            );
        }
        assert_eq!(classify_root(CLIENT_MESSAGE, 0x777), (true, false));
        assert_eq!(classify_root(PROPERTY_NOTIFY, 0x777), (true, false));
    }

    #[test]
    fn frame_changes_behind_refresh_and_others_do_not() {
        assert_eq!(classify_root(MAP_NOTIFY, 0x777), (true, true));
        assert_eq!(classify_root(UNMAP_NOTIFY, 0x400), (true, true));
        assert_eq!(classify_root(UNMAP_NOTIFY, 0x777), (true, false));
        assert_eq!(classify_root(CONFIGURE_NOTIFY, 0x400), (true, true));
        assert_eq!(classify_root(CONFIGURE_NOTIFY, 0x500), (true, false));
    }

    #[test]
    fn damage_counts_and_events_of_other_windows_pass_through() {
        let damage = root_event(DAMAGE_BASE, 0x400);
        assert_eq!(
            classify_event(&damage, ROOT, DAMAGE_BASE, 0, &[], &[], (0, 0, 1, 1)),
            (true, true)
        );
        let mut own = root_event(CLIENT_MESSAGE, 0);
        own.xany.window = 0x500;
        assert_eq!(
            classify_event(&own, ROOT, DAMAGE_BASE, 0, &[], &[], (0, 0, 1, 1)),
            (false, false)
        );
    }

    #[test]
    fn overlap_excludes_touching_edges() {
        assert!(overlaps((0, 0, 10, 10), (9, 9, 5, 5)));
        assert!(!overlaps((0, 0, 10, 10), (10, 0, 5, 5)));
        assert!(!overlaps((0, 0, 10, 10), (0, 10, 5, 5)));
    }
}
