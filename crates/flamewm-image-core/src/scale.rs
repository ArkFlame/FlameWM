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
    for y in 0..target_h {
        let source_y = y * src_h / target_h;
        for x in 0..target_w {
            let source_x = x * src_w / target_w;
            let source_index = (source_y * src_w + source_x) * 4;
            let output_index = (y * target_w + x) * 4;
            output[output_index..output_index + 4]
                .copy_from_slice(&source.pixels[source_index..source_index + 4]);
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
}

/// First covered source index and normalised coverage weights for one destination index.
pub(crate) fn area_tap(src_len: usize, dst_len: usize, destination: usize) -> (usize, Vec<f32>) {
    let ratio = src_len as f64 / dst_len as f64;
    let start = destination as f64 * ratio;
    let end = (destination + 1) as f64 * ratio;
    let first = start.floor() as usize;
    let last = (end.ceil() as usize).min(src_len);
    let mut weights: Vec<f32> = (first..last)
        .map(|index| {
            let lo = start.max(index as f64);
            let hi = end.min((index + 1) as f64);
            (hi - lo).max(0.0) as f32
        })
        .collect();
    let total: f32 = weights.iter().sum();
    if total > 0.0 {
        for weight in &mut weights {
            *weight /= total;
        }
    }
    (first, weights)
}

fn scale_area(source: &RgbaImage, width: u32, height: u32, output: &mut [u8]) {
    let target_w = width as usize;
    let src_w = source.width as usize;
    let x_taps: Vec<_> = (0..target_w)
        .map(|destination| area_tap(src_w, target_w, destination))
        .collect();
    let y_taps: Vec<_> = (0..height as usize)
        .map(|destination| area_tap(source.height as usize, height as usize, destination))
        .collect();
    for (y, (first_row, row_weights)) in y_taps.iter().enumerate() {
        for (x, (first_col, col_weights)) in x_taps.iter().enumerate() {
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

    fn fixture_rgba() -> RgbaImage {
        RgbaImage::from_rgba8(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 128]).unwrap()
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
                .chunks_exact(4)
                .all(|pixel| pixel == [10, 20, 30, 200])
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
                .chunks_exact(4)
                .all(|p| p == [7, 99, 201, 180])
        );
    }

    #[test]
    fn area_conserves_energy_on_checkerboard() {
        // A 2x2 box average of a checkerboard is 127.5 per block; nearest or
        // point sampling would instead return pure 0 or 255 and fail this.
        let scaled = scale_rgba(&checker_4x4(), 2, 2, ScaleFilter::Area).unwrap();
        let source_sum: u32 = checker_4x4()
            .pixels
            .chunks_exact(4)
            .map(|p| u32::from(p[0]))
            .sum();
        let output_sum: u32 = scaled
            .pixels
            .chunks_exact(4)
            .map(|p| u32::from(p[0]) * 4)
            .sum();
        assert!(source_sum.abs_diff(output_sum) <= 4 * 4);
        assert!(
            scaled
                .pixels
                .chunks_exact(4)
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
