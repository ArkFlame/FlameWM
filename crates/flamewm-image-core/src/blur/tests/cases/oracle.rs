//! Previous implementation (full f32 `tmp`, u8 `padded`, unbanded passes), kept
//! verbatim as the byte-exactness oracle. Moved out of `tests.rs` unchanged.

use crate::RgbaImage;
use crate::blur::{
    BlurError, DOWNSCALE_FACTOR, DOWNSCALE_RADIUS, MAX_BLUR_RADIUS, MIN_BLUR_RADIUS, Rect,
    box_radii, clamp_index, composite_over, pixel_f32, premultiply_pixel, unpremultiply_pixel,
    validate,
};

pub fn gaussian_blur_rgba(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    radius_px: f32,
) -> Result<(), BlurError> {
    validate(pixels.len(), width, height, radius_px)?;
    if radius_px < MIN_BLUR_RADIUS {
        return Ok(());
    }
    blur_straight(pixels, width as usize, height as usize, radius_px);
    Ok(())
}

pub fn frosted_backdrop(
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
    let pw = cw + 2 * pad;
    let ph = ch + 2 * pad;
    let src_w = src.width as usize;
    let max_x = i64::from(src.width) - 1;
    let max_y = i64::from(src.height) - 1;
    let mut padded = vec![0u8; pw * ph * 4];
    for py in 0..ph {
        let sy = (i64::from(crop.y) + py as i64 - pad as i64).clamp(0, max_y) as usize;
        for px in 0..pw {
            let sx = (i64::from(crop.x) + px as i64 - pad as i64).clamp(0, max_x) as usize;
            let si = (sy * src_w + sx) * 4;
            let di = (py * pw + px) * 4;
            padded[di..di + 4].copy_from_slice(&src.pixels[si..si + 4]);
        }
    }
    if blur {
        blur_straight(&mut padded, pw, ph, radius);
    }
    let mut out = vec![0u8; cw * ch * 4];
    for y in 0..ch {
        let row = ((y + pad) * pw + pad) * 4;
        out[y * cw * 4..(y + 1) * cw * 4].copy_from_slice(&padded[row..row + cw * 4]);
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

fn blur_straight(pixels: &mut [u8], w: usize, h: usize, radius: f32) {
    let sigma = radius / 2.0;
    let mut tmp = Vec::new();
    let mut acc = Vec::new();
    if radius <= DOWNSCALE_RADIUS {
        let mut work = vec![0.0f32; w * h * 4];
        for (src, dst) in pixels.chunks_exact(4).zip(work.chunks_exact_mut(4)) {
            premultiply_pixel(src, dst);
        }
        blur_boxes(&mut work, &mut tmp, &mut acc, w, h, sigma);
        for (src, dst) in work.chunks_exact(4).zip(pixels.chunks_exact_mut(4)) {
            unpremultiply_pixel(src, dst);
        }
        return;
    }
    let factor = DOWNSCALE_FACTOR;
    let dw = w.div_ceil(factor);
    let dh = h.div_ceil(factor);
    let mut down = vec![0.0f32; dw * dh * 4];
    box_downsample_premultiplied(pixels, w, h, &mut down, dw, dh, factor);
    blur_boxes(&mut down, &mut tmp, &mut acc, dw, dh, sigma / factor as f32);
    let mut taps = Vec::new();
    bilinear_upscale_straight(&down, dw, dh, pixels, w, h, factor, &mut taps);
}

fn blur_boxes(
    buf: &mut [f32],
    tmp: &mut Vec<f32>,
    acc: &mut Vec<f32>,
    w: usize,
    h: usize,
    sigma: f32,
) {
    tmp.resize(buf.len(), 0.0);
    acc.resize(w * 4, 0.0);
    for r in box_radii(sigma) {
        if r == 0 {
            continue;
        }
        box_horizontal(buf, tmp, w, h, r);
        box_vertical(tmp, buf, acc, w, h, r);
    }
}

fn box_horizontal(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize) {
    let stride = w * 4;
    let inv = 1.0 / (2 * r + 1) as f32;
    let ri = r as isize;
    let last = w as isize - 1;
    for y in 0..h {
        let row = &src[y * stride..(y + 1) * stride];
        let out = &mut dst[y * stride..(y + 1) * stride];
        let mut sum = [0.0f32; 4];
        for k in -ri..=ri {
            let p = clamp_index(k, last) * 4;
            for c in 0..4 {
                sum[c] += row[p + c];
            }
        }
        for x in 0..w {
            let o = x * 4;
            for c in 0..4 {
                out[o + c] = sum[c] * inv;
            }
            let add = clamp_index(x as isize + ri + 1, last) * 4;
            let sub = clamp_index(x as isize - ri, last) * 4;
            for c in 0..4 {
                sum[c] += row[add + c] - row[sub + c];
            }
        }
    }
}

fn box_vertical(src: &[f32], dst: &mut [f32], acc: &mut [f32], w: usize, h: usize, r: usize) {
    let stride = w * 4;
    let inv = 1.0 / (2 * r + 1) as f32;
    let ri = r as isize;
    let last = h as isize - 1;
    acc.fill(0.0);
    for k in -ri..=ri {
        let o = clamp_index(k, last) * stride;
        for (a, s) in acc.iter_mut().zip(&src[o..o + stride]) {
            *a += *s;
        }
    }
    for y in 0..h {
        let out = &mut dst[y * stride..(y + 1) * stride];
        for (d, a) in out.iter_mut().zip(acc.iter()) {
            *d = *a * inv;
        }
        let add = clamp_index(y as isize + ri + 1, last) * stride;
        let sub = clamp_index(y as isize - ri, last) * stride;
        for i in 0..stride {
            acc[i] += src[add + i] - src[sub + i];
        }
    }
}

fn box_downsample_premultiplied(
    src: &[u8],
    w: usize,
    h: usize,
    dst: &mut [f32],
    dw: usize,
    dh: usize,
    factor: usize,
) {
    for dy in 0..dh {
        let (y0, y1) = (dy * factor, (dy * factor + factor).min(h));
        for dx in 0..dw {
            let (x0, x1) = (dx * factor, (dx * factor + factor).min(w));
            let mut sum = [0.0f32; 4];
            for y in y0..y1 {
                for x in x0..x1 {
                    let si = (y * w + x) * 4;
                    let mut premul = [0.0f32; 4];
                    premultiply_pixel(&src[si..si + 4], &mut premul);
                    for c in 0..4 {
                        sum[c] += premul[c];
                    }
                }
            }
            let inv = 1.0 / ((y1 - y0) * (x1 - x0)) as f32;
            let di = (dy * dw + dx) * 4;
            for c in 0..4 {
                dst[di + c] = sum[c] * inv;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn bilinear_upscale_straight(
    src: &[f32],
    sw: usize,
    sh: usize,
    dst: &mut [u8],
    dw: usize,
    dh: usize,
    factor: usize,
    taps: &mut Vec<(usize, usize, f32)>,
) {
    let f = factor as f32;
    taps.clear();
    for x in 0..dw {
        let sx = (((x as f32 + 0.5) / f) - 0.5).clamp(0.0, (sw - 1) as f32);
        let x0 = sx.floor() as usize;
        taps.push((x0, (x0 + 1).min(sw - 1), sx - x0 as f32));
    }
    for y in 0..dh {
        let sy = (((y as f32 + 0.5) / f) - 0.5).clamp(0.0, (sh - 1) as f32);
        let y0 = sy.floor() as usize;
        let y1 = (y0 + 1).min(sh - 1);
        let ty = sy - y0 as f32;
        let row0 = &src[y0 * sw * 4..(y0 + 1) * sw * 4];
        let row1 = &src[y1 * sw * 4..(y1 + 1) * sw * 4];
        let out = &mut dst[y * dw * 4..(y + 1) * dw * 4];
        for (o, &(x0, x1, tx)) in out.chunks_exact_mut(4).zip(taps.iter()) {
            let a = pixel_f32(row0, x0);
            let b = pixel_f32(row0, x1);
            let c = pixel_f32(row1, x0);
            let d = pixel_f32(row1, x1);
            let mut premul = [0.0f32; 4];
            for ch in 0..4 {
                let top = a[ch] + (b[ch] - a[ch]) * tx;
                let bottom = c[ch] + (d[ch] - c[ch]) * tx;
                premul[ch] = top + (bottom - top) * ty;
            }
            unpremultiply_pixel(&premul, o);
        }
    }
}
