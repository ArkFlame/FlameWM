#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(dead_code)]

use std::os::raw::{c_char, c_int, c_long, c_short, c_uint, c_ulong, c_ushort, c_void};

#[repr(C)]
pub struct Display {
    _private: [u8; 0],
}

#[repr(C)]
pub struct Visual {
    _private: [u8; 0],
}

#[repr(C)]
pub struct _XGC {
    _private: [u8; 0],
}

pub type GC = *mut _XGC;
pub type Window = c_ulong;
pub type Drawable = c_ulong;
pub type Pixmap = c_ulong;
pub type Cursor = c_ulong;
pub type Colormap = c_ulong;
pub type Atom = c_ulong;
pub type Font = c_ulong;
pub type Time = c_ulong;
pub type Bool = c_int;
pub type Status = c_int;

#[repr(C)]
pub struct XVisualInfo {
    pub visual: *mut Visual,
    pub visualid: c_ulong,
    pub screen: c_int,
    pub depth: c_int,
    pub class: c_int,
    pub red_mask: c_ulong,
    pub green_mask: c_ulong,
    pub blue_mask: c_ulong,
    pub colormap_size: c_int,
    pub bits_per_rgb: c_int,
}

pub const TRUE_COLOR_CLASS: c_int = 4;
pub const INPUT_OUTPUT_CLASS: c_uint = 1;
pub const ALLOC_NONE: c_int = 0;
pub const TRANSPARENT_CLEAR_PIXEL: c_ulong = 0;
pub const CW_BACK_PIXMAP: c_ulong = 1 << 0;
pub const CW_BACK_PIXEL: c_ulong = 1 << 1;
pub const CW_BORDER_PIXMAP: c_ulong = 1 << 2;
pub const CW_COLORMAP: c_ulong = 1 << 13;

#[repr(C)]
pub struct XFontStructHead {
    pub ext_data: *mut c_void,
    pub fid: Font,
}

#[repr(C)]
pub struct XImageHead {
    pub width: c_int,
    pub height: c_int,
    pub xoffset: c_int,
    pub format: c_int,
    pub data: *mut c_char,
    pub byte_order: c_int,
    pub bitmap_unit: c_int,
    pub bitmap_bit_order: c_int,
    pub bitmap_pad: c_int,
    pub depth: c_int,
    pub bytes_per_line: c_int,
    pub bits_per_pixel: c_int,
    pub red_mask: c_ulong,
    pub green_mask: c_ulong,
    pub blue_mask: c_ulong,
}

#[repr(C)]
pub struct XImage {
    _private: [u8; 0],
}

#[repr(C)]
pub struct XSetWindowAttributes {
    pub background_pixmap: Pixmap,
    pub background_pixel: c_ulong,
    pub border_pixmap: Pixmap,
    pub border_pixel: c_ulong,
    pub bit_gravity: c_int,
    pub win_gravity: c_int,
    pub backing_store: c_int,
    pub backing_planes: c_ulong,
    pub backing_pixel: c_ulong,
    pub save_under: Bool,
    pub event_mask: c_long,
    pub do_not_propagate_mask: c_long,
    pub override_redirect: Bool,
    pub colormap: Colormap,
    pub cursor: Cursor,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XColor {
    pub pixel: c_ulong,
    pub red: c_ushort,
    pub green: c_ushort,
    pub blue: c_ushort,
    pub flags: c_char,
    pub pad: c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XAnyEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XExposeEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub x: c_int,
    pub y: c_int,
    pub width: c_int,
    pub height: c_int,
    pub count: c_int,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XConfigureEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub event: Window,
    pub window: Window,
    pub x: c_int,
    pub y: c_int,
    pub width: c_int,
    pub height: c_int,
    pub border_width: c_int,
    pub above: Window,
    pub override_redirect: Bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XMotionEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub root: Window,
    pub subwindow: Window,
    pub time: Time,
    pub x: c_int,
    pub y: c_int,
    pub x_root: c_int,
    pub y_root: c_int,
    pub state: c_uint,
    pub is_hint: c_char,
    pub same_screen: Bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XKeyEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub root: Window,
    pub subwindow: Window,
    pub time: Time,
    pub x: c_int,
    pub y: c_int,
    pub x_root: c_int,
    pub y_root: c_int,
    pub state: c_uint,
    pub keycode: c_uint,
    pub same_screen: Bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XButtonEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub root: Window,
    pub subwindow: Window,
    pub time: Time,
    pub x: c_int,
    pub y: c_int,
    pub x_root: c_int,
    pub y_root: c_int,
    pub state: c_uint,
    pub button: c_uint,
    pub same_screen: Bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union ClientMessageData {
    pub b: [c_char; 20],
    pub s: [c_short; 10],
    pub l: [c_long; 5],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XClientMessageEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub message_type: Atom,
    pub format: c_int,
    pub data: ClientMessageData,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XMapEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub event: Window,
    pub window: Window,
    pub override_redirect: Bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XRectangle {
    pub x: c_short,
    pub y: c_short,
    pub width: c_ushort,
    pub height: c_ushort,
}

#[repr(C)]
pub union XEvent {
    pub type_: c_int,
    pub xany: XAnyEvent,
    pub xexpose: XExposeEvent,
    pub xconfigure: XConfigureEvent,
    pub xmotion: XMotionEvent,
    pub xkey: XKeyEvent,
    pub xbutton: XButtonEvent,
    pub xclient: XClientMessageEvent,
    pub xmap: XMapEvent,
    pub pad: [c_long; 24],
}

#[repr(C)]
pub struct XErrorEvent {
    pub type_: c_int,
    pub display: *mut Display,
    pub resourceid: c_ulong,
    pub serial: c_ulong,
    pub error_code: u8,
    pub request_code: u8,
    pub minor_code: u8,
}

pub type XErrorHandler = Option<unsafe extern "C" fn(*mut Display, *mut XErrorEvent) -> c_int>;
pub type XIOErrorHandler = Option<unsafe extern "C" fn(*mut Display) -> c_int>;

// Process-lifetime X IO-error latch. Xlib invokes the IO-error handler on
// a dead connection (fd may stay open, so fd probes see "alive" and the
// normal path then issues X calls on dead state -> SIGSEGV in
// XCloseDisplay). The handler only sets the flag and returns; it must
// never exit. Checked in Drop paths to skip all X teardown.
//
// X_ERROR_SEEN is the second latch: mid-workload X failures observed as
// ordinary Err returns (fd still present, connection dead) also poison
// teardown. Any X failure return observed by SurfaceController must set it
// via mark_x_error_seen(); Drop skips X teardown when either latch is set.
static IO_BROKEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static X_ERROR_SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static IO_HANDLER_ONCE: std::sync::Once = std::sync::Once::new();

unsafe extern "C" fn io_error_latch(_display: *mut Display) -> c_int {
    IO_BROKEN.store(true, std::sync::atomic::Ordering::SeqCst);
    0
}

/// Install a non-fatal X error handler once per process so a stale
/// Picture/drawable id (e.g. a freed RENDER Picture raced by a client
/// exit during `manage`) is reported and skipped instead of killing the
/// window manager. The default Xlib handler exits the process on any
/// protocol error; the WM must survive those.
unsafe extern "C" fn protocol_error_ignore(
    _display: *mut Display,
    _event: *mut XErrorEvent,
) -> c_int {
    0
}

static PROTOCOL_HANDLER_ONCE: std::sync::Once = std::sync::Once::new();

/// Install the non-fatal protocol-error handler exactly once. Safe to call
/// after each successful XOpenDisplay; the handler is process-wide.
pub fn install_x_protocol_error_handler() {
    PROTOCOL_HANDLER_ONCE.call_once(|| unsafe {
        XSetErrorHandler(Some(protocol_error_ignore));
    });
}

/// Install the process-lifetime IO-error handler exactly once. Call after
/// each successful XOpenDisplay; the latch is process-wide by design.
pub fn install_x_io_error_handler() {
    IO_HANDLER_ONCE.call_once(|| unsafe {
        XSetIOErrorHandler(Some(io_error_latch));
    });
}

/// True once Xlib has reported a dead connection via the IO-error handler.
pub fn x_io_broken() -> bool {
    IO_BROKEN.load(std::sync::atomic::Ordering::SeqCst)
}

/// Record any observed X failure return. Mid-workload failures (sync-trap
/// fire, commit/pump error returns) mean the connection is dead even while
/// the fd still probes alive; the next X call (including XCloseDisplay)
/// may SIGSEGV. Keep behavior identical: set-only latch, no branching.
pub fn mark_x_error_seen() {
    X_ERROR_SEEN.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// True once any X failure return has been observed via mark_x_error_seen.
pub fn x_error_seen() -> bool {
    X_ERROR_SEEN.load(std::sync::atomic::Ordering::SeqCst)
}

pub const KEY_PRESS_MASK: c_long = 1 << 0;
pub const KEY_RELEASE_MASK: c_long = 1 << 1;
pub const BUTTON_PRESS_MASK: c_long = 1 << 2;
pub const BUTTON_RELEASE_MASK: c_long = 1 << 3;
pub const POINTER_MOTION_MASK: c_long = 1 << 6;
pub const LEAVE_WINDOW_MASK: c_long = 1 << 5;
pub const EXPOSURE_MASK: c_long = 1 << 15;
pub const STRUCTURE_NOTIFY_MASK: c_long = 1 << 17;

pub const KEY_PRESS: c_int = 2;
pub const KEY_RELEASE: c_int = 3;
pub const BUTTON_PRESS: c_int = 4;
pub const BUTTON_RELEASE: c_int = 5;
pub const MOTION_NOTIFY: c_int = 6;
pub const LEAVE_NOTIFY: c_int = 8;
pub const EXPOSE: c_int = 12;
pub const MAP_NOTIFY: c_int = 19;
pub const CONFIGURE_NOTIFY: c_int = 22;
pub const CLIENT_MESSAGE: c_int = 33;

pub const DO_RED: c_char = 1;
pub const DO_GREEN: c_char = 2;
pub const DO_BLUE: c_char = 4;

pub const ZPIXMAP: c_int = 2;
pub const QUEUED_AFTER_READING: c_int = 1;
pub const PROP_MODE_REPLACE: c_int = 0;
pub const XA_ATOM: Atom = 4;
pub const XA_CARDINAL: Atom = 6;
pub const CW_OVERRIDE_REDIRECT: c_ulong = 1 << 9;
pub const GRAB_MODE_ASYNC: c_int = 1;
pub const CURRENT_TIME: Time = 0;

pub const SHIFT_MASK: c_uint = 1 << 0;
pub const CONTROL_MASK: c_uint = 1 << 2;
pub const MOD1_MASK: c_uint = 1 << 3;
pub const MOD4_MASK: c_uint = 1 << 6;

pub const XK_BACK_SPACE: c_ulong = 0xff08;
pub const XK_DELETE: c_ulong = 0xffff;
pub const XK_TAB: c_ulong = 0xff09;
pub const XK_SPACE: c_ulong = 0x0020;
pub const XK_RETURN: c_ulong = 0xff0d;
pub const XK_ESCAPE: c_ulong = 0xff1b;
pub const XK_LEFT: c_ulong = 0xff51;
pub const XK_UP: c_ulong = 0xff52;
pub const XK_RIGHT: c_ulong = 0xff53;
pub const XK_DOWN: c_ulong = 0xff54;
pub const XK_SUPER_L: c_ulong = 0xffeb;
pub const XK_SUPER_R: c_ulong = 0xffec;

// Xcursor client-library theme/size applicators. Resolved dynamically by
// native/cursor.rs via `DynamicLibrary`; declared here so the facade has one
// named owner instead of ad-hoc byte-string symbols at call sites.
pub const XCURSOR_LIBRARY_NAMES: &[&str] = &["libXcursor.so.1", "libXcursor.so"];
pub const XCURSOR_SET_THEME_SYMBOL: &[u8] = b"XcursorSetTheme\0";
pub const XCURSOR_SET_SIZE_SYMBOL: &[u8] = b"XcursorSetSize\0";

// Standard X cursor font glyph indices from <X11/cursorfont.h>.
pub const XC_LEFT_PTR: c_uint = 68;
pub const XC_HAND2: c_uint = 60;
pub const XC_XTERM: c_uint = 152;
pub const XC_FLEUR: c_uint = 52;
pub const XC_SB_H_DOUBLE_ARROW: c_uint = 108;
pub const XC_SB_V_DOUBLE_ARROW: c_uint = 116;
pub const XC_BOTTOM_RIGHT_CORNER: c_uint = 14;
pub const XC_BOTTOM_LEFT_CORNER: c_uint = 12;

#[link(name = "X11")]
unsafe extern "C" {
    pub fn XOpenDisplay(display_name: *const c_char) -> *mut Display;
    pub fn XCloseDisplay(display: *mut Display) -> c_int;
    pub fn XDefaultScreen(display: *mut Display) -> c_int;
    pub fn XDefaultDepth(display: *mut Display, screen_number: c_int) -> c_int;
    pub fn XDefaultVisual(display: *mut Display, screen_number: c_int) -> *mut Visual;
    pub fn XRootWindow(display: *mut Display, screen_number: c_int) -> Window;
    pub fn XBlackPixel(display: *mut Display, screen_number: c_int) -> c_ulong;
    pub fn XWhitePixel(display: *mut Display, screen_number: c_int) -> c_ulong;
    pub fn XDefaultColormap(display: *mut Display, screen_number: c_int) -> Colormap;
    pub fn XMatchVisualInfo(
        display: *mut Display,
        screen: c_int,
        depth: c_int,
        class: c_int,
        vinfo_return: *mut XVisualInfo,
    ) -> Status;
    pub fn XFree(data: *mut c_void) -> c_int;
    pub fn XSetErrorHandler(handler: XErrorHandler) -> XErrorHandler;
    pub fn XSetIOErrorHandler(handler: XIOErrorHandler) -> XIOErrorHandler;
    pub fn XSync(display: *mut Display, discard: Bool) -> c_int;
    pub fn XCreateWindow(
        display: *mut Display,
        parent: Window,
        x: c_int,
        y: c_int,
        width: c_uint,
        height: c_uint,
        border_width: c_uint,
        depth: c_int,
        class: c_uint,
        visual: *mut Visual,
        valuemask: c_ulong,
        attributes: *mut XSetWindowAttributes,
    ) -> Window;
    pub fn XCreateColormap(
        display: *mut Display,
        window: Window,
        visual: *mut Visual,
        alloc: c_int,
    ) -> Colormap;
    pub fn XFreeColormap(display: *mut Display, colormap: Colormap) -> c_int;
    pub fn XCreateSimpleWindow(
        display: *mut Display,
        parent: Window,
        x: c_int,
        y: c_int,
        width: c_uint,
        height: c_uint,
        border_width: c_uint,
        border: c_ulong,
        background: c_ulong,
    ) -> Window;
    pub fn XChangeWindowAttributes(
        display: *mut Display,
        window: Window,
        valuemask: c_ulong,
        attributes: *mut XSetWindowAttributes,
    ) -> c_int;
    pub fn XDestroyWindow(display: *mut Display, window: Window) -> c_int;
    pub fn XCreatePixmap(
        display: *mut Display,
        drawable: Drawable,
        width: c_uint,
        height: c_uint,
        depth: c_uint,
    ) -> Pixmap;
    pub fn XFreePixmap(display: *mut Display, pixmap: Pixmap) -> c_int;
    pub fn XSetWindowBackgroundPixmap(
        display: *mut Display,
        window: Window,
        background_pixmap: Pixmap,
    ) -> c_int;
    pub fn XCopyArea(
        display: *mut Display,
        src: Drawable,
        dest: Drawable,
        gc: GC,
        src_x: c_int,
        src_y: c_int,
        width: c_uint,
        height: c_uint,
        dest_x: c_int,
        dest_y: c_int,
    ) -> c_int;
    pub fn XSelectInput(display: *mut Display, window: Window, event_mask: c_long) -> c_int;
    pub fn XMapWindow(display: *mut Display, window: Window) -> c_int;
    pub fn XUnmapWindow(display: *mut Display, window: Window) -> c_int;
    pub fn XRaiseWindow(display: *mut Display, window: Window) -> c_int;
    pub fn XMoveResizeWindow(
        display: *mut Display,
        window: Window,
        x: c_int,
        y: c_int,
        width: c_uint,
        height: c_uint,
    ) -> c_int;
    pub fn XGrabPointer(
        display: *mut Display,
        grab_window: Window,
        owner_events: Bool,
        event_mask: c_uint,
        pointer_mode: c_int,
        keyboard_mode: c_int,
        confine_to: Window,
        cursor: Cursor,
        time: Time,
    ) -> c_int;
    pub fn XUngrabPointer(display: *mut Display, time: Time) -> c_int;
    pub fn XDisplayWidth(display: *mut Display, screen_number: c_int) -> c_int;
    pub fn XDisplayHeight(display: *mut Display, screen_number: c_int) -> c_int;
    pub fn XStoreName(display: *mut Display, window: Window, window_name: *const c_char) -> c_int;
    pub fn XCreateGC(
        display: *mut Display,
        drawable: Drawable,
        valuemask: c_ulong,
        values: *mut std::ffi::c_void,
    ) -> GC;
    pub fn XFreeGC(display: *mut Display, gc: GC) -> c_int;
    pub fn XSetForeground(display: *mut Display, gc: GC, foreground: c_ulong) -> c_int;
    pub fn XSetGraphicsExposures(display: *mut Display, gc: GC, graphics_exposures: Bool) -> c_int;
    pub fn XSetClipMask(display: *mut Display, gc: GC, pixmap: Pixmap) -> c_int;
    pub fn XSetClipOrigin(
        display: *mut Display,
        gc: GC,
        clip_x_origin: c_int,
        clip_y_origin: c_int,
    ) -> c_int;
    pub fn XFillRectangle(
        display: *mut Display,
        drawable: Drawable,
        gc: GC,
        x: c_int,
        y: c_int,
        width: c_uint,
        height: c_uint,
    ) -> c_int;
    pub fn XDrawRectangle(
        display: *mut Display,
        drawable: Drawable,
        gc: GC,
        x: c_int,
        y: c_int,
        width: c_uint,
        height: c_uint,
    ) -> c_int;
    pub fn XDrawLine(
        display: *mut Display,
        drawable: Drawable,
        gc: GC,
        x1: c_int,
        y1: c_int,
        x2: c_int,
        y2: c_int,
    ) -> c_int;
    pub fn XDrawArc(
        display: *mut Display,
        drawable: Drawable,
        gc: GC,
        x: c_int,
        y: c_int,
        width: c_uint,
        height: c_uint,
        angle1: c_int,
        angle2: c_int,
    ) -> c_int;
    pub fn XFillArc(
        display: *mut Display,
        drawable: Drawable,
        gc: GC,
        x: c_int,
        y: c_int,
        width: c_uint,
        height: c_uint,
        angle1: c_int,
        angle2: c_int,
    ) -> c_int;
    pub fn XDrawString(
        display: *mut Display,
        drawable: Drawable,
        gc: GC,
        x: c_int,
        y: c_int,
        string: *const c_char,
        length: c_int,
    ) -> c_int;
    pub fn XCreateImage(
        display: *mut Display,
        visual: *mut Visual,
        depth: c_uint,
        format: c_int,
        offset: c_int,
        data: *mut c_char,
        width: c_uint,
        height: c_uint,
        bitmap_pad: c_int,
        bytes_per_line: c_int,
    ) -> *mut XImage;
    pub fn XDestroyImage(image: *mut XImage) -> c_int;
    pub fn XPutImage(
        display: *mut Display,
        drawable: Drawable,
        gc: GC,
        image: *mut XImage,
        src_x: c_int,
        src_y: c_int,
        dest_x: c_int,
        dest_y: c_int,
        width: c_uint,
        height: c_uint,
    ) -> c_int;
    pub fn XPutPixel(image: *mut XImage, x: c_int, y: c_int, pixel: c_ulong) -> c_int;
    pub fn XLoadFont(display: *mut Display, name: *const c_char) -> Font;
    pub fn XLoadQueryFont(display: *mut Display, name: *const c_char) -> *mut XFontStructHead;
    pub fn XFreeFont(display: *mut Display, font_struct: *mut XFontStructHead) -> c_int;
    pub fn XUnloadFont(display: *mut Display, font: Font) -> c_int;
    pub fn XSetFont(display: *mut Display, gc: GC, font: Font) -> c_int;
    pub fn XAllocColor(
        display: *mut Display,
        colormap: Colormap,
        screen_in_out: *mut XColor,
    ) -> Status;
    pub fn XLookupKeysym(key_event: *mut XKeyEvent, index: c_int) -> c_ulong;
    pub fn XNextEvent(display: *mut Display, event_return: *mut XEvent) -> c_int;
    pub fn XConnectionNumber(display: *mut Display) -> c_int;
    pub fn XPutBackEvent(display: *mut Display, event: *mut XEvent) -> c_int;
    pub fn XEventsQueued(display: *mut Display, mode: c_int) -> c_int;
    pub fn XPeekEvent(display: *mut Display, event_return: *mut XEvent) -> c_int;
    pub fn XFlush(display: *mut Display) -> c_int;
    pub fn XInternAtom(
        display: *mut Display,
        atom_name: *const c_char,
        only_if_exists: Bool,
    ) -> Atom;
    pub fn XChangeProperty(
        display: *mut Display,
        window: Window,
        property: Atom,
        type_: Atom,
        format: c_int,
        mode: c_int,
        data: *const u8,
        nelements: c_int,
    ) -> c_int;
    pub fn XDeleteProperty(display: *mut Display, window: Window, property: Atom) -> c_int;
    pub fn XSetWMProtocols(
        display: *mut Display,
        window: Window,
        protocols: *mut Atom,
        count: c_int,
    ) -> Status;
    pub fn XCreateFontCursor(display: *mut Display, shape: c_uint) -> Cursor;
    pub fn XDefineCursor(display: *mut Display, window: Window, cursor: Cursor) -> c_int;
    pub fn XFreeCursor(display: *mut Display, cursor: Cursor) -> c_int;
    pub fn XResizeWindow(
        display: *mut Display,
        window: Window,
        width: c_uint,
        height: c_uint,
    ) -> c_int;
    pub fn XSetClipRectangles(
        display: *mut Display,
        gc: GC,
        clip_x_origin: c_int,
        clip_y_origin: c_int,
        rectangles: *const XRectangle,
        n: c_int,
        ordering: c_int,
    ) -> c_int;
    pub fn XTextWidth(
        font_struct: *mut XFontStructHead,
        string: *const c_char,
        count: c_int,
    ) -> c_int;
    pub fn XGetImage(
        display: *mut Display,
        drawable: Drawable,
        x: c_int,
        y: c_int,
        width: c_uint,
        height: c_uint,
        plane_mask: c_ulong,
        format: c_int,
    ) -> *mut XImage;
}

unsafe extern "C" {
    pub fn malloc(size: usize) -> *mut c_void;
}
