pub use flamewm_render_core::lower::{MAX_BLUR_RADIUS_PX, blur_radius_px};

/// Blur runs on a snapshot shrunk by this factor once the radius is large: the blur cost falls with
/// the pixel count, and the soft result is upscaled bilinearly. Below twice the factor the blur is
/// already cheap, so it stays at full resolution.
pub(crate) const BLUR_DOWNSCALE: u32 = 4;

/// Opaque ARGB32 words (0xFFRRGGBB) for a straight RGBA8 snapshot, blurred per
/// [`blur_radius_px`]. Takes ownership so the blur runs in place without a copy.
/// Blur failure keeps the unblurred pixels.
#[must_use]
pub fn blurred_argb_words(
    width: u32,
    height: u32,
    mut rgba: Vec<u8>,
    opacity_percent: u8,
    blur_percent: u8,
) -> Vec<u32> {
    let radius = blur_radius_px(opacity_percent, blur_percent);
    if radius > 0.0 && crate::accel::gpu_blur_rgba(&mut rgba, width, height, radius) {
        return opaque_argb_words(&rgba);
    }
    if radius >= 2.0 * BLUR_DOWNSCALE as f32 {
        let (mut small, small_w, small_h) = box_downscale(&rgba, width, height, BLUR_DOWNSCALE);
        // The original full-resolution input is no longer needed. Release it
        // before the costly blur/upsample rather than keeping both alive.
        drop(rgba);
        blur_in_place_slice(&mut small, small_w, small_h, radius / BLUR_DOWNSCALE as f32);
        return bilinear_argb_words(&small, small_w, small_h, width, height);
    }
    if radius > 0.0 {
        blur_in_place_slice(&mut rgba, width, height, radius);
    }
    opaque_argb_words(&rgba)
}

pub(super) fn opaque_argb_words(rgba: &[u8]) -> Vec<u32> {
    rgba.chunks_exact(4)
        .map(|p| 0xff00_0000 | (u32::from(p[0]) << 16) | (u32::from(p[1]) << 8) | u32::from(p[2]))
        .collect()
}

/// Gaussian blur through the shared scratch buffers. A failure keeps the input pixels.
pub(crate) fn blur_in_place_slice(pixels: &mut [u8], width: u32, height: u32, radius: f32) {
    crate::accel::blur_rgba(pixels, width, height, radius);
}

/// Averages `factor` by `factor` blocks (edge blocks average what is inside the image).
pub(super) fn box_downscale(
    rgba: &[u8],
    width: u32,
    height: u32,
    factor: u32,
) -> (Vec<u8>, u32, u32) {
    let (small_w, small_h) = (width.div_ceil(factor), height.div_ceil(factor));
    let mut out = vec![0u8; (small_w * small_h * 4) as usize];
    for sy in 0..small_h {
        for sx in 0..small_w {
            let mut sum = [0u32; 4];
            let mut count = 0u32;
            for y in sy * factor..((sy + 1) * factor).min(height) {
                for x in sx * factor..((sx + 1) * factor).min(width) {
                    let i = ((y * width + x) * 4) as usize;
                    for (channel, total) in sum.iter_mut().enumerate() {
                        *total += u32::from(rgba[i + channel]);
                    }
                    count += 1;
                }
            }
            let o = ((sy * small_w + sx) * 4) as usize;
            for (channel, total) in sum.iter().enumerate() {
                out[o + channel] = (total / count) as u8;
            }
        }
    }
    (out, small_w, small_h)
}

/// Bilinear taps `(lower index, upper index, weight of the upper tap out of 256)` mapping each of
/// `out` target coordinates onto a source of `len` samples.
pub(super) fn bilinear_taps(len: u32, out: u32) -> Vec<(usize, usize, u32)> {
    (0..out).map(|i| bilinear_tap(len, out, i)).collect()
}

/// The [`bilinear_taps`] entry for target coordinate `i`.
#[inline]
pub(super) fn bilinear_tap(len: u32, out: u32, i: u32) -> (usize, usize, u32) {
    let scale = len as f32 / out as f32;
    let f = ((i as f32 + 0.5) * scale - 0.5).clamp(0.0, (len - 1) as f32);
    let lo = f.floor();
    let hi = (lo as u32 + 1).min(len - 1);
    (lo as usize, hi as usize, ((f - lo) * 256.0).round() as u32)
}

/// One source row blended horizontally at every column tap, unrounded (`channel * 256`), so the
/// vertical blend in [`BlurredCanvas::crop`] yields exactly the words of [`bilinear_word`].
pub(super) fn horizontal_line(row: &[u8], cols: &[(usize, usize, u32)], line: &mut [u32]) {
    for (&(a, b, wx), out) in cols.iter().zip(line.chunks_exact_mut(3)) {
        let (a_off, b_off) = (a * 4, b * 4);
        for (channel, value) in out.iter_mut().enumerate() {
            *value =
                u32::from(row[a_off + channel]) * (256 - wx) + u32::from(row[b_off + channel]) * wx;
        }
    }
}

/// One opaque ARGB32 word blended from two source rows at a column tap and a row weight.
#[inline]
pub(super) fn bilinear_word(
    top_row: &[u8],
    bottom_row: &[u8],
    col: (usize, usize, u32),
    wy: u32,
) -> u32 {
    let (a, b, wx) = (col.0 * 4, col.1 * 4, col.2);
    let mut word = 0xff00_0000u32;
    for channel in 0..3usize {
        let top =
            u32::from(top_row[a + channel]) * (256 - wx) + u32::from(top_row[b + channel]) * wx;
        let bottom = u32::from(bottom_row[a + channel]) * (256 - wx)
            + u32::from(bottom_row[b + channel]) * wx;
        let value = (top * (256 - wy) + bottom * wy + (1 << 15)) >> 16;
        word |= value << (16 - 8 * channel);
    }
    word
}

/// Bilinear upscale of a small RGBA image to opaque ARGB32 words for a `width` x `height` target.
/// Weights are 8-bit fixed point and the taps are computed once per column and per row.
pub(super) fn bilinear_argb_words(
    small: &[u8],
    small_w: u32,
    small_h: u32,
    width: u32,
    height: u32,
) -> Vec<u32> {
    let (cols, rows) = (
        bilinear_taps(small_w, width),
        bilinear_taps(small_h, height),
    );
    let stride = small_w as usize * 4;
    let mut words = Vec::with_capacity((width * height) as usize);
    for &(r0, r1, wy) in &rows {
        let (top_row, bottom_row) = (&small[r0 * stride..], &small[r1 * stride..]);
        words.extend(
            cols.iter()
                .map(|&col| bilinear_word(top_row, bottom_row, col, wy)),
        );
    }
    words
}
