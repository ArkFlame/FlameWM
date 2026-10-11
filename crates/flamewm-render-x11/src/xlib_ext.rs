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
    pub fn free(ptr: *mut c_void);
}
