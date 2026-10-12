//! Xlib declarations used by window capture that are absent from `xlib.rs`,
//! whose bytes are baseline-pinned.

use std::os::raw::{c_int, c_uint, c_void};

use crate::xlib::{Display, Drawable, Window};

unsafe extern "C" {
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
