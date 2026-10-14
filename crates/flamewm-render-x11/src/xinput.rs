//! XInput2 and generic-event FFI kept beside the pinned `xlib.rs` surface.
//!
//! `XEvent` in `xlib.rs` has no generic-event member, so the cookie view is
//! read from the union storage through `generic_cookie`.

use std::os::raw::{c_char, c_int, c_uint, c_ulong, c_void};

use crate::xlib::{Bool, Display, Status, Time, Window, XEvent};

pub const GENERIC_EVENT: c_int = 35;

/// XInput2 event type for a raw (unfiltered, ungrabbed) button press.
pub const XI_RAW_BUTTON_PRESS: c_int = 15;
pub const XI_ALL_MASTER_DEVICES: c_int = 1;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XGenericEventCookie {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub extension: c_int,
    pub evtype: c_int,
    pub cookie: c_uint,
    pub data: *mut c_void,
}

/// Prefix of XInput2 `XIRawEvent`; only the fields read by the popup dismissal path.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct XIRawEvent {
    pub type_: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub extension: c_int,
    pub evtype: c_int,
    pub time: Time,
    pub deviceid: c_int,
    pub sourceid: c_int,
    pub detail: c_int,
}

#[repr(C)]
pub struct XIEventMask {
    pub deviceid: c_int,
    pub mask_len: c_int,
    pub mask: *mut u8,
}

const _: () = assert!(std::mem::size_of::<XGenericEventCookie>() <= std::mem::size_of::<XEvent>());

/// Reads the generic-event view of `event`.
///
/// # Safety
///
/// `event` must hold a GenericEvent (`type_ == GENERIC_EVENT`) as filled by XNextEvent.
pub(crate) unsafe fn generic_cookie(event: &XEvent) -> XGenericEventCookie {
    // SAFETY: the size guard above keeps the read inside the union; the caller
    // guarantees the active member is a generic event.
    unsafe { std::ptr::read((event as *const XEvent).cast::<XGenericEventCookie>()) }
}

unsafe extern "C" {
    pub fn XGetEventData(display: *mut Display, cookie: *mut XGenericEventCookie) -> Bool;
    pub fn XFreeEventData(display: *mut Display, cookie: *mut XGenericEventCookie);
    pub fn XQueryExtension(
        display: *mut Display,
        name: *const c_char,
        major_opcode_return: *mut c_int,
        first_event_return: *mut c_int,
        first_error_return: *mut c_int,
    ) -> Bool;
    pub fn XQueryPointer(
        display: *mut Display,
        window: Window,
        root_return: *mut Window,
        child_return: *mut Window,
        root_x_return: *mut c_int,
        root_y_return: *mut c_int,
        win_x_return: *mut c_int,
        win_y_return: *mut c_int,
        mask_return: *mut c_uint,
    ) -> Bool;
}

#[link(name = "Xi")]
unsafe extern "C" {
    pub fn XIQueryVersion(display: *mut Display, major: *mut c_int, minor: *mut c_int) -> Status;
    pub fn XISelectEvents(
        display: *mut Display,
        window: Window,
        masks: *mut XIEventMask,
        num_masks: c_int,
    ) -> Status;
}
