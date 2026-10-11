//! Optional XRender picture-transform operations for transition effects.
//!
//! Kept apart from `xrender.rs`, whose bytes are baseline-pinned.
#![allow(non_camel_case_types)]

use std::os::raw::{c_char, c_int, c_ulong, c_void};

use crate::ffi::dynamic_library::DynamicLibrary;
use crate::xlib::{Display, Drawable};

type Picture = c_ulong;

#[repr(C)]
struct XRenderPictFormat {
    _private: [u8; 0],
}

type FindStandardFormatFn = unsafe extern "C" fn(*mut Display, c_int) -> *mut XRenderPictFormat;
type CreatePictureFn = unsafe extern "C" fn(
    *mut Display,
    Drawable,
    *mut XRenderPictFormat,
    c_ulong,
    *const c_void,
) -> Picture;
type CompositeFn = unsafe extern "C" fn(
    *mut Display,
    c_int,
    Picture,
    Picture,
    Picture,
    c_int,
    c_int,
    c_int,
    c_int,
    c_int,
    c_int,
    u32,
    u32,
);
type FreePictureFn = unsafe extern "C" fn(*mut Display, Picture);

#[derive(Clone, Copy)]
struct BaseApi {
    find_standard_format: FindStandardFormatFn,
    create_picture: CreatePictureFn,
    composite: CompositeFn,
    free_picture: FreePictureFn,
}

type SetPictureTransformFn = unsafe extern "C" fn(*mut Display, Picture, *const XTransform);
type SetPictureFilterFn =
    unsafe extern "C" fn(*mut Display, Picture, *const c_char, *mut c_int, c_int);
type SetPictureClipRectanglesFn =
    unsafe extern "C" fn(*mut Display, Picture, c_int, c_int, *const c_void, c_int);

/// XFixed 16.16 matrix, row-major, mapping destination to source space.
#[repr(C)]
#[derive(Clone, Copy)]
struct XTransform {
    matrix: [[c_int; 3]; 3],
}

const PICT_STANDARD_ARGB_32: c_int = 1;
const PICT_OP_SRC: c_int = 1;

/// Which optional XRender extension entry points the loaded libXrender exports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XRenderCapabilities {
    pub picture_transform: bool,
    pub picture_filter: bool,
    pub picture_clip_rectangles: bool,
}

/// XRender picture operations used by transition effects. Optional symbols
/// resolve to `None` when the library lacks them; no call path panics.
pub struct XRenderTransformApi {
    _library: DynamicLibrary,
    base: BaseApi,
    set_picture_transform: Option<SetPictureTransformFn>,
    set_picture_filter: Option<SetPictureFilterFn>,
    set_picture_clip_rectangles: Option<SetPictureClipRectanglesFn>,
}

impl XRenderTransformApi {
    pub fn load() -> Result<Self, String> {
        let library = DynamicLibrary::open(&["libXrender.so.1", "libXrender.so"])?;
        // SAFETY: each name is a static NUL-terminated symbol and `T` is the exact
        // function-pointer type declared above, matching the C ABI.
        let base = BaseApi {
            find_standard_format: unsafe { library.symbol(b"XRenderFindStandardFormat\0") }?,
            create_picture: unsafe { library.symbol(b"XRenderCreatePicture\0") }?,
            composite: unsafe { library.symbol(b"XRenderComposite\0") }?,
            free_picture: unsafe { library.symbol(b"XRenderFreePicture\0") }?,
        };
        // SAFETY: names are static NUL-terminated; a failed lookup is an absent extension.
        let set_picture_transform =
            unsafe { library.symbol(b"XRenderSetPictureTransform\0") }.ok();
        let set_picture_filter = unsafe { library.symbol(b"XRenderSetPictureFilter\0") }.ok();
        let set_picture_clip_rectangles =
            unsafe { library.symbol(b"XRenderSetPictureClipRectangles\0") }.ok();
        Ok(Self {
            _library: library,
            base,
            set_picture_transform,
            set_picture_filter,
            set_picture_clip_rectangles,
        })
    }

    pub fn capabilities(&self) -> XRenderCapabilities {
        XRenderCapabilities {
            picture_transform: self.set_picture_transform.is_some(),
            picture_filter: self.set_picture_filter.is_some(),
            picture_clip_rectangles: self.set_picture_clip_rectangles.is_some(),
        }
    }

    /// Creates an ARGB32 picture over `drawable`; the caller frees it.
    ///
    /// # Safety
    ///
    /// `display` must be live and `drawable` a live pixmap or window on it.
    pub(crate) unsafe fn create_argb_picture(
        &self,
        display: *mut Display,
        drawable: Drawable,
    ) -> Result<Picture, String> {
        // SAFETY: `display` is live per contract; NULL means no such standard format.
        let format = unsafe { (self.base.find_standard_format)(display, PICT_STANDARD_ARGB_32) };
        if format.is_null() {
            return Err("XRenderFindStandardFormat(ARGB32) returned NULL".to_string());
        }
        // SAFETY: live `display`/`drawable`/`format`; NULL attributes select defaults.
        let picture =
            unsafe { (self.base.create_picture)(display, drawable, format, 0, std::ptr::null()) };
        if picture == 0 {
            return Err("XRenderCreatePicture returned 0 for ARGB32".to_string());
        }
        Ok(picture)
    }

    /// # Safety
    ///
    /// `display` must be live and `picture` a picture created on it, not yet freed.
    pub(crate) unsafe fn free_picture(&self, display: *mut Display, picture: Picture) {
        // SAFETY: caller contract above.
        unsafe { (self.base.free_picture)(display, picture) };
    }

    /// Sets the destination-to-source transform of `picture`.
    ///
    /// # Safety
    ///
    /// `display` must be live and `picture` a live picture on it.
    pub(crate) unsafe fn set_transform(
        &self,
        display: *mut Display,
        picture: Picture,
        matrix: [[f64; 3]; 3],
    ) -> Result<(), String> {
        let set = self
            .set_picture_transform
            .ok_or_else(|| "XRenderSetPictureTransform unavailable".to_string())?;
        let mut fixed = [[0 as c_int; 3]; 3];
        for (row, values) in matrix.iter().enumerate() {
            for (col, value) in values.iter().enumerate() {
                fixed[row][col] = to_fixed(*value)?;
            }
        }
        let transform = XTransform { matrix: fixed };
        // SAFETY: live `display`/`picture` per contract; `transform` outlives the call.
        unsafe { set(display, picture, &transform) };
        Ok(())
    }

    /// # Safety
    ///
    /// `display` must be live and `picture` a live picture on it.
    pub(crate) unsafe fn set_bilinear_filter(
        &self,
        display: *mut Display,
        picture: Picture,
    ) -> Result<(), String> {
        let set = self
            .set_picture_filter
            .ok_or_else(|| "XRenderSetPictureFilter unavailable".to_string())?;
        // SAFETY: NUL-terminated static name; zero params with NULL pointer is valid.
        unsafe {
            set(
                display,
                picture,
                b"bilinear\0".as_ptr() as *const c_char,
                std::ptr::null_mut(),
                0,
            )
        };
        Ok(())
    }

    /// Source-over-replace copy of `src` (through its transform) into `dst`.
    ///
    /// # Safety
    ///
    /// `display`, `src` and `dst` must be live on the same display.
    pub(crate) unsafe fn composite_src(
        &self,
        display: *mut Display,
        src: Picture,
        dst: Picture,
        rect: (i32, i32, u32, u32),
    ) {
        let (x, y, width, height) = rect;
        // SAFETY: caller contract; source offsets are zero because the transform maps
        // destination pixels into source space.
        unsafe {
            (self.base.composite)(
                display, PICT_OP_SRC, src, 0, dst, 0, 0, 0, 0, x, y, width, height,
            )
        };
    }
}

/// Converts to XFixed 16.16; rejects non-finite values and magnitudes that overflow.
fn to_fixed(value: f64) -> Result<c_int, String> {
    if !value.is_finite() || value.abs() >= 32768.0 {
        return Err(format!("XFixed value out of range: {value}"));
    }
    Ok((value * 65536.0).round() as c_int)
}
