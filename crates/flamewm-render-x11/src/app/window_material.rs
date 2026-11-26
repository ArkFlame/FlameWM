//! X11 properties connecting app window materials to the owning compositor.

use std::ffi::CString;
use std::os::raw::{c_int, c_long, c_uchar, c_ulong};
use std::ptr;

use crate::xlib::{Atom, Display, Window};

const CARDINAL: c_ulong = 6;
const PROP_MODE_REPLACE: c_int = 0;

unsafe extern "C" {
    fn XInternAtom(display: *mut Display, name: *const i8, only_if_exists: c_int) -> Atom;
    fn XGetWindowProperty(
        display: *mut Display,
        window: Window,
        property: Atom,
        long_offset: c_long,
        long_length: c_long,
        delete: c_int,
        req_type: Atom,
        actual_type: *mut Atom,
        actual_format: *mut c_int,
        nitems: *mut c_ulong,
        bytes_after: *mut c_ulong,
        prop: *mut *mut c_uchar,
    ) -> c_int;
    fn XChangeProperty(
        display: *mut Display,
        window: Window,
        property: Atom,
        type_: Atom,
        format: c_int,
        mode: c_int,
        data: *const c_uchar,
        nelements: c_int,
    ) -> c_int;
    fn XDeleteProperty(display: *mut Display, window: Window, property: Atom) -> c_int;
    fn XFree(data: *mut std::ffi::c_void) -> c_int;
}

/// The root contract is exactly one CARDINAL value equal to 1.
#[must_use]
pub(crate) fn compositor_backdrop_flag(words: &[u32]) -> bool {
    words == [1]
}

/// Read the compositor capability once during app construction.
///
/// # Safety
/// `display` must be live and `root` must belong to it.
pub(crate) unsafe fn read_compositor_backdrop(display: *mut Display, root: Window) -> bool {
    let name = CString::new("_FLAMEWM_COMPOSITOR_BACKDROP").expect("static atom");
    // SAFETY: the X display is live for this constructor call.
    let property = unsafe { XInternAtom(display, name.as_ptr(), 1) };
    if property == 0 {
        return false;
    }
    let mut actual_type = 0;
    let mut actual_format = 0;
    let mut nitems = 0;
    let mut bytes_after = 0;
    let mut raw = ptr::null_mut();
    // SAFETY: output pointers are valid; the requested property is bounded to one item.
    let status = unsafe {
        XGetWindowProperty(
            display,
            root,
            property,
            0,
            1,
            0,
            CARDINAL,
            &mut actual_type,
            &mut actual_format,
            &mut nitems,
            &mut bytes_after,
            &mut raw,
        )
    };
    let words = if status == 0
        && actual_type == CARDINAL
        && actual_format == 32
        && nitems == 1
        && bytes_after == 0
        && !raw.is_null()
    {
        // Xlib stores format-32 property items in native longs.
        Some([unsafe { *(raw.cast::<c_ulong>()) as u32 }])
    } else {
        None
    };
    if !raw.is_null() {
        // SAFETY: XGetWindowProperty allocated this buffer.
        unsafe { XFree(raw.cast()) };
    }
    words.is_some_and(|words| compositor_backdrop_flag(&words))
}

/// Encode a non-opaque material as the CARDINAL[2] property words.
#[must_use]
pub(crate) fn material_words(opacity_percent: u8, blur_percent: u8) -> Option<[u32; 2]> {
    (opacity_percent < 100 || blur_percent > 0)
        .then_some([u32::from(opacity_percent), u32::from(blur_percent)])
}

/// Publish only a changed value. The app window starts without this property.
///
/// # Safety
/// Must be called on the live app display thread.
pub(crate) unsafe fn publish_window_material(
    display: *mut Display,
    window: Window,
    previous: &mut Option<[u32; 2]>,
    next: Option<[u32; 2]>,
) {
    if *previous == next {
        return;
    }
    let name = CString::new("_FLAMEWM_WINDOW_MATERIAL").expect("static atom");
    // SAFETY: the X display is live for this app.
    let property = unsafe { XInternAtom(display, name.as_ptr(), 0) };
    if property == 0 {
        return;
    }
    match next {
        Some(words) => {
            // Xlib's format-32 interface consumes native long slots.
            let values = [c_ulong::from(words[0]), c_ulong::from(words[1])];
            // SAFETY: values has two format-32 items and remains live for the call.
            unsafe {
                XChangeProperty(
                    display,
                    window,
                    property,
                    CARDINAL,
                    32,
                    PROP_MODE_REPLACE,
                    values.as_ptr().cast(),
                    2,
                );
            }
        }
        None => {
            // SAFETY: property belongs to this app window.
            unsafe { XDeleteProperty(display, window, property) };
        }
    }
    *previous = next;
}

#[cfg(test)]
mod tests {
    use super::{compositor_backdrop_flag, material_words};

    #[test]
    fn parses_only_the_enabled_root_flag() {
        assert!(compositor_backdrop_flag(&[1]));
        assert!(!compositor_backdrop_flag(&[]));
        assert!(!compositor_backdrop_flag(&[0]));
        assert!(!compositor_backdrop_flag(&[1, 1]));
    }

    #[test]
    fn material_property_words_omit_opaque_material() {
        assert_eq!(material_words(72, 40), Some([72, 40]));
        assert_eq!(material_words(100, 0), None);
        assert_eq!(material_words(100, 1), Some([100, 1]));
    }
}
