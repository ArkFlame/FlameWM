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
    let (w, h, f) = (width as usize, height as usize, factor as usize);
    let (small_w, small_h) = (w.div_ceil(f), h.div_ceil(f));
    let mut out = vec![0u8; small_w * small_h * 4];
    for (sy, out_row) in out.chunks_exact_mut(small_w * 4).enumerate() {
        let (y0, y1) = (sy * f, ((sy + 1) * f).min(h));
        for (sx, dst) in out_row.chunks_exact_mut(4).enumerate() {
            let (x0, x1) = (sx * f, ((sx + 1) * f).min(w));
            let mut sum = [0u32; 4];
            for y in y0..y1 {
                for px in rgba[(y * w + x0) * 4..(y * w + x1) * 4].chunks_exact(4) {
                    sum[0] += u32::from(px[0]);
                    sum[1] += u32::from(px[1]);
                    sum[2] += u32::from(px[2]);
                    sum[3] += u32::from(px[3]);
                }
            }
            let count = ((y1 - y0) * (x1 - x0)) as u32;
            for (out_px, total) in dst.iter_mut().zip(sum) {
                *out_px = (total / count) as u8;
            }
        }
    }
    (out, small_w as u32, small_h as u32)
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
    // Each small row's horizontal blend is shared by every output row that taps it, so it is
    // computed once per small row. The integers match `bilinear_word` exactly.
    let mut lines: Vec<Option<Vec<u32>>> = vec![None; small_h as usize];
    let mut words = Vec::with_capacity((width * height) as usize);
    for &(r0, r1, wy) in &rows {
        for r in [r0, r1] {
            if lines[r].is_none() {
                let mut line = vec![0u32; width as usize * 3];
                horizontal_line(&small[r * stride..(r + 1) * stride], &cols, &mut line);
                lines[r] = Some(line);
            }
        }
        let (Some(top), Some(bottom)) = (&lines[r0], &lines[r1]) else {
            unreachable!("both tapped rows were filled above");
        };
        words.extend(
            top.chunks_exact(3)
                .zip(bottom.chunks_exact(3))
                .map(|(t, b)| {
                    let blend = |channel: usize| {
                        (t[channel] * (256 - wy) + b[channel] * wy + (1 << 15)) >> 16
                    };
                    0xff00_0000 | (blend(0) << 16) | (blend(1) << 8) | blend(2)
                }),
        );
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pre-optimisation per-pixel bilinear word, kept as the reference the tiled code must match.
    fn bilinear_word_reference(
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

    /// The pre-optimisation box downscale (per-pixel channel loop), kept as the reference.
    fn box_downscale_reference(
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

    fn bilinear_argb_words_reference(
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
                    .map(|&col| bilinear_word_reference(top_row, bottom_row, col, wy)),
            );
        }
        words
    }

    /// Deterministic synthetic snapshot with hard edges and a gradient, so every tap matters.
    fn synthetic_rgba(width: u32, height: u32, seed: u32) -> Vec<u8> {
        let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        let mut out = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                let noise = (state >> 16) as u8 / 16;
                let edge = if (x / 37 + y / 23) % 2 == 0 { 200 } else { 40 };
                out.extend_from_slice(&[
                    (edge as u32 + u32::from(noise) + x) as u8,
                    (x * 255 / width.max(1)) as u8,
                    (y * 255 / height.max(1)) as u8,
                    255,
                ]);
            }
        }
        out
    }

    fn max_channel_diff(a: &[u32], b: &[u32]) -> u32 {
        a.iter()
            .zip(b)
            .flat_map(|(x, y)| {
                (0..3).map(move |c| {
                    let (xc, yc) = ((x >> (8 * c)) & 0xff, (y >> (8 * c)) & 0xff);
                    xc.abs_diff(yc)
                })
            })
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn box_downscale_matches_the_reference_exactly() {
        for (width, height) in [(301u32, 133u32), (64, 64), (5, 3), (1, 1)] {
            let rgba = synthetic_rgba(width, height, width ^ height);
            let fast = box_downscale(&rgba, width, height, BLUR_DOWNSCALE);
            let reference = box_downscale_reference(&rgba, width, height, BLUR_DOWNSCALE);
            assert_eq!(fast, reference, "{width}x{height}");
        }
    }

    #[test]
    fn bilinear_upscale_matches_the_reference_within_two_levels() {
        let (small_w, small_h) = (76u32, 34u32);
        let small = synthetic_rgba(small_w, small_h, 7);
        for (width, height) in [(301u32, 133u32), (300, 132), (5, 4)] {
            let fast = bilinear_argb_words(&small, small_w, small_h, width, height);
            let reference = bilinear_argb_words_reference(&small, small_w, small_h, width, height);
            assert!(max_channel_diff(&fast, &reference) <= 2, "{width}x{height}");
        }
    }

    /// New blur path against the old pipeline (reference downscale, the same kernel, reference
    /// upscale) on a synthetic image. Both use the shared kernel, so the difference is the
    /// tiling only and must stay within 2 levels per channel.
    #[test]
    fn blurred_argb_words_matches_the_reference_pipeline() {
        let (width, height) = (301u32, 213u32);
        let rgba = synthetic_rgba(width, height, 3);
        let radius = blur_radius_px(0, 100);
        assert!(
            radius >= 2.0 * BLUR_DOWNSCALE as f32,
            "the downscaled path must run"
        );
        let fast = blurred_argb_words(width, height, rgba.clone(), 0, 100);
        let (mut small, small_w, small_h) =
            box_downscale_reference(&rgba, width, height, BLUR_DOWNSCALE);
        blur_in_place_slice(&mut small, small_w, small_h, radius / BLUR_DOWNSCALE as f32);
        let reference = bilinear_argb_words_reference(&small, small_w, small_h, width, height);
        assert!(max_channel_diff(&fast, &reference) <= 2);
    }
}
