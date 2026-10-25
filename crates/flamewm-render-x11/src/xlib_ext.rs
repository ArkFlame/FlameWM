//! Xlib declarations used by window capture that are absent from `xlib.rs`,
//! whose bytes are baseline-pinned.

use std::os::raw::{c_int, c_long, c_uint, c_ulong, c_void};

use crate::xlib::{Atom, Bool, Display, Drawable, Time, Window, XEvent};

unsafe extern "C" {
    pub fn XSendEvent(
        display: *mut Display,
        window: Window,
        propagate: Bool,
        event_mask: c_long,
        event: *mut XEvent,
    ) -> c_int;
    pub fn XConvertSelection(
        display: *mut Display,
        selection: Atom,
        target: Atom,
        property: Atom,
        requestor: Window,
        time: Time,
    ) -> c_int;
    pub fn XSetSelectionOwner(
        display: *mut Display,
        selection: Atom,
        owner: Window,
        time: Time,
    ) -> c_int;
    pub fn XGetSelectionOwner(display: *mut Display, selection: Atom) -> Window;
    pub fn XGetWindowProperty(
        display: *mut Display,
        window: Window,
        property: Atom,
        long_offset: c_long,
        long_length: c_long,
        delete: Bool,
        req_type: Atom,
        actual_type: *mut Atom,
        actual_format: *mut c_int,
        nitems: *mut c_ulong,
        bytes_after: *mut c_ulong,
        prop: *mut *mut u8,
    ) -> c_int;
    pub fn XGetGeometry(
        display: *mut Display,
        drawable: Drawable,
        root: *mut Window,
        x: *mut c_int,
        y: *mut c_int,
        width: *mut c_uint,
        height: *mut c_uint,
        border_width: *mut c_uint,
        depth: *mut c_uint,
    ) -> c_int;
    pub fn XTranslateCoordinates(
        display: *mut Display,
        src_window: Window,
        dest_window: Window,
        src_x: c_int,
        src_y: c_int,
        dest_x_return: *mut c_int,
        dest_y_return: *mut c_int,
        child_return: *mut Window,
    ) -> c_int;
    pub fn XQueryTree(
        display: *mut Display,
        window: Window,
        root_return: *mut Window,
        parent_return: *mut Window,
        children_return: *mut *mut Window,
        nchildren_return: *mut c_uint,
    ) -> c_int;
    pub fn free(ptr: *mut c_void);
    pub fn XGetWindowAttributes(
        display: *mut Display,
        window: Window,
        attributes: *mut XWindowAttributes,
    ) -> c_int;
}

/// `VisibilityChangeMask`: a window learns when windows stacked above it start or stop covering it.
pub const VISIBILITY_CHANGE_MASK: c_long = 1 << 16;
/// `VisibilityNotify` event type.
pub const VISIBILITY_NOTIFY: c_int = 15;
/// `VisibilityUnobscured`: nothing covers the window.
pub const VISIBILITY_UNOBSCURED: c_int = 0;
/// `IsViewable` map state: the window and all its ancestors are mapped.
pub const IS_VIEWABLE: c_int = 2;
/// `InputOnly` window class: paints nothing.
pub const INPUT_ONLY: c_int = 2;

/// Xlib `XVisibilityEvent`, read from the `XEvent` union's common prefix.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct XVisibilityEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub state: c_int,
}

/// Xlib `XWindowAttributes`.
#[repr(C)]
pub struct XWindowAttributes {
    pub x: c_int,
    pub y: c_int,
    pub width: c_int,
    pub height: c_int,
    pub border_width: c_int,
    pub depth: c_int,
    pub visual: *mut c_void,
    pub root: Window,
    pub class: c_int,
    pub bit_gravity: c_int,
    pub win_gravity: c_int,
    pub backing_store: c_int,
    pub backing_planes: c_ulong,
    pub backing_pixel: c_ulong,
    pub save_under: Bool,
    pub colormap: c_ulong,
    pub map_installed: Bool,
    pub map_state: c_int,
    pub all_event_masks: c_long,
    pub your_event_mask: c_long,
    pub do_not_propagate_mask: c_long,
    pub override_redirect: Bool,
    pub screen: *mut c_void,
}
