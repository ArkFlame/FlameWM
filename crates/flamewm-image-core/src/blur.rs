//! Gaussian-approximating RGBA8 blur for translucent "frosted" surfaces.
//!
//! Input and output are straight RGBA8. Colour is blurred premultiplied so
//! transparent pixels never bleed black. Each blur is three separable box
//! passes with running sums and clamp-to-edge sampling, so cost per pass is
//! O(pixels) regardless of radius. Radius above `DOWNSCALE_RADIUS` blurs a
//! box-downscaled copy (4x) and upscales bilinearly. Trade-off: high-radius output loses fine detail that a
//! full-resolution blur would keep, which is acceptable for frosted glass.

use crate::RgbaImage;

/// Largest accepted blur radius in pixels.
pub const MAX_BLUR_RADIUS: f32 = 64.0;
/// Radii below this are a no-op.
const MIN_BLUR_RADIUS: f32 = 0.5;
/// Above this radius the blur runs on a box-downscaled copy.
const DOWNSCALE_RADIUS: f32 = 12.0;
/// Integer downscale factor used above `DOWNSCALE_RADIUS`.
const DOWNSCALE_FACTOR: usize = 4;
/// Extra float capacity tolerated before `fit_f32` releases memory.
const SHRINK_SLACK: usize = 4096;
/// Inverse of 255, used in premultiplied conversions.
const INV_255: f32 = 1.0 / 255.0;
/// 255.0 for unpremultiplied conversions.
const F255: f32 = 255.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlurError {
    ZeroDimension,
    BufferLength { expected: usize, actual: usize },
    InvalidRadius,
    RadiusTooLarge,
}

/// Integer pixel rectangle in source coordinates. Origin may lie outside the
/// source image; out-of-range samples clamp to the nearest edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Reusable float work buffers. Keep one per thread to avoid per-call allocation.
/// Buffers hold only what the current blur needs; `release` drops all capacity.
#[derive(Debug, Default)]
pub struct BlurScratch {
    /// Premultiplied region, blurred in place (radius <= `DOWNSCALE_RADIUS`).
    work: Vec<f32>,
    /// Box-downscaled premultiplied copy (radius > `DOWNSCALE_RADIUS`).
    down: Vec<f32>,
    /// One row copy for in-place horizontal passes.
    line: Vec<f32>,
    /// Column-band copy plus accumulator for vertical passes.
    band: Vec<f32>,
    taps: Vec<(usize, usize, f32)>,
}

impl BlurScratch {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Frees all retained buffer capacity. The next blur reallocates as needed.
    pub fn release(&mut self) {
        *self = Self::default();
    }

    /// Discard unusually large temporary buffers once their output is complete.
    /// A small scratch remains reusable; a one-off full-resolution blur must not
    /// permanently pin tens of MiB in an otherwise idle UI process.
    /// This is deliberately called *after* emission, never during a blur.
    pub fn release_if_over_budget(&mut self, max_bytes: usize) {
        let float_bytes = std::mem::size_of::<f32>();
        let tap_bytes = std::mem::size_of::<(usize, usize, f32)>();
        let mut sizes = [
            self.work.capacity().saturating_mul(float_bytes),
            self.down.capacity().saturating_mul(float_bytes),
            self.line.capacity().saturating_mul(float_bytes),
            self.band.capacity().saturating_mul(float_bytes),
            self.taps.capacity().saturating_mul(tap_bytes),
        ];
        let mut retained = sizes
            .iter()
            .fold(0usize, |sum, bytes| sum.saturating_add(*bytes));
        // Retire the largest allocation first, retaining useful smaller scratch
        // (row/band buffers and taps) whenever it fits the budget. This avoids
        // reallocation on the next small blur after a one-off oversized image.
        while retained > max_bytes {
            let Some((index, &bytes)) = sizes.iter().enumerate().max_by_key(|(_, size)| **size)
            else {
                break;
            };
            if bytes == 0 {
                break;
            }
            match index {
                0 => self.work = Vec::new(),
                1 => self.down = Vec::new(),
                2 => self.line = Vec::new(),
                3 => self.band = Vec::new(),
                4 => self.taps = Vec::new(),
                _ => unreachable!(),
            }
            sizes[index] = 0;
            retained = retained.saturating_sub(bytes);
        }
    }
}

/// Blurs straight RGBA8 `pixels` in place.
pub fn gaussian_blur_rgba(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    radius_px: f32,
) -> Result<(), BlurError> {
    gaussian_blur_rgba_with(&mut BlurScratch::new(), pixels, width, height, radius_px)
}

/// Same as [`gaussian_blur_rgba`], reusing `scratch` buffers.
pub fn gaussian_blur_rgba_with(
    scratch: &mut BlurScratch,
    pixels: &mut [u8],
    width: u32,
    height: u32,
    radius_px: f32,
) -> Result<(), BlurError> {
    validate(pixels.len(), width, height, radius_px)?;
    if radius_px < MIN_BLUR_RADIUS {
        return Ok(());
    }
    let (w, h) = (width as usize, height as usize);
    let factor = blur_stage(
        scratch,
        |x, y| rgba_at(pixels, (y * w + x) * 4),
        w,
        h,
        radius_px,
    );
    blur_emit(scratch, factor, w, h, Window { x: 0, y: 0, w, h }, pixels);
    Ok(())
}

/// Crops `crop` from `src` (edge-clamped), blurs it, and optionally composites
/// `tint` (straight RGBA, source-over) on top. The blur runs on a region padded
/// by `2 * radius` on each side, filled from clamped source pixels, so edges do
/// not darken. Radius is clamped to `[0, MAX_BLUR_RADIUS]`; NaN means 0.
#[must_use]
pub fn frosted_backdrop(
    src: &RgbaImage,
    crop: Rect,
    radius_px: f32,
    tint: Option<[u8; 4]>,
) -> RgbaImage {
    frosted_backdrop_with(&mut BlurScratch::new(), src, crop, radius_px, tint)
}

/// Same as [`frosted_backdrop`], reusing `scratch` buffers.
#[must_use]
pub fn frosted_backdrop_with(
    scratch: &mut BlurScratch,
    src: &RgbaImage,
    crop: Rect,
    radius_px: f32,
    tint: Option<[u8; 4]>,
) -> RgbaImage {
    if src.width == 0 || src.height == 0 {
        return RgbaImage {
            width: 1,
            height: 1,
            pixels: vec![0, 0, 0, 0],
        };
    }
    let radius = if radius_px.is_nan() {
        0.0
    } else {
        radius_px.clamp(0.0, MAX_BLUR_RADIUS)
    };
    let blur = radius >= MIN_BLUR_RADIUS;
    let pad = if blur {
        (2.0 * radius).ceil() as usize
    } else {
        0
    };
    let cw = crop.width.max(1) as usize;
    let ch = crop.height.max(1) as usize;
    let (pw, ph) = (cw + 2 * pad, ch + 2 * pad);
    // Reject crops whose output or padded working buffers cannot be sized in
    // `usize` instead of wrapping and allocating a bogus length.
    let sized = |w: usize, h: usize| w.checked_mul(h).and_then(|n| n.checked_mul(4));
    if sized(cw, ch).is_none() || sized(pw, ph).is_none() {
        return RgbaImage {
            width: 1,
            height: 1,
            pixels: vec![0, 0, 0, 0],
        };
    }
    let src_w = src.width as usize;
    let max_x = i64::from(src.width) - 1;
    let max_y = i64::from(src.height) - 1;
    // Padded-grid sample: crop origin shifted by `pad`, clamped to the source.
    let sample = |px: usize, py: usize| -> [u8; 4] {
        let sx = (i64::from(crop.x) + px as i64 - pad as i64).clamp(0, max_x) as usize;
        let sy = (i64::from(crop.y) + py as i64 - pad as i64).clamp(0, max_y) as usize;
        rgba_at(&src.pixels, (sy * src_w + sx) * 4)
    };
    let mut out = vec![0u8; cw * ch * 4];
    if blur {
        let factor = blur_stage(scratch, sample, pw, ph, radius);
        let win = Window {
            x: pad,
            y: pad,
            w: cw,
            h: ch,
        };
        blur_emit(scratch, factor, pw, ph, win, &mut out);
    } else {
        for y in 0..ch {
            for x in 0..cw {
                out[(y * cw + x) * 4..][..4].copy_from_slice(&sample(x, y));
            }
        }
    }
    if let Some(tint) = tint {
        for pixel in out.chunks_exact_mut(4) {
            composite_over(pixel, tint);
        }
    }
    RgbaImage {
        width: cw as u32,
        height: ch as u32,
        pixels: out,
    }
}

mod kernel;

use kernel::*;

mod frosted_profile;

mod horizontal_golden;

mod tests;
