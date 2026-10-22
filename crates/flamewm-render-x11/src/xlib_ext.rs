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
    pub fn free(ptr: *mut c_void);
}
