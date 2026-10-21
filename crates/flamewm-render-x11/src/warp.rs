//! Magic Lamp warp: XRender paint path for the bend.
//!
//! The geometry lives in [`crate::lamp`]: horizontal source bands for a bottom/top panel,
//! vertical bands for a left/right panel. Each band is an affine map of the captured window
//! rows (or columns). `paint_warped` composites every band with its own transform.

use std::os::raw::c_ulong;

use crate::lamp::LampBand;
use crate::xcomposite::{CapturedImage, check_capture_size, upload_pixmap_depth};
use crate::xlib::{Display, Pixmap, XFreePixmap};
use crate::xrender_transform::{PICT_STANDARD_RGB_24, XRenderTransformApi};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WarpRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Destination rect `(x, y, width, height)` in overlay-local pixels and the destination-to-source
/// matrix for one band. Returns `None` for a band with no visible pixels.
///
/// The matrix is relative to the rect origin: the server feeds it `dst_pixel - rect_origin`
/// (see `XRenderTransformApi::composite_src_relative`), so offsets stay small for narrow
/// icons on wide windows, where absolute offsets overflow XFixed 16.16.
pub fn band_transform(
    band: &LampBand,
    origin: (f64, f64),
) -> Option<((i32, i32, u32, u32), [[f64; 3]; 3])> {
    let (sx, sy, sw, sh) = band.src;
    let (dx, dy, dw, dh) = band.dst;
    if !(dw > 0.0 && dh > 0.0) {
        return None;
    }
    let x0 = dx - origin.0;
    let y0 = dy - origin.1;
    let ix0 = x0.round() as i32;
    let iy0 = y0.round() as i32;
    let ix1 = (x0 + dw).round() as i32;
    let iy1 = (y0 + dh).round() as i32;
    if ix1 <= ix0 || iy1 <= iy0 {
        return None;
    }
    let kx = sw / dw;
    let ky = sh / dh;
    // Source point for rect-relative pixel `u` is `kx * u + offset`; `u = (pixel - ix0)`.
    let off_x = sx + (f64::from(ix0) - x0) * kx;
    let off_y = sy + (f64::from(iy0) - y0) * ky;
    Some((
        (ix0, iy0, (ix1 - ix0) as u32, (iy1 - iy0) as u32),
        [[kx, 0.0, off_x], [0.0, ky, off_y], [0.0, 0.0, 1.0]],
    ))
}

/// Capture uploaded once as a pixmap and picture. A warp reuses it for every frame and the
/// owner frees it exactly once with [`free_warp_source`].
#[derive(Debug)]
pub struct WarpSource {
    pixmap: Pixmap,
    picture: c_ulong,
    width: u32,
    height: u32,
}

/// Uploads `capture` once for later [`paint_warped`] calls.
///
/// Fails without leaving resources behind when the transform extension is missing or the
/// capture is oversized.
///
/// # Safety
///
/// `display` must be a live Xlib display.
pub unsafe fn upload_warp_source(
    display: *mut Display,
    api: &XRenderTransformApi,
    capture: &CapturedImage,
) -> Result<WarpSource, String> {
    if display.is_null() {
        return Err("upload_warp_source: null display".to_string());
    }
    if !api.capabilities().picture_transform {
        return Err("upload_warp_source: XRenderSetPictureTransform unavailable".to_string());
    }
    check_capture_size(capture.width, capture.height)?;
    // SAFETY: `display` is live per contract; the pixmap is freed on every failure path below.
    let pixmap = unsafe { upload_pixmap_depth(display, capture, 24) }?;
    // SAFETY: `display` and `pixmap` are live; the picture is owned by the returned source.
    match unsafe { api.create_standard_picture(display, pixmap, PICT_STANDARD_RGB_24) } {
        Ok(picture) => Ok(WarpSource {
            pixmap,
            picture,
            width: capture.width,
            height: capture.height,
        }),
        Err(error) => {
            // SAFETY: `pixmap` was created above and is not used again.
            unsafe { XFreePixmap(display, pixmap) };
            Err(error)
        }
    }
}

/// Releases the picture and pixmap created by [`upload_warp_source`].
///
/// # Safety
///
/// `display` must be the live display `source` was uploaded on; `source` is consumed.
pub unsafe fn free_warp_source(
    display: *mut Display,
    api: &XRenderTransformApi,
    source: WarpSource,
) {
    // SAFETY: both ids were created by `upload_warp_source` on `display` and are released once.
    unsafe {
        api.free_picture(display, source.picture);
        XFreePixmap(display, source.pixmap);
    }
}

/// Paints the uploaded `source` through `bands` onto `dest`: one transformed composite per band,
/// with a bilinear filter when the library provides it. `origin` is the root position of
/// `dest`'s (0,0); band destinations are root coordinates.
///
/// Fails without touching `dest` when a band reads outside the capture.
///
/// # Safety
///
/// `display` must be a live Xlib display, `source` uploaded on it, and `dest` a live XRender
/// picture on it.
pub unsafe fn paint_warped(
    display: *mut Display,
    api: &XRenderTransformApi,
    source: &WarpSource,
    bands: &[LampBand],
    origin: (f64, f64),
    dest: c_ulong,
) -> Result<(), String> {
    let (w, h) = (f64::from(source.width), f64::from(source.height));
    if bands.iter().any(|b| {
        b.src.0 < 0.0
            || b.src.1 < 0.0
            || b.src.0 + b.src.2 > w + 1e-6
            || b.src.1 + b.src.3 > h + 1e-6
    }) {
        return Err("paint_warped: band source outside capture size".to_string());
    }
    if display.is_null() {
        return Err("paint_warped: null display".to_string());
    }
    // SAFETY: `source.picture` and `dest` are live pictures on `display` per contract.
    unsafe { paint_bands(display, api, source.picture, dest, bands, origin) }
}

unsafe fn paint_bands(
    display: *mut Display,
    api: &XRenderTransformApi,
    source: c_ulong,
    dest: c_ulong,
    bands: &[LampBand],
    origin: (f64, f64),
) -> Result<(), String> {
    if api.capabilities().picture_filter {
        // SAFETY: `source` is a live picture on `display`.
        unsafe { api.set_bilinear_filter(display, source) }?;
    }
    for band in bands {
        let Some((rect, matrix)) = band_transform(band, origin) else {
            continue;
        };
        // SAFETY: `source` and `dest` are live pictures on `display`.
        unsafe {
            api.set_transform(display, source, matrix)?;
            api.composite_src_relative(display, source, dest, rect);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn band(src: (f64, f64, f64, f64), dst: (f64, f64, f64, f64)) -> LampBand {
        LampBand { src, dst }
    }

    fn apply(m: [[f64; 3]; 3], x: f64, y: f64) -> (f64, f64) {
        (m[0][0] * x + m[0][2], m[1][1] * y + m[1][2])
    }

    #[test]
    fn identity_band_maps_each_pixel_to_its_source_row() {
        // Window at root (300,100), 800x600, one band covering rows 40..60 at its own place.
        let origin = (300.0, 100.0);
        let b = band((0.0, 40.0, 800.0, 20.0), (300.0, 140.0, 800.0, 20.0));
        let (rect, m) = band_transform(&b, origin).unwrap();
        assert_eq!(rect, (0, 40, 800, 20));
        // Rect-relative pixel (0,0) is the band's first source row.
        let (sx, sy) = apply(m, 0.0, 0.0);
        assert!(sx.abs() < 1e-9 && (sy - 40.0).abs() < 1e-9);
        let (sx, sy) = apply(m, 800.0, 20.0);
        assert!((sx - 800.0).abs() < 1e-9 && (sy - 60.0).abs() < 1e-9);
    }

    #[test]
    fn narrow_band_on_wide_window_keeps_matrix_offsets_small() {
        // 4K window, 48px icon on the far right: lateral scale ~80, and the origin is far from zero.
        let origin = (0.0, 0.0);
        let b = band((0.0, 10.0, 3840.0, 6.0), (3000.0, 2000.0, 48.0, 6.0));
        let (_, m) = band_transform(&b, origin).unwrap();
        assert!(m[0][0] > 50.0 && m[0][0] < 100.0);
        assert!(
            m[0][2].abs() < 32768.0,
            "offset {} overflows XFixed",
            m[0][2]
        );
        // The absolute-origin form would be ~3000 * 80 = 240000 and does not fit XFixed 16.16.
        assert!(3000.0 * m[0][0] > 32768.0);
    }

    #[test]
    fn zero_area_band_is_skipped() {
        assert!(
            band_transform(
                &band((0.0, 0.0, 10.0, 1.0), (5.0, 5.0, 0.0, 1.0)),
                (0.0, 0.0)
            )
            .is_none()
        );
        assert!(
            band_transform(
                &band((0.0, 0.0, 10.0, 1.0), (5.0, 5.0, 3.0, 0.0)),
                (0.0, 0.0)
            )
            .is_none()
        );
        // A sub-pixel band whose edges round to the same row is skipped too.
        assert!(
            band_transform(
                &band((0.0, 0.0, 10.0, 1.0), (5.0, 5.1, 3.0, 0.3)),
                (0.0, 0.0)
            )
            .is_none()
        );
    }

    #[test]
    fn adjacent_bands_share_rounded_edges() {
        let a = band((0.0, 0.0, 800.0, 6.0), (300.0, 150.4, 790.0, 5.6));
        let b = band((0.0, 6.0, 800.0, 6.0), (300.0, 156.0, 790.0, 5.6));
        let (ra, _) = band_transform(&a, (300.0, 100.0)).unwrap();
        let (rb, _) = band_transform(&b, (300.0, 100.0)).unwrap();
        assert_eq!(
            ra.1 + ra.3 as i32,
            rb.1,
            "bands must tile with no gap or overlap"
        );
    }

    #[test]
    fn paint_rejects_source_outside_capture_before_any_x_call() {
        let source = WarpSource {
            pixmap: 0,
            picture: 0,
            width: 200,
            height: 300,
        };
        let Ok(api) = XRenderTransformApi::load() else {
            return;
        };
        let bands = [band((0.0, 0.0, 400.0, 6.0), (0.0, 0.0, 400.0, 6.0))];
        // SAFETY: the size check fails before any Xlib call uses the null display.
        let result =
            unsafe { paint_warped(std::ptr::null_mut(), &api, &source, &bands, (0.0, 0.0), 0) };
        assert!(result.unwrap_err().contains("size"));
    }

    #[test]
    fn upload_and_paint_fail_closed_without_x() {
        let Ok(api) = XRenderTransformApi::load() else {
            return;
        };
        let capture = CapturedImage {
            width: 400,
            height: 300,
            argb: vec![0; 400 * 300 * 4],
        };
        // SAFETY: a null display is rejected before any Xlib call.
        let upload = unsafe { upload_warp_source(std::ptr::null_mut(), &api, &capture) };
        assert!(upload.is_err());
    }
}
