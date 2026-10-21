//! Procedural navy background for the lock screen, RGBA8, fully opaque.

use flamewm_image_core::gaussian_blur_rgba;

use crate::layout;

/// Vertical gradient stops as (position, rgb).
const GRADIENT: [(f64, [f64; 3]); 3] = [
    (0.0, [28.0, 40.0, 90.0]),  // #1C285A
    (0.67, [32.0, 44.0, 96.0]), // #202C60
    (1.0, [51.0, 66.0, 114.0]), // #334272
];

/// Soft radial blob. `centre` and `radius_ref` are in relative/reference units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Blob {
    pub cx: f64,
    pub cy: f64,
    pub rgb: [f64; 3],
    pub opacity: f64,
    pub radius_ref: f64,
}

/// Blobs composited over the gradient, centre positions relative to the output size.
pub const BLOBS: [Blob; 3] = [
    Blob {
        cx: 0.15,
        cy: 0.55,
        rgb: [16.0, 23.0, 56.0],
        opacity: 0.27,
        radius_ref: 32.0,
    },
    Blob {
        cx: 0.75,
        cy: 0.23,
        rgb: [24.0, 24.0, 58.0],
        opacity: 0.30,
        radius_ref: 24.0,
    },
    Blob {
        cx: 0.82,
        cy: 0.86,
        rgb: [104.0, 117.0, 161.0],
        opacity: 0.13,
        radius_ref: 18.0,
    },
];

fn gradient_at(t: f64) -> [f64; 3] {
    let last = GRADIENT.len() - 1;
    for pair in GRADIENT.windows(2) {
        let (t0, c0) = pair[0];
        let (t1, c1) = pair[1];
        if t <= t1 {
            let k = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
            return [0, 1, 2].map(|i| c0[i] + (c1[i] - c0[i]) * k);
        }
    }
    GRADIENT[last].1
}

/// Reference navy background at `w`x`h`: gradient plus three soft blobs.
pub fn reference_navy(w: u32, h: u32) -> Vec<u8> {
    let (wu, hu) = (w as usize, h as usize);
    let s = layout::fit(w, h).s;
    let (wf, hf) = (f64::from(w), f64::from(h));

    // Per-column and per-row squared distance terms, normalised by radius,
    // so the inner loop is an add and a polynomial with no exp().
    let blob_terms: Vec<(Vec<f64>, Vec<f64>, f64)> = BLOBS
        .iter()
        .map(|b| {
            let r = (b.radius_ref * s).max(1.0);
            let (px, py) = (b.cx * wf, b.cy * hf);
            let cols = (0..wu)
                .map(|x| ((x as f64 + 0.5 - px) / r).powi(2))
                .collect();
            let rows = (0..hu)
                .map(|y| ((y as f64 + 0.5 - py) / r).powi(2))
                .collect();
            (cols, rows, b.opacity)
        })
        .collect();

    if wu == 0 || hu == 0 {
        return Vec::new();
    }
    let mut out = vec![0u8; wu * hu * 4];
    for (y, out_row) in out.chunks_exact_mut(wu * 4).enumerate() {
        let t = if hu > 1 {
            y as f64 / (hu - 1) as f64
        } else {
            0.0
        };
        let row = gradient_at(t);
        for (x, px) in out_row.chunks_exact_mut(4).enumerate() {
            let mut c = row;
            for (blob, (cols, rows, opacity)) in BLOBS.iter().zip(&blob_terms) {
                let u = cols[x] + rows[y];
                if u < 1.0 {
                    let falloff = 1.0 - u;
                    let a = opacity * falloff * falloff;
                    for i in 0..3 {
                        c[i] += (blob.rgb[i] - c[i]) * a;
                    }
                }
            }
            px.copy_from_slice(&[to_u8(c[0]), to_u8(c[1]), to_u8(c[2]), 255]);
        }
    }
    out
}

/// Blur radius at 1080p for a 100% blur, scaled by output height.
const BLUR_RADIUS_1080P: f32 = 24.0;

/// Blurs and darkens an opaque RGBA8 background in place, once, at build time.
/// `blur_percent` 0..=100 scales a 24 px radius at 1080p; `dim_percent` 0..=80 multiplies
/// RGB by `1 - dim/100`. 0/0 leaves the pixels untouched, bit for bit.
pub fn post_process(
    pixels: &mut [u8],
    w: u32,
    h: u32,
    blur_percent: u8,
    dim_percent: u8,
) -> Result<(), String> {
    if blur_percent > 0 && w > 0 && h > 0 {
        let radius =
            f32::from(blur_percent.min(100)) / 100.0 * BLUR_RADIUS_1080P * h as f32 / 1080.0;
        gaussian_blur_rgba(pixels, w, h, radius)
            .map_err(|error| format!("background blur: {error:?}"))?;
    }
    if dim_percent > 0 {
        let keep = 1.0 - f64::from(dim_percent.min(80)) / 100.0;
        for pixel in pixels.chunks_exact_mut(4) {
            for channel in &mut pixel[..3] {
                *channel = (f64::from(*channel) * keep).round() as u8;
            }
        }
    }
    Ok(())
}

/// Solid RGBA8 fill of `w`x`h`.
pub fn flat(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    let row = [rgb[0], rgb[1], rgb[2], 255].repeat(w as usize);
    let mut out = Vec::with_capacity(row.len() * h as usize);
    for _ in 0..h {
        out.extend_from_slice(&row);
    }
    out
}

fn to_u8(v: f64) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn px(buf: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * w as usize + x as usize) * 4;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    fn near(a: [u8; 4], b: [u8; 3], tol: i32) -> bool {
        (0..3).all(|i| (i32::from(a[i]) - i32::from(b[i])).abs() <= tol)
    }

    #[test]
    fn size_and_opaque_alpha() {
        let buf = reference_navy(64, 36);
        assert_eq!(buf.len(), 64 * 36 * 4);
        assert!(buf.chunks(4).all(|p| p[3] == 255));
    }

    #[test]
    fn top_and_bottom_rows_match_gradient_stops() {
        let (w, h) = (96, 54);
        let buf = reference_navy(w, h);
        let top_centre = px(&buf, w, w / 2, 0);
        assert!(near(top_centre, [28, 40, 90], 2), "{top_centre:?}");
        let bottom_left = px(&buf, w, 0, h - 1);
        assert!(near(bottom_left, [51, 66, 114], 2), "{bottom_left:?}");
    }

    #[test]
    fn deterministic_across_calls() {
        let a = reference_navy(320, 180);
        let b = reference_navy(320, 180);
        assert_eq!(a, b);
    }

    #[test]
    fn corner_samples_near_reference_colours() {
        let (w, h) = (1920, 1080);
        let buf = reference_navy(w, h);
        assert!(near(px(&buf, w, 0, 0), [0x1F, 0x2B, 0x5F], 12));
        assert!(near(px(&buf, w, w / 2, h - 1), [0x31, 0x3D, 0x71], 12));
        assert!(near(px(&buf, w, w - 1, h - 1), [0x33, 0x46, 0x6E], 12));
    }

    #[test]
    fn blob_opacity_within_bounds() {
        let bounds = [0.27, 0.30, 0.13];
        for (blob, bound) in BLOBS.iter().zip(bounds) {
            assert!(blob.opacity > 0.0 && blob.opacity <= bound + 1e-12);
            assert!((18.0..=32.0).contains(&blob.radius_ref));
        }
    }

    #[test]
    fn no_nan_or_degenerate_sizes() {
        for (w, h) in [(1, 1), (1, 40), (3, 2), (320, 240)] {
            let buf = reference_navy(w, h);
            assert_eq!(buf.len(), w as usize * h as usize * 4);
        }
        assert!(reference_navy(0, 0).is_empty());
        assert!(gradient_at(0.5).iter().all(|c| c.is_finite()));
    }

    #[test]
    fn flat_fills_opaque() {
        let buf = flat(3, 2, [10, 20, 30]);
        assert_eq!(buf.len(), 24);
        assert!(buf.chunks(4).all(|p| p == [10, 20, 30, 255]));
    }

    /// Pre-rewrite per-pixel `extend` algorithm, kept verbatim as the byte-for-byte oracle.
    fn old_reference_navy(w: u32, h: u32) -> Vec<u8> {
        let (wu, hu) = (w as usize, h as usize);
        let s = layout::fit(w, h).s;
        let (wf, hf) = (f64::from(w), f64::from(h));
        let blob_terms: Vec<(Vec<f64>, Vec<f64>, f64)> = BLOBS
            .iter()
            .map(|b| {
                let r = (b.radius_ref * s).max(1.0);
                let (px, py) = (b.cx * wf, b.cy * hf);
                let cols = (0..wu)
                    .map(|x| ((x as f64 + 0.5 - px) / r).powi(2))
                    .collect();
                let rows = (0..hu)
                    .map(|y| ((y as f64 + 0.5 - py) / r).powi(2))
                    .collect();
                (cols, rows, b.opacity)
            })
            .collect();
        let mut out = Vec::with_capacity(wu * hu * 4);
        for y in 0..hu {
            let t = if hu > 1 {
                y as f64 / (hu - 1) as f64
            } else {
                0.0
            };
            let row = gradient_at(t);
            for x in 0..wu {
                let mut c = row;
                for (blob, (cols, rows, opacity)) in BLOBS.iter().zip(&blob_terms) {
                    let u = cols[x] + rows[y];
                    if u < 1.0 {
                        let falloff = 1.0 - u;
                        let a = opacity * falloff * falloff;
                        for i in 0..3 {
                            c[i] += (blob.rgb[i] - c[i]) * a;
                        }
                    }
                }
                out.extend([to_u8(c[0]), to_u8(c[1]), to_u8(c[2]), 255]);
            }
        }
        out
    }

    fn old_flat(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        let count = w as usize * h as usize;
        let mut out = Vec::with_capacity(count * 4);
        for _ in 0..count {
            out.extend([rgb[0], rgb[1], rgb[2], 255]);
        }
        out
    }

    #[test]
    fn rewritten_fills_match_old_algorithm_byte_for_byte() {
        let sizes = [
            (0, 0),
            (0, 5),
            (5, 0),
            (1, 1),
            (1, 40),
            (3, 2),
            (64, 36),
            (320, 180),
            (333, 211),
        ];
        for (w, h) in sizes {
            assert_eq!(
                reference_navy(w, h),
                old_reference_navy(w, h),
                "reference_navy {w}x{h}"
            );
            assert_eq!(
                flat(w, h, [10, 20, 30]),
                old_flat(w, h, [10, 20, 30]),
                "flat {w}x{h}"
            );
        }
    }

    fn checker(w: u32, h: u32) -> Vec<u8> {
        (0..w * h)
            .flat_map(|i| {
                let v = if (i % w + i / w) % 2 == 0 { 40 } else { 220 };
                [v, v, v, 255]
            })
            .collect()
    }

    fn high_frequency(buf: &[u8], w: u32) -> f64 {
        let mut sum = 0.0;
        for y in 0..(buf.len() / 4 / w as usize) {
            for x in 1..w as usize {
                let a = buf[(y * w as usize + x - 1) * 4] as f64;
                let b = buf[(y * w as usize + x) * 4] as f64;
                sum += (a - b).abs();
            }
        }
        sum
    }

    #[test]
    fn zero_zero_is_bit_identical() {
        let original = checker(32, 18);
        let mut buf = original.clone();
        post_process(&mut buf, 32, 18, 0, 0).expect("process");
        assert_eq!(buf, original);
    }

    #[test]
    fn dim_50_halves_luminance_and_keeps_alpha() {
        let mut buf = vec![200, 100, 50, 255];
        post_process(&mut buf, 1, 1, 0, 50).expect("process");
        assert_eq!(buf, vec![100, 50, 25, 255]);
    }

    #[test]
    fn blur_reduces_high_frequency_energy() {
        // 1080 rows so the radius is the full 24 px; a short image would blur under one pixel.
        let original = checker(64, 1080);
        let mut blurred = original.clone();
        post_process(&mut blurred, 64, 1080, 100, 0).expect("process");
        assert!(high_frequency(&blurred, 64) < high_frequency(&original, 64) * 0.5);
    }

    #[test]
    fn percentages_are_clamped() {
        let mut dim = vec![200, 200, 200, 255];
        post_process(&mut dim, 1, 1, 0, 255).expect("process");
        assert_eq!(dim[0], 40, "dim clamps to 80%: 200 * 0.2");
        let mut blur_a = checker(16, 9);
        let mut blur_b = checker(16, 9);
        post_process(&mut blur_a, 16, 9, 100, 0).expect("process");
        post_process(&mut blur_b, 16, 9, 250, 0).expect("process");
        assert_eq!(blur_a, blur_b, "blur above 100 behaves as 100");
    }

    #[test]
    #[ignore = "timing probe: run with --release --ignored --nocapture"]
    fn timing_1080p() {
        let start = Instant::now();
        let buf = reference_navy(1920, 1080);
        let elapsed = start.elapsed();
        println!(
            "reference_navy 1920x1080: {elapsed:?} ({} bytes)",
            buf.len()
        );
    }
}
