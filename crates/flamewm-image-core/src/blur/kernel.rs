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

/// Blurs the `w` x `h` grid whose premultiplied float rows are written by `fill_row(y, row)`,
/// leaving float state in `scratch`. Returns the downscale factor (1 = full resolution).
/// Requires `radius >= 0.5`. Rows are filled whole, so the per-pixel source read is not a call.
pub(super) fn blur_stage(
    scratch: &mut BlurScratch,
    fill_row: impl Fn(usize, &mut [f32]),
    w: usize,
    h: usize,
    radius: f32,
) -> usize {
    let sigma = radius / 2.0;
    let factor = blur_factor(radius);
    let (grid, dw, dh, sigma) = if factor == 1 {
        fit_f32(&mut scratch.work, w * h * 4);
        for (y, row) in scratch.work.chunks_exact_mut(w * 4).enumerate() {
            fill_row(y, row);
        }
        (&mut scratch.work, w, h, sigma)
    } else {
        let (dw, dh) = (w.div_ceil(factor), h.div_ceil(factor));
        fit_f32(&mut scratch.down, dw * dh * 4);
        box_downsample_rows(
            &fill_row,
            (w, h),
            &mut scratch.down,
            &mut scratch.line,
            (dw, dh),
            factor,
        );
        (&mut scratch.down, dw, dh, sigma / factor as f32)
    };
    blur_boxes(grid, &mut scratch.line, &mut scratch.band, dw, dh, sigma);
    factor
}

/// Downscale factor for `radius`: full resolution up to `HALF_DOWNSCALE_RADIUS`, 2x up to
/// `DOWNSCALE_RADIUS`, then `DOWNSCALE_FACTOR`. Sigma shrinks with the grid, so the blur stays
/// proportional and the upscale hides the coarser grid under the wider kernel.
pub(super) fn blur_factor(radius: f32) -> usize {
    if radius > DOWNSCALE_RADIUS {
        DOWNSCALE_FACTOR
    } else if radius > HALF_DOWNSCALE_RADIUS {
        2
    } else {
        1
    }
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
    bilinear_upscale_straight(
        &scratch.down,
        (dw, dh),
        factor,
        win,
        dst,
        &mut scratch.taps,
        &mut scratch.line,
    );
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
#[inline]
pub(super) fn rgba_at(px: &[u8], i: usize) -> [u8; 4] {
    [px[i], px[i + 1], px[i + 2], px[i + 3]]
}

/// Reads one premultiplied float pixel at pixel index `x` of a row.
#[cfg(test)]
#[inline]
pub(super) fn pixel_f32(row: &[f32], x: usize) -> [f32; 4] {
    let p: &[f32; 4] = row[x * 4..x * 4 + 4].try_into().unwrap_or(&[0.0; 4]);
    *p
}

#[inline]
pub(super) fn clamp_index(index: isize, last: isize) -> usize {
    index.max(0).min(last) as usize
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
        // Indexed loops, not iterator chains: the fast profile (opt-level 1) does not inline
        // zipped iterator adaptors, so each element was a call. Same operation order per channel.
        let mut sum = [0.0f32; 4];
        for j in 0..2 * r + 1 {
            for c in 0..4 {
                sum[c] += line[j * 4 + c];
            }
        }
        for x in 0..w {
            let (out, add, sub) = ((x * 4), (x + 2 * r + 1) * 4, x * 4);
            for c in 0..4 {
                row[out + c] = sum[c] * inv;
                sum[c] += line[add + c] - line[sub + c];
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
        scale_row(&mut buf[y * stride..(y + 1) * stride], acc, inv);
        let add_row = clamp_index(y as isize + ri + 1, last);
        let sub_row = clamp_index(y as isize - ri, last);
        // The last row is already overwritten in `buf`, so take a row `<= y` from the ring.
        let s = (sub_row % slots) * stride;
        let sub = &saved[s..s + stride];
        if add_row > y {
            slide_row(acc, &buf[add_row * stride..(add_row + 1) * stride], sub);
        } else {
            let s = (add_row % slots) * stride;
            slide_row(acc, &saved[s..s + stride], sub);
        }
    }
}

/// `dst[i] = src[i] * k` over one row. Indexed so the loop needs no iterator calls at opt-level 1.
#[inline]
fn scale_row(dst: &mut [f32], src: &[f32], k: f32) {
    let n = dst.len();
    let src = &src[..n];
    for i in 0..n {
        dst[i] = src[i] * k;
    }
}

/// `acc[i] += add[i] - sub[i]` over one row.
#[inline]
fn slide_row(acc: &mut [f32], add: &[f32], sub: &[f32]) {
    let n = acc.len();
    let (add, sub) = (&add[..n], &sub[..n]);
    for i in 0..n {
        acc[i] += add[i] - sub[i];
    }
}

/// Box-averages the premultiplied `w` x `h` grid, filled one row at a time by `fill_row`, into
/// `dst` (dw x dh). `line` holds one filled row. Each cell sums its block in row-major order, the
/// same order as a per-block loop, so the result is bit-identical to it.
pub(super) fn box_downsample_rows(
    fill_row: &impl Fn(usize, &mut [f32]),
    (w, h): (usize, usize),
    dst: &mut [f32],
    line: &mut Vec<f32>,
    (dw, dh): (usize, usize),
    factor: usize,
) {
    fit_f32(line, w * 4);
    dst.fill(0.0);
    for y in 0..h {
        fill_row(y, &mut line[..w * 4]);
        let drow = &mut dst[(y / factor) * dw * 4..(y / factor + 1) * dw * 4];
        // Walk whole blocks: a per-pixel `x / factor` is an integer division in the hot loop.
        for dx in 0..dw {
            let cell = &mut drow[dx * 4..dx * 4 + 4];
            for x in dx * factor..(dx * factor + factor).min(w) {
                let src = &line[x * 4..x * 4 + 4];
                for c in 0..4 {
                    cell[c] += src[c];
                }
            }
        }
    }
    for dy in 0..dh {
        let rows = (dy * factor + factor).min(h) - dy * factor;
        for dx in 0..dw {
            let cols = (dx * factor + factor).min(w) - dx * factor;
            let inv = 1.0 / (rows * cols) as f32;
            for c in &mut dst[(dy * dw + dx) * 4..(dy * dw + dx) * 4 + 4] {
                *c *= inv;
            }
        }
    }
}

/// Bilinear upscale of premultiplied `src` (sw x sh) into straight RGBA8 `dst`
/// (`win.w` x `win.h`), covering grid window `win` where each source pixel spans
/// `factor` grid pixels. `taps` caches per-column source indices and weights. Each output row
/// first lerps its two source rows into `vrow` (one pass over contiguous floats), then lerps
/// columns, so the per-pixel work is one horizontal lerp per channel.
pub(super) fn bilinear_upscale_straight(
    src: &[f32],
    (sw, sh): (usize, usize),
    factor: usize,
    win: Window,
    dst: &mut [u8],
    taps: &mut Vec<(usize, usize, f32)>,
    vrow: &mut Vec<f32>,
) {
    let f = factor as f32;
    taps.clear();
    for x in win.x..win.x + win.w {
        let sx = (((x as f32 + 0.5) / f) - 0.5).clamp(0.0, (sw - 1) as f32);
        let x0 = sx.floor() as usize;
        taps.push((x0 * 4, ((x0 + 1).min(sw - 1)) * 4, sx - x0 as f32));
    }
    let stride = sw * 4;
    fit_f32(vrow, stride);
    for j in 0..win.h {
        let y = win.y + j;
        let sy = (((y as f32 + 0.5) / f) - 0.5).clamp(0.0, (sh - 1) as f32);
        let y0 = sy.floor() as usize;
        let y1 = (y0 + 1).min(sh - 1);
        let ty = sy - y0 as f32;
        let row0 = &src[y0 * stride..(y0 + 1) * stride];
        let row1 = &src[y1 * stride..(y1 + 1) * stride];
        for ((v, &a), &b) in vrow.iter_mut().zip(row0).zip(row1) {
            *v = a + (b - a) * ty;
        }
        let out = &mut dst[j * win.w * 4..(j + 1) * win.w * 4];
        for (k, &(a0, a1, tx)) in taps.iter().enumerate() {
            let mut premul = [0.0f32; 4];
            for ch in 0..4 {
                let a = vrow[a0 + ch];
                premul[ch] = a + (vrow[a1 + ch] - a) * tx;
            }
            unpremultiply_pixel(&premul, &mut out[k * 4..k * 4 + 4]);
        }
    }
}

/// Premultiplies one straight RGBA8 row into float `dst` (same arithmetic as `premultiply_pixel`).
#[inline]
pub(super) fn premultiply_row(src: &[u8], dst: &mut [f32]) {
    let n = dst.len();
    let src = &src[..n];
    // Indexed by pixel, not `step_by`: iterator adaptors are calls at opt-level 1.
    for p in 0..n / 4 {
        let i = p * 4;
        let a = f32::from(src[i + 3]);
        let k = a * INV_255;
        dst[i] = f32::from(src[i]) * k;
        dst[i + 1] = f32::from(src[i + 1]) * k;
        dst[i + 2] = f32::from(src[i + 2]) * k;
        dst[i + 3] = a;
    }
}

/// Straight RGBA8 to premultiplied float (colour scaled by alpha, 0..255 range).
#[inline]
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
#[inline]
pub(super) fn unpremultiply_pixel(src: &[f32], dst: &mut [u8]) {
    let a = src[3].max(0.0).min(F255);
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
/// without SSE4.1 on the default `x86_64` target. `max`/`min` are intrinsics; `f32::clamp` is a
/// call at opt-level 1, and the bench runs at that level.
#[inline]
pub(super) fn to_u8(value: f32) -> u8 {
    (value.max(0.0).min(255.0) + 0.5) as u8
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
