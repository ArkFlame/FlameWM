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
type CreateSolidFillFn = unsafe extern "C" fn(*mut Display, *const XRenderColor) -> Picture;

#[derive(Clone, Copy)]
struct BaseApi {
    find_standard_format: FindStandardFormatFn,
    create_picture: CreatePictureFn,
    composite: CompositeFn,
    free_picture: FreePictureFn,
    create_solid_fill: CreateSolidFillFn,
}

/// `XRenderColor`: 16-bit non-premultiplied channels.
#[repr(C)]
#[derive(Clone, Copy)]
struct XRenderColor {
    red: u16,
    green: u16,
    blue: u16,
    alpha: u16,
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

// Xrender.h PictStandard* enum: ARGB32 = 0, RGB24 = 1, A8 = 2. RGB24 = 2 selected A8 (alpha only).
pub(crate) const PICT_STANDARD_RGB_24: c_int = 1;
const PICT_STANDARD_ARGB_32: c_int = 0;
const PICT_OP_SRC: c_int = 1;
const PICT_OP_OVER: c_int = 3;

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
            create_solid_fill: unsafe { library.symbol(b"XRenderCreateSolidFill\0") }?,
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
        // SAFETY: forwarded contract.
        unsafe { self.create_standard_picture(display, drawable, PICT_STANDARD_ARGB_32) }
    }

    /// Creates a picture over `drawable` in the standard XRender format `standard`
    /// (`PICT_STANDARD_*`); the caller frees it.
    ///
    /// # Safety
    ///
    /// `display` must be live and `drawable` a live pixmap or window on it.
    pub(crate) unsafe fn create_standard_picture(
        &self,
        display: *mut Display,
        drawable: Drawable,
        standard: c_int,
    ) -> Result<Picture, String> {
        // SAFETY: `display` is live per contract; NULL means no such standard format.
        let format = unsafe { (self.base.find_standard_format)(display, standard) };
        if format.is_null() {
            return Err(format!("XRenderFindStandardFormat({standard}) returned NULL"));
        }
        // SAFETY: live `display`/`drawable`/`format`; NULL attributes select defaults.
        let picture =
            unsafe { (self.base.create_picture)(display, drawable, format, 0, std::ptr::null()) };
        if picture == 0 {
            return Err(format!("XRenderCreatePicture returned 0 for format {standard}"));
        }
        Ok(picture)
    }

    /// Solid-colour picture that repeats everywhere; with `alpha` it serves as a constant mask.
    /// Channels are 16-bit. The caller frees it.
    ///
    /// # Safety
    ///
    /// `display` must be live.
    pub(crate) unsafe fn create_solid_fill(
        &self,
        display: *mut Display,
        red: u16,
        green: u16,
        blue: u16,
        alpha: u16,
    ) -> Result<Picture, String> {
        let color = XRenderColor { red, green, blue, alpha };
        // SAFETY: live `display` per contract; `color` outlives the call.
        let picture = unsafe { (self.base.create_solid_fill)(display, &color) };
        if picture == 0 {
            return Err("XRenderCreateSolidFill returned 0".to_string());
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
        // SAFETY: forwarded contract; no mask.
        unsafe { self.composite_op(display, PICT_OP_SRC, src, 0, dst, rect) };
    }

    /// Source-replace copy of `src` (through its transform) into `dst` over `rect`. The server
    /// feeds the transform `dst_pixel - rect_origin`, so the matrix is relative to `rect`.
    ///
    /// # Safety
    ///
    /// `display`, `src` and `dst` must be live on the same display.
    pub(crate) unsafe fn composite_src_relative(
        &self,
        display: *mut Display,
        src: Picture,
        dst: Picture,
        rect: (i32, i32, u32, u32),
    ) {
        let (x, y, width, height) = rect;
        // SAFETY: forwarded contract; no mask; the source origin is zero so the transform input is
        // relative to the rect origin.
        unsafe {
            (self.base.composite)(display, PICT_OP_SRC, src, 0, dst, 0, 0, 0, 0, x, y, width, height)
        };
    }

    /// Source-over copy of `src` into `dst` over `rect`, multiplied by `mask` (0 for none).
    /// The source offset equals the rect origin so the transform sees absolute destination pixels.
    ///
    /// # Safety
    ///
    /// `display`, `src`, `mask` (when non-zero) and `dst` must be live on the same display.
    pub(crate) unsafe fn composite_over_masked(
        &self,
        display: *mut Display,
        src: Picture,
        mask: Picture,
        dst: Picture,
        rect: (i32, i32, u32, u32),
    ) {
        // SAFETY: forwarded contract.
        unsafe { self.composite_op(display, PICT_OP_OVER, src, mask, dst, rect) };
    }

    unsafe fn composite_op(
        &self,
        display: *mut Display,
        op: c_int,
        src: Picture,
        mask: Picture,
        dst: Picture,
        rect: (i32, i32, u32, u32),
    ) {
        let (x, y, width, height) = rect;
        // The server feeds the transform with `dst_pixel - dst_origin + src_origin`. Every matrix in
        // this module is expressed in absolute destination coordinates, so the source origin must
        // equal the rect origin; a zero source origin sampled rect-relative points and went
        // transparent (black under SRC) for strips away from the window origin.
        // SAFETY: caller contract on `display`/`src`/`mask`/`dst`.
        unsafe {
            (self.base.composite)(display, op, src, mask, dst, x, y, x, y, x, y, width, height)
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
