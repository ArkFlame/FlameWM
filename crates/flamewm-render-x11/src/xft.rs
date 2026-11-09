#![allow(non_camel_case_types)]

use std::collections::HashMap;
use std::env;
use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_short, c_uchar, c_ulong, c_ushort, c_void};
use std::path::Path;
use std::ptr;

use flamewm_render_core::Color;

use crate::ffi::dynamic_library::DynamicLibrary;
use crate::text_mask_cache::{TextMaskCache, TextMaskKey, TextMaskRef};
use crate::xlib::{
    Colormap, Display, Drawable, Visual, XCloseDisplay, XCreateGC, XCreatePixmap, XDefaultColormap,
    XDefaultScreen, XDefaultVisual, XDestroyImage, XFillRectangle, XFreeGC, XGetImage, XImageHead,
    XOpenDisplay, XRectangle, XRootWindow, XSetForeground, ZPIXMAP,
};

#[repr(C)]
struct XftDraw {
    _private: [u8; 0],
}

#[repr(C)]
struct XftFont {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct XRenderColor {
    red: u16,
    green: u16,
    blue: u16,
    alpha: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct XftColor {
    pixel: c_ulong,
    color: XRenderColor,
}

#[repr(C)]
struct FcConfig {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct XGlyphInfo {
    width: c_ushort,
    height: c_ushort,
    x: c_short,
    y: c_short,
    x_off: c_short,
    y_off: c_short,
}

// XRender.h's XGlyphInfo is two unsigned shorts followed by four shorts.
const _: () = {
    assert!(std::mem::size_of::<XGlyphInfo>() == 12);
    assert!(std::mem::align_of::<XGlyphInfo>() == 2);
    let info = XGlyphInfo {
        width: 0,
        height: 0,
        x: 0,
        y: 0,
        x_off: 0,
        y_off: 0,
    };
    let _: c_ushort = info.width;
    let _: c_ushort = info.height;
    let _: c_short = info.x;
    let _: c_short = info.y;
    let _: c_short = info.x_off;
    let _: c_short = info.y_off;
};

type XftDrawCreateFn =
    unsafe extern "C" fn(*mut Display, Drawable, *mut Visual, Colormap) -> *mut XftDraw;
type XftDrawDestroyFn = unsafe extern "C" fn(*mut XftDraw);
type XftDrawCreateAlphaFn = unsafe extern "C" fn(*mut Display, Drawable, c_int) -> *mut XftDraw;
type XftDrawChangeFn = unsafe extern "C" fn(*mut XftDraw, Drawable);
/// `XftDrawSetClip(draw, region)`; a null region removes the clip.
type XftDrawSetClipFn = unsafe extern "C" fn(*mut XftDraw, *const c_void);
type XftDrawSetClipRectanglesFn =
    unsafe extern "C" fn(*mut XftDraw, c_int, c_int, *const XRectangle, c_int) -> c_int;
type XftDrawStringUtf8Fn = unsafe extern "C" fn(
    *mut XftDraw,
    *const XftColor,
    *mut XftFont,
    c_int,
    c_int,
    *const c_uchar,
    c_int,
);
type XftTextExtentsUtf8Fn =
    unsafe extern "C" fn(*mut Display, *mut XftFont, *const c_uchar, c_int, *mut XGlyphInfo);
type XftFontOpenNameFn = unsafe extern "C" fn(*mut Display, c_int, *const c_char) -> *mut XftFont;
type XftFontCloseFn = unsafe extern "C" fn(*mut Display, *mut XftFont);
type XftColorAllocValueFn = unsafe extern "C" fn(
    *mut Display,
    *mut Visual,
    Colormap,
    *const XRenderColor,
    *mut XftColor,
) -> c_int;
type XftColorFreeFn = unsafe extern "C" fn(*mut Display, *mut Visual, Colormap, *mut XftColor);

type FcConfigGetCurrentFn = unsafe extern "C" fn() -> *mut FcConfig;
type FcConfigAppFontAddFileFn = unsafe extern "C" fn(*mut FcConfig, *const c_uchar) -> c_int;
type FcConfigBuildFontsFn = unsafe extern "C" fn(*mut FcConfig) -> c_int;

#[derive(Clone, Copy)]
struct XftApi {
    draw_create: XftDrawCreateFn,
    draw_destroy: XftDrawDestroyFn,
    draw_create_alpha: Option<XftDrawCreateAlphaFn>,
    draw_change: XftDrawChangeFn,
    draw_string_utf8: XftDrawStringUtf8Fn,
    draw_set_clip: Option<XftDrawSetClipFn>,
    draw_set_clip_rectangles: Option<XftDrawSetClipRectanglesFn>,
    text_extents_utf8: Option<XftTextExtentsUtf8Fn>,
    font_open_name: XftFontOpenNameFn,
    font_close: XftFontCloseFn,
    color_alloc_value: XftColorAllocValueFn,
    color_free: XftColorFreeFn,
}

#[derive(Clone, Copy)]
struct FontconfigApi {
    config_get_current: FcConfigGetCurrentFn,
    config_app_font_add_file: FcConfigAppFontAddFileFn,
    config_build_fonts: FcConfigBuildFontsFn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct FontKey {
    pixels: u16,
    weight: u16,
}

/// Upper bound on rows per text-mask pixmap batch (bounds the batch to a few MB).
const MAX_MASK_BATCH_HEIGHT: u32 = 4096;

/// One text run to rasterize as an A8 coverage mask.
#[derive(Clone, Copy, Debug)]
pub struct AlphaMaskSpec<'a> {
    pub size: f32,
    pub weight: u16,
    pub text: &'a str,
    pub pad: u32,
}

/// A rendered A8 coverage mask, row-major, `width * height` bytes.
#[derive(Debug)]
pub(crate) struct AlphaMask {
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

pub struct XftBackend {
    display: *mut Display,
    screen: c_int,
    visual: *mut Visual,
    colormap: Colormap,
    draw: *mut XftDraw,
    api: XftApi,
    _xft_library: Option<DynamicLibrary>,
    _fontconfig_library: Option<DynamicLibrary>,
    fonts: HashMap<FontKey, *mut XftFont>,
    colors: HashMap<Color, XftColor>,
    preferred_family: String,
    text_masks: TextMaskCache,
    /// Synchronous `XGetImage` readbacks issued for text masks (one per batch).
    pub(crate) readbacks: u64,
}

mod backend;

mod backend_mask;

mod backend_measure;

mod fonts;
use fonts::*;

pub use fonts::install_default_layout_measure;

pub(crate) use fonts::{scoped_text_measure, xft_measure_estimate, xrender_color_for};

mod tests;

mod default_measure_tests;
