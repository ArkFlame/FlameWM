//! Deterministic RGBA8 scaling: nearest-neighbor, bilinear, and box-area.

use crate::RgbaImage;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScaleFilter {
    Nearest,
    Bilinear,
    /// Box-average (area coverage) filter. Preferred for downscaling: each
    /// output pixel is the coverage-weighted mean of the source pixels it spans.
    Area,
}

/// `Bilinear` requests that shrink by more than this factor (on either axis)
/// are promoted to `Area` so downscaled art does not alias.
const AREA_DOWNSCALE_THRESHOLD_NUM: u64 = 3;
const AREA_DOWNSCALE_THRESHOLD_DEN: u64 = 2;

/// Scales to an exact target size. Returns `None` on size overflow.
/// Bilinear operates per-channel including alpha (straight alpha); border
/// samples clamp to the edge.
#[must_use]
pub fn scale_rgba(
    source: &RgbaImage,
    width: u32,
    height: u32,
    filter: ScaleFilter,
) -> Option<RgbaImage> {
    if width == 0 || height == 0 || source.width == 0 || source.height == 0 {
        return None;
    }
    let pixel_count = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)?;
    let mut pixels = vec![0u8; pixel_count];
    let shrinks = u64::from(source.width) * AREA_DOWNSCALE_THRESHOLD_DEN
        > u64::from(width) * AREA_DOWNSCALE_THRESHOLD_NUM
        || u64::from(source.height) * AREA_DOWNSCALE_THRESHOLD_DEN
            > u64::from(height) * AREA_DOWNSCALE_THRESHOLD_NUM;
    let filter = match filter {
        ScaleFilter::Bilinear if shrinks => ScaleFilter::Area,
        other => other,
    };
    match filter {
        ScaleFilter::Nearest => scale_nearest(source, width, height, &mut pixels),
        ScaleFilter::Bilinear => scale_bilinear(source, width, height, &mut pixels),
        ScaleFilter::Area => scale_area(source, width, height, &mut pixels),
    }
    RgbaImage::from_rgba8(width, height, pixels)
}

fn scale_nearest(source: &RgbaImage, width: u32, height: u32, output: &mut [u8]) {
    let target_w = width as usize;
    let target_h = height as usize;
    let src_w = source.width as usize;
    let src_h = source.height as usize;
    // The mapping is one-dimensional per axis: compute each source index once, not per pixel.
    let columns: Vec<usize> = (0..target_w).map(|x| x * src_w / target_w).collect();
    let rows: Vec<usize> = (0..target_h).map(|y| y * src_h / target_h).collect();
    for (y, &source_y) in rows.iter().enumerate() {
        let row = &source.pixels[source_y * src_w * 4..][..src_w * 4];
        let out_row = &mut output[y * target_w * 4..][..target_w * 4];
        for (pixel, &source_x) in out_row.as_chunks_mut::<4>().0.iter_mut().zip(&columns) {
            pixel.copy_from_slice(&row[source_x * 4..source_x * 4 + 4]);
        }
    }
}

fn scale_bilinear(source: &RgbaImage, width: u32, height: u32, output: &mut [u8]) {
    let target_w = width as usize;
    let target_h = height as usize;
    let src_w = source.width as usize;
    let src_h = source.height as usize;
    let x_ratio = src_w as f32 / target_w as f32;
    let y_ratio = src_h as f32 / target_h as f32;
    // The horizontal mapping is the same on every row: compute each column's two byte offsets and
    // weight once instead of once per pixel.
    let columns: Vec<(usize, usize, f32)> = (0..target_w)
        .map(|x| {
            let source_x = (x as f32 + 0.5) * x_ratio - 0.5;
            let x0 = source_x.floor().max(0.0) as usize;
            let x1 = (x0 + 1).min(src_w - 1);
            let fx = (source_x - x0 as f32).clamp(0.0, 1.0);
            (x0 * 4, x1 * 4, fx)
        })
        .collect();
    for y in 0..target_h {
        let source_y = (y as f32 + 0.5) * y_ratio - 0.5;
        let y0 = source_y.floor().max(0.0) as usize;
        let y1 = (y0 + 1).min(src_h - 1);
        let fy = (source_y - y0 as f32).clamp(0.0, 1.0);
        let row0 = &source.pixels[y0 * src_w * 4..][..src_w * 4];
        let row1 = &source.pixels[y1 * src_w * 4..][..src_w * 4];
        let out_row = &mut output[y * target_w * 4..][..target_w * 4];
        for (out, &(i0, i1, fx)) in out_row.as_chunks_mut::<4>().0.iter_mut().zip(&columns) {
            for (channel, slot) in out.iter_mut().enumerate() {
                let a = f32::from(row0[i0 + channel]);
                let b = f32::from(row0[i1 + channel]);
                let c = f32::from(row1[i0 + channel]);
                let d = f32::from(row1[i1 + channel]);
                let value = a * (1.0 - fx) * (1.0 - fy)
                    + b * fx * (1.0 - fy)
                    + c * (1.0 - fx) * fy
                    + d * fx * fy;
                *slot = value.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// Appends the coverage weights of one destination index to `weights` and returns the first
/// covered source index. `TapTable` uses this to build every tap of an axis in one pass.
fn push_area_tap(
    src_len: usize,
    dst_len: usize,
    destination: usize,
    weights: &mut Vec<f32>,
) -> usize {
    let ratio = src_len as f64 / dst_len as f64;
    let start = destination as f64 * ratio;
    let end = (destination + 1) as f64 * ratio;
    let first = start.floor() as usize;
    let last = (end.ceil() as usize).min(src_len);
    let base = weights.len();
    weights.extend((first..last).map(|index| {
        let lo = start.max(index as f64);
        let hi = end.min((index + 1) as f64);
        (hi - lo).max(0.0) as f32
    }));
    let total: f32 = weights[base..].iter().sum();
    if total > 0.0 {
        for weight in &mut weights[base..] {
            *weight /= total;
        }
    }
    first
}

/// Every tap of one axis, packed into three flat buffers instead of one `Vec` per destination
/// index: a 4000-to-1000 downscale needs three allocations, not two thousand.
pub(crate) struct TapTable {
    first: Vec<usize>,
    /// Start of each destination's weights in `weights`; has `dst_len + 1` entries.
    offsets: Vec<u32>,
    weights: Vec<f32>,
}

impl TapTable {
    pub(crate) fn new(src_len: usize, dst_len: usize) -> Self {
        let mut first = Vec::with_capacity(dst_len);
        let mut offsets = Vec::with_capacity(dst_len + 1);
        let mut weights = Vec::with_capacity(dst_len * 2);
        offsets.push(0);
        for destination in 0..dst_len {
            first.push(push_area_tap(src_len, dst_len, destination, &mut weights));
            offsets.push(weights.len() as u32);
        }
        Self {
            first,
            offsets,
            weights,
        }
    }

    pub(crate) fn tap(&self, destination: usize) -> (usize, &[f32]) {
        let start = self.offsets[destination] as usize;
        let end = self.offsets[destination + 1] as usize;
        (self.first[destination], &self.weights[start..end])
    }
}

fn scale_area(source: &RgbaImage, width: u32, height: u32, output: &mut [u8]) {
    let target_w = width as usize;
    let src_w = source.width as usize;
    let x_taps = TapTable::new(src_w, target_w);
    let y_taps = TapTable::new(source.height as usize, height as usize);
    for y in 0..height as usize {
        let (first_row, row_weights) = y_taps.tap(y);
        for x in 0..target_w {
            let (first_col, col_weights) = x_taps.tap(x);
            let mut acc = [0f32; 4];
            for (row_offset, row_weight) in row_weights.iter().enumerate() {
                let row = (first_row + row_offset) * src_w;
                for (col_offset, col_weight) in col_weights.iter().enumerate() {
                    let weight = row_weight * col_weight;
                    let index = (row + first_col + col_offset) * 4;
                    for (sum, &value) in acc.iter_mut().zip(&source.pixels[index..index + 4]) {
                        *sum += weight * f32::from(value);
                    }
                }
            }
            let output_index = (y * target_w + x) * 4;
            for channel in 0..4 {
                output[output_index + channel] = acc[channel].round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One destination index's taps, for the coverage-weight contract tests below.
    fn area_tap(src_len: usize, dst_len: usize, destination: usize) -> (usize, Vec<f32>) {
        let mut weights = Vec::new();
        let first = push_area_tap(src_len, dst_len, destination, &mut weights);
        (first, weights)
    }

    fn fixture_rgba() -> RgbaImage {
        RgbaImage::from_rgba8(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 128]).unwrap()
    }

    /// The per-pixel bilinear this module shipped before the column mapping was hoisted.
    fn bilinear_reference(source: &RgbaImage, width: u32, height: u32) -> Vec<u8> {
        let (target_w, target_h) = (width as usize, height as usize);
        let (src_w, src_h) = (source.width as usize, source.height as usize);
        let x_ratio = src_w as f32 / target_w as f32;
        let y_ratio = src_h as f32 / target_h as f32;
        let mut output = vec![0_u8; target_w * target_h * 4];
        for y in 0..target_h {
            let source_y = (y as f32 + 0.5) * y_ratio - 0.5;
            let y0 = source_y.floor().max(0.0) as usize;
            let y1 = (y0 + 1).min(src_h - 1);
            let fy = (source_y - y0 as f32).clamp(0.0, 1.0);
            for x in 0..target_w {
                let source_x = (x as f32 + 0.5) * x_ratio - 0.5;
                let x0 = source_x.floor().max(0.0) as usize;
                let x1 = (x0 + 1).min(src_w - 1);
                let fx = (source_x - x0 as f32).clamp(0.0, 1.0);
                let output_index = (y * target_w + x) * 4;
                let (p00, p10) = ((y0 * src_w + x0) * 4, (y0 * src_w + x1) * 4);
                let (p01, p11) = ((y1 * src_w + x0) * 4, (y1 * src_w + x1) * 4);
                for channel in 0..4 {
                    let a = f32::from(source.pixels[p00 + channel]);
                    let b = f32::from(source.pixels[p10 + channel]);
                    let c = f32::from(source.pixels[p01 + channel]);
                    let d = f32::from(source.pixels[p11 + channel]);
                    let value = a * (1.0 - fx) * (1.0 - fy)
                        + b * fx * (1.0 - fy)
                        + c * (1.0 - fx) * fy
                        + d * fx * fy;
                    output[output_index + channel] = value.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        output
    }

    #[test]
    fn hoisted_bilinear_is_bit_identical_to_the_per_pixel_version() {
        let (sw, sh) = (37_u32, 23_u32);
        let mut seed = 0x2545_f491_u32;
        let pixels: Vec<u8> = (0..sw * sh * 4)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed >> 8) as u8
            })
            .collect();
        let source = RgbaImage::from_rgba8(sw, sh, pixels).unwrap();
        // Upscale, the 1.33x shrink the lock wallpaper takes, equal size and a 1 px column.
        for (w, h) in [(80, 50), (28, 18), (37, 23), (1, 9), (9, 1), (50, 23)] {
            let mut fast = vec![0_u8; w as usize * h as usize * 4];
            scale_bilinear(&source, w, h, &mut fast);
            assert_eq!(fast, bilinear_reference(&source, w, h), "{w}x{h}");
        }
    }

    #[test]
    fn nearest_matches_expected_mapping() {
        let scaled = scale_rgba(&fixture_rgba(), 2, 2, ScaleFilter::Nearest).unwrap();
        assert_eq!(
            scaled.pixels,
            vec![
                255, 0, 0, 255, 0, 255, 0, 128, 255, 0, 0, 255, 0, 255, 0, 128
            ]
        );
    }

    #[test]
    fn bilinear_uniform_stays_uniform() {
        let source = RgbaImage::from_rgba8(
            2,
            2,
            vec![
                10, 20, 30, 200, 10, 20, 30, 200, 10, 20, 30, 200, 10, 20, 30, 200,
            ],
        )
        .unwrap();
        let scaled = scale_rgba(&source, 4, 4, ScaleFilter::Bilinear).unwrap();
        assert!(
            scaled
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| *pixel == [10, 20, 30, 200])
        );
    }

    fn checker_4x4() -> RgbaImage {
        let mut pixels = Vec::new();
        for y in 0..4 {
            for x in 0..4 {
                let v = if (x + y) % 2 == 0 { 0 } else { 255 };
                pixels.extend_from_slice(&[v, v, v, 255]);
            }
        }
        RgbaImage::from_rgba8(4, 4, pixels).unwrap()
    }

    #[test]
    fn area_uniform_colour_stays_constant() {
        let source = RgbaImage::from_rgba8(5, 3, [7, 99, 201, 180].repeat(15)).unwrap();
        let scaled = scale_rgba(&source, 2, 1, ScaleFilter::Area).unwrap();
        assert!(
            scaled
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [7, 99, 201, 180])
        );
    }

    #[test]
    fn area_conserves_energy_on_checkerboard() {
        // A 2x2 box average of a checkerboard is 127.5 per block; nearest or
        // point sampling would instead return pure 0 or 255 and fail this.
        let scaled = scale_rgba(&checker_4x4(), 2, 2, ScaleFilter::Area).unwrap();
        let source_sum: u32 = checker_4x4()
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| u32::from(p[0]))
            .sum();
        let output_sum: u32 = scaled
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| u32::from(p[0]) * 4)
            .sum();
        assert!(source_sum.abs_diff(output_sum) <= 4 * 4);
        assert!(
            scaled
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| (127..=128).contains(&p[0]))
        );
    }

    #[test]
    fn area_does_not_overshoot_source_range() {
        let source =
            RgbaImage::from_rgba8(3, 1, vec![40, 0, 0, 255, 200, 0, 0, 255, 120, 0, 0, 255])
                .unwrap();
        let scaled = scale_rgba(&source, 1, 1, ScaleFilter::Area).unwrap();
        assert!((40..=200).contains(&scaled.pixels[0]));
        assert_eq!(scaled.pixels[0], 120);
    }

    #[test]
    fn area_tap_weights_cover_downscale_and_upscale_edges() {
        assert_eq!(area_tap(5, 2, 0), (0, vec![0.4, 0.4, 0.2]));
        assert_eq!(area_tap(5, 2, 1), (2, vec![0.2, 0.4, 0.4]));
        assert_eq!(area_tap(3, 5, 0), (0, vec![1.0]));
        let (first, weights) = area_tap(3, 5, 1);
        assert_eq!(first, 0);
        assert_eq!(weights.len(), 2);
        assert!((weights[0] - 2.0 / 3.0).abs() < f32::EPSILON);
        assert!((weights[1] - 1.0 / 3.0).abs() < f32::EPSILON);
        assert_eq!(area_tap(3, 5, 4), (2, vec![1.0]));
    }

    #[test]
    fn area_downscale_keeps_frozen_odd_ratio_pixels() {
        let source = RgbaImage::from_rgba8(
            5,
            1,
            vec![
                0, 40, 0, 255, 10, 30, 10, 255, 20, 20, 20, 255, 30, 10, 30, 255, 40, 0, 40, 255,
            ],
        )
        .unwrap();

        let scaled = scale_rgba(&source, 2, 1, ScaleFilter::Area).unwrap();

        assert_eq!(scaled.pixels, vec![8, 32, 8, 255, 32, 8, 32, 255]);
    }

    #[test]
    fn bilinear_downscale_over_one_and_half_uses_area() {
        let source = checker_4x4();
        let bilinear = scale_rgba(&source, 2, 2, ScaleFilter::Bilinear).unwrap();
        let area = scale_rgba(&source, 2, 2, ScaleFilter::Area).unwrap();
        assert_eq!(bilinear.pixels, area.pixels);
    }

    #[test]
    #[ignore = "timing bench: run with --release --ignored --nocapture"]
    fn bench_area_vs_bilinear_256_to_64() {
        let source = RgbaImage::from_rgba8(
            256,
            256,
            (0..256 * 256 * 4).map(|i| (i % 251) as u8).collect(),
        )
        .unwrap();
        for filter in [ScaleFilter::Bilinear, ScaleFilter::Area] {
            let started = std::time::Instant::now();
            for _ in 0..200 {
                std::hint::black_box(scale_rgba(&source, 64, 64, filter).unwrap());
            }
            println!("{filter:?}: {:?} per 200 scales", started.elapsed());
        }
    }
}
