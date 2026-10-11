//! Magic Lamp warp: pure strip geometry plus the XRender paint path.
//!
//! The mesh is `strip_count` vertical strips. Each strip is a quad whose top
//! and bottom edges narrow toward the target at different rates; the bottom
//! pinches first, which gives the genie curve. `paint_warped` paints each
//! strip as an affine map of its quad bounding box, so slanted edges are
//! approximated.

use std::os::raw::c_ulong;

use crate::xcomposite::{CapturedImage, check_capture_size, upload_argb_pixmap};
use crate::xlib::{Display, XFreePixmap};
use crate::xrender_transform::XRenderTransformApi;

pub const MAX_WARP_STRIPS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WarpRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// One strip: source x-range (absolute) and destination quad corners in
/// order top-left, top-right, bottom-right, bottom-left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WarpStrip {
    pub src_x0: f64,
    pub src_x1: f64,
    pub dst: [(f64, f64); 4],
}

#[derive(Debug, Clone, PartialEq)]
pub struct WarpMesh {
    pub source: WarpRect,
    pub strips: Vec<WarpStrip>,
}

fn lerp(a: f64, b: f64, s: f64) -> f64 {
    (1.0 - s) * a + s * b
}

fn validate_rect(rect: WarpRect, label: &str) -> Result<(), String> {
    let finite = [rect.x, rect.y, rect.width, rect.height]
        .iter()
        .all(|v| v.is_finite());
    if !finite || rect.width <= 0.0 || rect.height <= 0.0 {
        return Err(format!("warp: {label} rect must be finite and non-empty"));
    }
    Ok(())
}

/// Builds the Magic Lamp mesh at progress `t` (clamped to 0..=1).
///
/// `t = 0` reproduces `source` exactly; `t = 1` places every edge on `target`.
/// Strip boundaries are shared bit-for-bit, so strips tile without overlap.
pub fn magic_lamp_mesh(
    source: WarpRect,
    target: WarpRect,
    t: f64,
    strip_count: usize,
) -> Result<WarpMesh, String> {
    validate_rect(source, "source")?;
    validate_rect(target, "target")?;
    if !(1..=MAX_WARP_STRIPS).contains(&strip_count) {
        return Err(format!("warp: strip_count must be 1..={MAX_WARP_STRIPS}"));
    }
    if !t.is_finite() {
        return Err("warp: progress must be finite".to_string());
    }
    let t = t.clamp(0.0, 1.0);
    // The top edge narrows slowly; the bottom edge pinches first (smoothstep).
    let top_s = t * t;
    let bottom_s = t * t * (3.0 - 2.0 * t);
    let y_top = lerp(source.y, target.y, t);
    let y_bottom = lerp(source.y + source.height, target.y + target.height, t);

    let edge_x = |u: f64, s: f64| {
        lerp(
            source.x + u * source.width,
            target.x + u * target.width,
            s,
        )
    };
    let edge_u = |i: usize| i as f64 / strip_count as f64;
    let strips = (0..strip_count)
        .map(|i| {
            let (u0, u1) = (edge_u(i), edge_u(i + 1));
            WarpStrip {
                src_x0: source.x + u0 * source.width,
                src_x1: source.x + u1 * source.width,
                dst: [
                    (edge_x(u0, top_s), y_top),
                    (edge_x(u1, top_s), y_top),
                    (edge_x(u1, bottom_s), y_bottom),
                    (edge_x(u0, bottom_s), y_bottom),
                ],
            }
        })
        .collect();
    Ok(WarpMesh { source, strips })
}

/// Integer destination rect `(x, y, width, height)` and destination-to-source
/// matrix (row-major, source space local to `source`) for one strip.
/// Returns `None` for a zero-area strip.
pub fn strip_transform(
    strip: &WarpStrip,
    source: WarpRect,
) -> Option<((i32, i32, u32, u32), [[f64; 3]; 3])> {
    let x0 = strip.dst.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let x1 = strip.dst.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    let y0 = strip.dst.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let y1 = strip.dst.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let ix0 = x0.floor() as i32;
    let iy0 = y0.floor() as i32;
    let ix1 = x1.ceil() as i32;
    let iy1 = y1.ceil() as i32;
    if ix1 <= ix0 || iy1 <= iy0 {
        return None;
    }
    let kx = (strip.src_x1 - strip.src_x0) / (x1 - x0);
    let ky = source.height / (y1 - y0);
    let local_x0 = strip.src_x0 - source.x;
    Some((
        (ix0, iy0, (ix1 - ix0) as u32, (iy1 - iy0) as u32),
        [[kx, 0.0, local_x0 - x0 * kx], [0.0, ky, -y0 * ky], [0.0, 0.0, 1.0]],
    ))
}

/// Paints `capture` through `mesh` onto `dest`: one transformed XRenderComposite
/// per strip, with a bilinear filter when the library provides it.
///
/// Fails without touching `dest` when the transform extension is missing or
/// when the capture does not match `mesh.source`.
///
/// # Safety
///
/// `display` must be a live Xlib display and `dest` a live XRender picture on it.
pub unsafe fn paint_warped(
    display: *mut Display,
    api: &XRenderTransformApi,
    capture: &CapturedImage,
    mesh: &WarpMesh,
    dest: c_ulong,
) -> Result<(), String> {
    if display.is_null() {
        return Err("paint_warped: null display".to_string());
    }
    if !api.capabilities().picture_transform {
        return Err("paint_warped: XRenderSetPictureTransform unavailable".to_string());
    }
    check_capture_size(capture.width, capture.height)?;
    if f64::from(capture.width) != mesh.source.width || f64::from(capture.height) != mesh.source.height {
        return Err("paint_warped: capture size does not match mesh source".to_string());
    }
    // SAFETY: `display` is live per contract; the pixmap is freed below on every path.
    let pixmap = unsafe { upload_argb_pixmap(display, capture) }?;
    // SAFETY: `display` and `pixmap` are live; the picture is freed below.
    let source = match unsafe { api.create_argb_picture(display, pixmap) } {
        Ok(picture) => picture,
        Err(error) => {
            // SAFETY: `pixmap` was created above and is not used again.
            unsafe { XFreePixmap(display, pixmap) };
            return Err(error);
        }
    };
    // SAFETY: `source` and `dest` are live pictures on `display` per contract.
    let result = unsafe { paint_strips(display, api, source, dest, mesh) };
    // SAFETY: both resources were created above and are released exactly once.
    unsafe {
        api.free_picture(display, source);
        XFreePixmap(display, pixmap);
    }
    result
}

unsafe fn paint_strips(
    display: *mut Display,
    api: &XRenderTransformApi,
    source: c_ulong,
    dest: c_ulong,
    mesh: &WarpMesh,
) -> Result<(), String> {
    if api.capabilities().picture_filter {
        // SAFETY: `source` is a live picture on `display`.
        unsafe { api.set_bilinear_filter(display, source) }?;
    }
    for strip in &mesh.strips {
        let Some((rect, matrix)) = strip_transform(strip, mesh.source) else {
            continue;
        };
        // SAFETY: `source` and `dest` are live pictures on `display`.
        unsafe {
            api.set_transform(display, source, matrix)?;
            api.composite_src(display, source, dest, rect);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: WarpRect = WarpRect {
        x: 100.0,
        y: 50.0,
        width: 400.0,
        height: 300.0,
    };
    const TARGET: WarpRect = WarpRect {
        x: 20.0,
        y: 600.0,
        width: 60.0,
        height: 40.0,
    };

    #[test]
    fn t_zero_is_identity() {
        let mesh = magic_lamp_mesh(SOURCE, TARGET, 0.0, 16).unwrap();
        let first = mesh.strips[0];
        assert_eq!(first.dst[0], (SOURCE.x, SOURCE.y));
        let last = mesh.strips[15];
        assert_eq!(last.dst[1], (SOURCE.x + SOURCE.width, SOURCE.y));
        assert_eq!(last.dst[2], (SOURCE.x + SOURCE.width, SOURCE.y + SOURCE.height));
        assert_eq!(last.dst[3], (last.src_x0, SOURCE.y + SOURCE.height));
    }

    #[test]
    fn t_one_lands_on_target_rect() {
        let mesh = magic_lamp_mesh(SOURCE, TARGET, 1.0, 16).unwrap();
        for strip in &mesh.strips {
            for (x, y) in strip.dst {
                assert!(x >= TARGET.x && x <= TARGET.x + TARGET.width);
                assert!(y == TARGET.y || y == TARGET.y + TARGET.height);
            }
        }
        assert_eq!(mesh.strips[0].dst[0].0, TARGET.x);
        assert_eq!(mesh.strips[15].dst[1].0, TARGET.x + TARGET.width);
        assert_eq!(mesh.strips[0].dst[0].1, TARGET.y);
        assert_eq!(mesh.strips[0].dst[3].1, TARGET.y + TARGET.height);
    }

    #[test]
    fn strips_are_monotone_and_never_overlap() {
        for step in 0..=20 {
            let t = f64::from(step) / 20.0;
            let mesh = magic_lamp_mesh(SOURCE, TARGET, t, MAX_WARP_STRIPS).unwrap();
            for (index, strip) in mesh.strips.iter().enumerate() {
                let [tl, tr, br, bl] = strip.dst;
                assert!(tl.0 <= tr.0 && bl.0 <= br.0, "inverted strip at t={t}");
                assert!(tl.1 <= bl.1, "inverted height at t={t}");
                if let Some(next) = mesh.strips.get(index + 1) {
                    assert_eq!(tr.0, next.dst[0].0, "gap or overlap at t={t}");
                    assert_eq!(br.0, next.dst[3].0, "gap or overlap at t={t}");
                }
            }
        }
    }

    #[test]
    fn bottom_pinches_before_top() {
        let mesh = magic_lamp_mesh(SOURCE, TARGET, 0.5, 8).unwrap();
        let strip = mesh.strips[0];
        let top_width = strip.dst[1].0 - strip.dst[0].0;
        let bottom_width = strip.dst[2].0 - strip.dst[3].0;
        assert!(top_width > bottom_width, "genie curve must pinch the bottom first");
    }

    #[test]
    fn rejects_bad_inputs() {
        assert!(magic_lamp_mesh(SOURCE, TARGET, 0.5, 0).is_err());
        assert!(magic_lamp_mesh(SOURCE, TARGET, 0.5, MAX_WARP_STRIPS + 1).is_err());
        assert!(magic_lamp_mesh(SOURCE, TARGET, f64::NAN, 8).is_err());
        let flat = WarpRect { width: 0.0, ..TARGET };
        assert!(magic_lamp_mesh(SOURCE, flat, 0.5, 8).is_err());
    }

    #[test]
    fn strip_transform_maps_quad_bounds_to_source_strip() {
        let mesh = magic_lamp_mesh(SOURCE, TARGET, 0.0, 4).unwrap();
        let strip = mesh.strips[1];
        let ((x, y, w, h), m) = strip_transform(&strip, SOURCE).unwrap();
        assert_eq!((x, y), (strip.dst[0].0.floor() as i32, SOURCE.y.floor() as i32));
        assert_eq!((w, h), (100, 300));
        // Destination top-left maps to the strip's local source origin.
        let map = |dx: f64, dy: f64| {
            (
                m[0][0] * dx + m[0][1] * dy + m[0][2],
                m[1][0] * dx + m[1][1] * dy + m[1][2],
            )
        };
        let (sx, sy) = map(strip.dst[0].0, strip.dst[0].1);
        assert!((sx - (strip.src_x0 - SOURCE.x)).abs() < 1e-9);
        assert!(sy.abs() < 1e-9);
    }

    #[test]
    fn strip_transform_skips_zero_area() {
        let strip = WarpStrip {
            src_x0: 0.0,
            src_x1: 10.0,
            dst: [(5.0, 5.0); 4],
        };
        assert!(strip_transform(&strip, SOURCE).is_none());
    }

    #[test]
    fn paint_warped_fails_closed_without_x() {
        let Ok(api) = XRenderTransformApi::load() else {
            return;
        };
        let capture = CapturedImage {
            width: 400,
            height: 300,
            argb: vec![0; 400 * 300 * 4],
        };
        let mesh = magic_lamp_mesh(SOURCE, TARGET, 0.5, 8).unwrap();
        // SAFETY: a null display is rejected before any Xlib call.
        let result = unsafe { paint_warped(std::ptr::null_mut(), &api, &capture, &mesh, 0) };
        assert!(result.is_err());
    }
}
