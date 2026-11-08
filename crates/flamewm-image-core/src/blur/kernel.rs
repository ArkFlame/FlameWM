use super::*;

pub(super) fn validate(
    len: usize,
    width: u32,
    height: u32,
    radius_px: f32,
) -> Result<(), BlurError> {
    if width == 0 || height == 0 {
        return Err(BlurError::ZeroDimension);
    }
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(BlurError::BufferLength {
            expected: usize::MAX,
            actual: len,
        })?;
    if len != expected {
        return Err(BlurError::BufferLength {
            expected,
            actual: len,
        });
    }
    if !radius_px.is_finite() {
        return Err(BlurError::InvalidRadius);
    }
    if radius_px > MAX_BLUR_RADIUS {
        return Err(BlurError::RadiusTooLarge);
    }
    Ok(())
}

/// Source window (in padded-grid coordinates) written by `blur_emit`.
#[derive(Clone, Copy)]
pub(super) struct Window {
    pub(super) x: usize,
    pub(super) y: usize,
    pub(super) w: usize,
    pub(super) h: usize,
}

/// Blurs the `w` x `h` grid whose straight RGBA8 pixels are `sample(x, y)`, leaving
/// float state in `scratch`. Returns the downscale factor (1 = full resolution).
/// Requires `radius >= 0.5`.
pub(super) fn blur_stage(
    scratch: &mut BlurScratch,
    sample: impl Fn(usize, usize) -> [u8; 4],
    w: usize,
    h: usize,
    radius: f32,
) -> usize {
    let sigma = radius / 2.0;
    let (factor, grid, dw, dh, sigma) = if radius <= DOWNSCALE_RADIUS {
        fit_f32(&mut scratch.work, w * h * 4);
        for (y, row) in scratch.work.chunks_exact_mut(w * 4).enumerate() {
            for (x, dst) in row.chunks_exact_mut(4).enumerate() {
                premultiply_pixel(&sample(x, y), dst);
            }
        }
        (1, &mut scratch.work, w, h, sigma)
    } else {
        let (dw, dh) = (w.div_ceil(DOWNSCALE_FACTOR), h.div_ceil(DOWNSCALE_FACTOR));
        fit_f32(&mut scratch.down, dw * dh * 4);
        box_downsample_premultiplied(&sample, w, h, &mut scratch.down, dw, dh, DOWNSCALE_FACTOR);
        let down_sigma = sigma / DOWNSCALE_FACTOR as f32;
        (DOWNSCALE_FACTOR, &mut scratch.down, dw, dh, down_sigma)
    };
    blur_boxes(grid, &mut scratch.line, &mut scratch.band, dw, dh, sigma);
    factor
}

/// Writes window `win` of the grid staged by `blur_stage` (`w` x `h`, `factor`)
/// as straight RGBA8 into `dst` (`win.w` x `win.h`).
pub(super) fn blur_emit(
    scratch: &mut BlurScratch,
    factor: usize,
    w: usize,
    h: usize,
    win: Window,
    dst: &mut [u8],
) {
    if factor == 1 {
        for j in 0..win.h {
            for i in 0..win.w {
                let si = ((win.y + j) * w + win.x + i) * 4;
                let di = (j * win.w + i) * 4;
                unpremultiply_pixel(&scratch.work[si..si + 4], &mut dst[di..di + 4]);
            }
        }
        return;
    }
    let (dw, dh) = (w.div_ceil(factor), h.div_ceil(factor));
    bilinear_upscale_straight(&scratch.down, dw, dh, factor, win, dst, &mut scratch.taps);
}

/// Retains already-initialized storage for full-overwrite scratch buffers.
/// `work`, `down`, and `line` are entirely overwritten before consumption;
/// `band` is overwritten row by row before use and its accumulator is cleared.
/// Avoid writing zeros over the entire buffer before immediately replacing
/// them with blur samples. Newly grown tail elements are initialized safely.
pub(super) fn fit_f32(buf: &mut Vec<f32>, len: usize) {
    buf.truncate(len);
    if buf.capacity() > len.saturating_mul(2).saturating_add(SHRINK_SLACK) {
        buf.shrink_to(len);
    }
    buf.resize(len, 0.0);
}

/// Three box passes (horizontal then vertical each) approximating a Gaussian.
/// Result is left in `buf`; `line` and `band` are pass-local scratch.
pub(super) fn blur_boxes(
    buf: &mut [f32],
    line: &mut Vec<f32>,
    band: &mut Vec<f32>,
    w: usize,
    h: usize,
    sigma: f32,
) {
    for r in box_radii(sigma) {
        if r == 0 {
            continue;
        }
        box_horizontal(buf, line, w, r);
        box_vertical(buf, band, w, h, r);
    }
}

/// Box radii for three passes matching variance `sigma^2` (Kutskir's method).
pub(super) fn box_radii(sigma: f32) -> [usize; 3] {
    const N: f32 = 3.0;
    let w_ideal = (12.0 * sigma * sigma / N + 1.0).sqrt();
    let mut wl = w_ideal.floor() as i64;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wl_f = wl as f32;
    let m_ideal =
        (12.0 * sigma * sigma - N * wl_f * wl_f - 4.0 * N * wl_f - 3.0 * N) / (-4.0 * wl_f - 4.0);
    let m = m_ideal.round().clamp(0.0, N) as usize;
    let mut radii = [0usize; 3];
    for (i, radius) in radii.iter_mut().enumerate() {
        let size = if i < m { wl } else { wl + 2 };
        *radius = ((size - 1) / 2) as usize;
    }
    radii
}

/// Reads straight RGBA8 pixel at byte index `i`.
pub(super) fn rgba_at(px: &[u8], i: usize) -> [u8; 4] {
    [px[i], px[i + 1], px[i + 2], px[i + 3]]
}

/// Reads one premultiplied float pixel at pixel index `x` of a row.
pub(super) fn pixel_f32(row: &[f32], x: usize) -> [f32; 4] {
    let p: &[f32; 4] = row[x * 4..x * 4 + 4].try_into().unwrap_or(&[0.0; 4]);
    *p
}

pub(super) fn clamp_index(index: isize, last: isize) -> usize {
    index.clamp(0, last) as usize
}

/// Horizontal box pass over each row of `buf` (`w` px wide), in place. Each row is
/// copied to `line` first so the sliding window reads unmodified inputs.
pub(super) fn box_horizontal(buf: &mut [f32], line: &mut Vec<f32>, w: usize, r: usize) {
    let stride = w * 4;
    let pad = r * 4;
    // `line` holds the row edge-padded by `r` on both sides, so the window of output `x` is the
    // contiguous span `[x, x + 2r]` and needs no clamping. The sums use the same order as the
    // clamped form, so the output is bit-identical.
    // One extra tail pixel: the final slide reads one past the last window's `add` slot.
    fit_f32(line, stride + 2 * pad + 4);
    let inv = 1.0 / (2 * r + 1) as f32;
    for row in buf.chunks_exact_mut(stride) {
        let (head, rest) = line.split_at_mut(pad);
        let (body, tail) = rest.split_at_mut(stride);
        body.copy_from_slice(row);
        for chunk in head.chunks_exact_mut(4) {
            chunk.copy_from_slice(&row[..4]);
        }
        for chunk in tail.chunks_exact_mut(4) {
            chunk.copy_from_slice(&row[stride - 4..]);
        }
        // Lanes as fixed arrays: the four channels are independent, so each step is one 4-wide
        // add/sub/mul. Operation order per channel matches the scalar form exactly.
        let lane = |s: &[f32]| [s[0], s[1], s[2], s[3]];
        let mut sum = [0.0f32; 4];
        for window in line[..(2 * r + 1) * 4].chunks_exact(4) {
            let v = lane(window);
            for c in 0..4 {
                sum[c] += v[c];
            }
        }
        // Zipped iterators: the slide reads `line` without per-element bounds checks.
        let adds = line[(2 * r + 1) * 4..].chunks_exact(4);
        let subs = line.chunks_exact(4);
        for ((out, add), sub) in row.chunks_exact_mut(4).zip(adds).zip(subs) {
            for c in 0..4 {
                out[c] = sum[c] * inv;
                sum[c] += add[c] - sub[c];
            }
        }
    }
}

/// Vertical box pass over `buf` (`w` x `h`), in place, one whole row at a time. Each output
/// element sees the same operations in the same order as the per-column form.
pub(super) fn box_vertical(buf: &mut [f32], band: &mut Vec<f32>, w: usize, h: usize, r: usize) {
    let stride = w * 4;
    let inv = 1.0 / (2 * r + 1) as f32;
    let ri = r as isize;
    let last = h as isize - 1;
    // Whole rows, contiguous, so the adds and subtracts vectorize. `buf` row `y` is overwritten
    // at step `y`, but later steps still subtract it, so a ring of the last `r + 1` original rows
    // is kept in `band`. The ring rows follow the subtract order, and the accumulator sits after
    // them. The add row is read from `buf`, which still holds originals for rows after `y`.
    let slots = r + 1;
    fit_f32(band, (slots + 1) * stride);
    let (saved, acc) = band.split_at_mut(slots * stride);
    acc.fill(0.0);
    for k in -ri..=ri {
        let o = clamp_index(k, last) * stride;
        for (a, s) in acc.iter_mut().zip(&buf[o..o + stride]) {
            *a += *s;
        }
    }
    for y in 0..h {
        let slot = (y % slots) * stride;
        saved[slot..slot + stride].copy_from_slice(&buf[y * stride..(y + 1) * stride]);
        for (d, a) in buf[y * stride..(y + 1) * stride].iter_mut().zip(acc.iter()) {
            *d = *a * inv;
        }
        let add_row = clamp_index(y as isize + ri + 1, last);
        let sub_row = clamp_index(y as isize - ri, last);
        // The last row is already overwritten in `buf`, so take a row `<= y` from the ring.
        let add: &[f32] = if add_row > y {
            &buf[add_row * stride..(add_row + 1) * stride]
        } else {
            let s = (add_row % slots) * stride;
            &saved[s..s + stride]
        };
        let s = (sub_row % slots) * stride;
        let sub = &saved[s..s + stride];
        for ((a, p), q) in acc.iter_mut().zip(add).zip(sub) {
            *a += *p - *q;
        }
    }
}

/// Box-averages the premultiplied `w` x `h` grid `sample(x, y)` into `dst` (dw x dh).
pub(super) fn box_downsample_premultiplied(
    sample: &impl Fn(usize, usize) -> [u8; 4],
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
                    let mut premul = [0.0f32; 4];
                    premultiply_pixel(&sample(x, y), &mut premul);
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

/// Bilinear upscale of premultiplied `src` (sw x sh) into straight RGBA8 `dst`
/// (`win.w` x `win.h`), covering grid window `win` where each source pixel spans
/// `factor` grid pixels. `taps` caches per-column source indices and weights.
pub(super) fn bilinear_upscale_straight(
    src: &[f32],
    sw: usize,
    sh: usize,
    factor: usize,
    win: Window,
    dst: &mut [u8],
    taps: &mut Vec<(usize, usize, f32)>,
) {
    let f = factor as f32;
    taps.clear();
    for x in win.x..win.x + win.w {
        let sx = (((x as f32 + 0.5) / f) - 0.5).clamp(0.0, (sw - 1) as f32);
        let x0 = sx.floor() as usize;
        taps.push((x0, (x0 + 1).min(sw - 1), sx - x0 as f32));
    }
    for j in 0..win.h {
        let y = win.y + j;
        let sy = (((y as f32 + 0.5) / f) - 0.5).clamp(0.0, (sh - 1) as f32);
        let y0 = sy.floor() as usize;
        let y1 = (y0 + 1).min(sh - 1);
        let ty = sy - y0 as f32;
        let row0 = &src[y0 * sw * 4..(y0 + 1) * sw * 4];
        let row1 = &src[y1 * sw * 4..(y1 + 1) * sw * 4];
        let out = &mut dst[j * win.w * 4..(j + 1) * win.w * 4];
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

/// Straight RGBA8 to premultiplied float (colour scaled by alpha, 0..255 range).
pub(super) fn premultiply_pixel(src: &[u8], dst: &mut [f32]) {
    let a = f32::from(src[3]);
    let k = a * INV_255;
    dst[0] = f32::from(src[0]) * k;
    dst[1] = f32::from(src[1]) * k;
    dst[2] = f32::from(src[2]) * k;
    dst[3] = a;
}

/// Premultiplied float back to straight RGBA8. Alpha below half a unit yields
/// fully transparent black.
pub(super) fn unpremultiply_pixel(src: &[f32], dst: &mut [u8]) {
    let a = src[3].clamp(0.0, F255);
    if a < 0.5 {
        dst.copy_from_slice(&[0, 0, 0, 0]);
        return;
    }
    let k = F255 / a;
    for c in 0..3 {
        dst[c] = to_u8(src[c] * k);
    }
    dst[3] = to_u8(a);
}

/// Rounds to nearest in `[0, 255]`. Avoids `f32::round`, which is a libm call
/// without SSE4.1 on the default `x86_64` target.
pub(super) fn to_u8(value: f32) -> u8 {
    (value.clamp(0.0, 255.0) + 0.5) as u8
}

/// Source-over composite of straight `top` onto straight `pixel`, in place.
pub(super) fn composite_over(pixel: &mut [u8], top: [u8; 4]) {
    let ab = f32::from(pixel[3]) * INV_255;
    let at = f32::from(top[3]) * INV_255;
    let ao = at + ab * (1.0 - at);
    if ao <= 0.0 {
        pixel.copy_from_slice(&[0, 0, 0, 0]);
        return;
    }
    for c in 0..3 {
        let value = (f32::from(top[c]) * at + f32::from(pixel[c]) * ab * (1.0 - at)) / ao;
        pixel[c] = to_u8(value);
    }
    pixel[3] = to_u8(ao * F255);
}
