#[cfg(test)]
mod cases {
    use super::super::*;
    use std::time::{Duration, Instant};

    fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        rgba.repeat(width as usize * height as usize)
    }

    fn rect(x: i32, y: i32, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn pixel_at(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * width as usize + x as usize) * 4;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    }

    fn noisy(width: u32, height: u32) -> Vec<u8> {
        let mut seed = 0x9e37_79b9u32;
        (0..width * height * 4)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 24) as u8
            })
            .collect()
    }

    #[test]
    fn radius_zero_is_bit_exact_no_op() {
        let original = noisy(17, 9);
        for radius in [0.0f32, 0.49, -3.0] {
            let mut pixels = original.clone();
            gaussian_blur_rgba(&mut pixels, 17, 9, radius).unwrap();
            assert_eq!(pixels, original, "radius {radius}");
        }
    }

    #[test]
    fn constant_colour_is_unchanged_for_any_radius() {
        for rgba in [[200, 100, 50, 255], [200, 100, 50, 128], [0, 0, 0, 0]] {
            for radius in [0.5f32, 3.0, 11.0, 12.5, 24.0, 40.0, 64.0] {
                let mut pixels = solid(37, 29, rgba);
                gaussian_blur_rgba(&mut pixels, 37, 29, radius).unwrap();
                assert!(
                    pixels.chunks_exact(4).all(|p| p == rgba),
                    "rgba {rgba:?} radius {radius}"
                );
            }
        }
    }

    #[test]
    fn single_white_pixel_spreads_symmetrically_and_conserves_mass() {
        const N: usize = 61;
        let c = N / 2;
        let red = |x: usize, y: usize| (y * N + x) * 4;
        for radius in [4.0f32, 8.0] {
            // Float work buffer, checked before u8 quantisation.
            let mut scratch = BlurScratch::new();
            scratch.work = vec![0.0; N * N * 4];
            for i in 0..N * N {
                scratch.work[i * 4 + 3] = 255.0;
            }
            scratch.work[red(c, c)] = 255.0;

            blur_boxes(
                &mut scratch.work,
                &mut scratch.line,
                &mut scratch.band,
                N,
                N,
                radius / 2.0,
            );
            let work = &scratch.work;

            let total: f64 = (0..N * N).map(|i| f64::from(work[i * 4])).sum();
            assert!(
                (total / 255.0 - 1.0).abs() < 0.01,
                "mass {total} radius {radius}"
            );
            let peak = work[red(c, c)];
            assert!(peak < 255.0 && peak > 0.0, "peak {peak} radius {radius}");
            for d in 1..=c {
                let left = work[red(c - d, c)];
                let right = work[red(c + d, c)];
                let up = work[red(c, c - d)];
                let down = work[red(c, c + d)];
                assert!((left - right).abs() < 1e-2, "horizontal symmetry d={d}");
                assert!((up - down).abs() < 1e-2, "vertical symmetry d={d}");
                assert!((left - up).abs() < 1e-2, "transpose symmetry d={d}");
                let inner = work[red(c + d - 1, c)];
                assert!(
                    right <= inner + 1e-3,
                    "monotone falloff d={d} radius {radius}"
                );
            }
        }
    }

    #[test]
    fn edge_clamping_keeps_white_half_white_at_border() {
        const W: u32 = 64;
        const H: u32 = 8;
        let mut pixels = Vec::with_capacity((W * H * 4) as usize);
        for _ in 0..H {
            for x in 0..W {
                let v = if x < W / 2 { 255 } else { 0 };
                pixels.extend_from_slice(&[v, v, v, 255]);
            }
        }
        gaussian_blur_rgba(&mut pixels, W, H, 8.0).unwrap();
        for y in 0..H {
            assert_eq!(
                pixel_at(&pixels, W, 0, y),
                [255, 255, 255, 255],
                "left border row {y}"
            );
            assert_eq!(
                pixel_at(&pixels, W, W - 1, y),
                [0, 0, 0, 255],
                "right border row {y}"
            );
        }
    }

    #[test]
    fn transparent_pixels_do_not_bleed_black() {
        const N: u32 = 32;
        let mut pixels = Vec::with_capacity((N * N * 4) as usize);
        for _ in 0..N {
            for x in 0..N {
                if x < N / 2 {
                    pixels.extend_from_slice(&[255, 255, 255, 255]);
                } else {
                    pixels.extend_from_slice(&[0, 0, 0, 0]);
                }
            }
        }
        gaussian_blur_rgba(&mut pixels, N, N, 4.0).unwrap();
        let mut seen_translucent = false;
        for x in N / 2..N / 2 + 4 {
            let p = pixel_at(&pixels, N, x, N / 2);
            if p[3] > 0 {
                seen_translucent = true;
                assert!(
                    p[0] >= 250 && p[1] >= 250 && p[2] >= 250,
                    "bleed at x={x}: {p:?}"
                );
            }
        }
        assert!(
            seen_translucent,
            "expected translucent pixels next to the edge"
        );
    }

    #[test]
    fn blur_is_deterministic() {
        let original = noisy(73, 41);
        for radius in [2.0f32, 9.0, 20.0, 50.0] {
            let mut first = original.clone();
            let mut second = original.clone();
            gaussian_blur_rgba(&mut first, 73, 41, radius).unwrap();
            gaussian_blur_rgba(&mut second, 73, 41, radius).unwrap();
            assert_eq!(first, second, "radius {radius}");
        }
    }

    #[test]
    fn scratch_reuse_matches_fresh_allocation() {
        let original = noisy(40, 30);
        let mut scratch = BlurScratch::new();
        let mut reused = original.clone();
        gaussian_blur_rgba_with(&mut scratch, &mut reused, 40, 30, 20.0).unwrap();
        let mut fresh = original.clone();
        gaussian_blur_rgba(&mut fresh, 40, 30, 20.0).unwrap();
        assert_eq!(reused, fresh);
    }

    #[test]
    fn invalid_arguments_are_rejected() {
        let mut short = vec![0u8; 10];
        assert_eq!(
            gaussian_blur_rgba(&mut short, 2, 2, 4.0),
            Err(BlurError::BufferLength {
                expected: 16,
                actual: 10
            })
        );
        let mut empty = Vec::new();
        assert_eq!(
            gaussian_blur_rgba(&mut empty, 0, 4, 4.0),
            Err(BlurError::ZeroDimension)
        );
        let mut wide = vec![0u8; 4097 * 4];
        assert_eq!(gaussian_blur_rgba(&mut wide, 4097, 1, 4.0), Ok(()));
        let mut px = vec![0u8; 4];
        assert_eq!(
            gaussian_blur_rgba(&mut px, 1, 1, f32::NAN),
            Err(BlurError::InvalidRadius)
        );
        assert_eq!(
            gaussian_blur_rgba(&mut px, 1, 1, 65.0),
            Err(BlurError::RadiusTooLarge)
        );
    }

    #[test]
    fn frosted_backdrop_with_matches_allocating_variant() {
        let src = RgbaImage::from_rgba8(40, 30, noisy(40, 30)).unwrap();
        let crop = rect(-5, 3, 25, 20);
        let mut scratch = BlurScratch::new();
        for radius in [0.0f32, 2.0, 9.0, 20.0] {
            let fresh = frosted_backdrop(&src, crop, radius, Some([10, 20, 30, 100]));
            let reused =
                frosted_backdrop_with(&mut scratch, &src, crop, radius, Some([10, 20, 30, 100]));
            assert_eq!(fresh.pixels, reused.pixels, "radius {radius}");
            assert_eq!((fresh.width, fresh.height), (reused.width, reused.height));
        }
    }

    #[test]
    fn frosted_backdrop_dirty_scratch_matches_fresh() {
        let src = RgbaImage::from_rgba8(40, 30, noisy(40, 30)).unwrap();
        let crop = rect(2, 1, 17, 11);
        let mut scratch = BlurScratch::new();
        // Leave a large, non-zero padded buffer behind so reuse must re-zero it.
        let _ = frosted_backdrop_with(&mut scratch, &src, rect(0, 0, 40, 30), 20.0, None);
        let fresh = frosted_backdrop(&src, crop, 6.0, None);
        let reused = frosted_backdrop_with(&mut scratch, &src, crop, 6.0, None);
        assert_eq!(fresh.pixels, reused.pixels);
    }

    #[test]
    fn frosted_backdrop_crops_with_edge_clamping() {
        let src = RgbaImage::from_rgba8(8, 8, solid(8, 8, [255, 255, 255, 255])).unwrap();
        let out = frosted_backdrop(&src, rect(-4, -4, 6, 6), 6.0, None);
        assert_eq!((out.width, out.height), (6, 6));
        assert!(
            out.pixels
                .chunks_exact(4)
                .all(|p| p == [255, 255, 255, 255])
        );
    }

    #[test]
    fn frosted_backdrop_crops_region_without_blur() {
        let mut pixels = Vec::new();
        for y in 0..4u8 {
            for x in 0..4u8 {
                pixels.extend_from_slice(&[x * 10, y * 10, 0, 255]);
            }
        }
        let src = RgbaImage::from_rgba8(4, 4, pixels).unwrap();
        let out = frosted_backdrop(&src, rect(1, 2, 2, 1), 0.0, None);
        assert_eq!(out.pixels, vec![10, 20, 0, 255, 20, 20, 0, 255]);
    }

    #[test]
    fn frosted_backdrop_tint_composites_over() {
        let src = RgbaImage::from_rgba8(2, 2, solid(2, 2, [255, 255, 255, 255])).unwrap();
        let out = frosted_backdrop(&src, rect(0, 0, 2, 2), 0.0, Some([0, 0, 0, 128]));
        for p in out.pixels.chunks_exact(4) {
            assert!((i32::from(p[0]) - 127).abs() <= 1, "{p:?}");
            assert_eq!(p[3], 255);
        }
    }

    /// Previous implementation (full f32 `tmp`, u8 `padded`, unbanded passes), kept
    /// verbatim as the byte-exactness oracle.
    mod oracle {
        use crate::RgbaImage;
        use crate::blur::{
            BlurError, DOWNSCALE_FACTOR, DOWNSCALE_RADIUS, MAX_BLUR_RADIUS, MIN_BLUR_RADIUS, Rect,
            box_radii, clamp_index, composite_over, pixel_f32, premultiply_pixel,
            unpremultiply_pixel, validate,
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

        fn box_vertical(
            src: &[f32],
            dst: &mut [f32],
            acc: &mut [f32],
            w: usize,
            h: usize,
            r: usize,
        ) {
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
    }

    #[test]
    fn gaussian_blur_matches_previous_implementation() {
        let mut scratch = BlurScratch::new();
        for (w, h) in [(61u32, 9u32), (37, 29), (13, 7), (1280, 31), (1, 1)] {
            let original = noisy(w, h);
            for radius in [3.0f32, 12.0, 13.0, 19.0, 24.0] {
                let mut expected = original.clone();
                oracle::gaussian_blur_rgba(&mut expected, w, h, radius).unwrap();
                let mut got = original.clone();
                gaussian_blur_rgba_with(&mut scratch, &mut got, w, h, radius).unwrap();
                assert_eq!(got, expected, "{w}x{h} radius {radius}");
            }
        }
    }

    #[test]
    fn frosted_backdrop_matches_previous_implementation() {
        let src = RgbaImage::from_rgba8(41, 23, noisy(41, 23)).unwrap();
        let crops = [
            rect(-5, 3, 25, 13),
            rect(30, -7, 11, 31),
            rect(0, 0, 41, 23),
            rect(3, 2, 1, 1),
        ];
        let mut scratch = BlurScratch::new();
        for crop in crops {
            for radius in [0.0f32, 3.0, 12.0, 13.0, 19.0, 24.0] {
                for tint in [None, Some([10, 20, 30, 100])] {
                    let expected = oracle::frosted_backdrop(&src, crop, radius, tint);
                    let got = frosted_backdrop_with(&mut scratch, &src, crop, radius, tint);
                    assert_eq!((got.width, got.height), (expected.width, expected.height));
                    assert_eq!(
                        got.pixels, expected.pixels,
                        "crop {crop:?} radius {radius} tint {tint:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn scratch_capacity_tracks_current_request_and_releases() {
        let mut scratch = BlurScratch::new();
        let mut big = noisy(1280, 31);
        gaussian_blur_rgba_with(&mut scratch, &mut big, 1280, 31, 12.0).unwrap();
        assert!(scratch.work.capacity() >= 1280 * 31 * 4);

        let mut small = noisy(16, 16);
        gaussian_blur_rgba_with(&mut scratch, &mut small, 16, 16, 3.0).unwrap();
        assert!(scratch.work.capacity() <= 2 * 16 * 16 * 4 + SHRINK_SLACK);

        let mut again = noisy(16, 16);
        gaussian_blur_rgba_with(&mut scratch, &mut again, 16, 16, 3.0).unwrap();
        let settled = scratch.work.capacity();
        gaussian_blur_rgba_with(&mut scratch, &mut again, 16, 16, 3.0).unwrap();
        assert_eq!(scratch.work.capacity(), settled);

        scratch.release();
        assert_eq!(scratch.work.capacity(), 0);
        assert_eq!(scratch.line.capacity(), 0);
        assert_eq!(scratch.band.capacity(), 0);
        assert_eq!(scratch.down.capacity(), 0);
    }

    #[test]
    fn fit_f32_preserves_overwritten_scratch_instead_of_zeroing_it() {
        let mut buffer = vec![3.0_f32; 16];
        fit_f32(&mut buffer, 16);
        assert_eq!(buffer, vec![3.0; 16]);
        fit_f32(&mut buffer, 4);
        assert_eq!(buffer, vec![3.0; 4]);
        fit_f32(&mut buffer, 8);
        assert_eq!(&buffer[..4], &[3.0; 4]);
        assert_eq!(&buffer[4..], &[0.0; 4]);
    }

    #[test]
    fn release_if_over_budget_keeps_useful_small_scratch() {
        let mut scratch = BlurScratch::new();
        scratch.work = vec![1.0; 100_000];
        scratch.down = vec![2.0; 32];
        scratch.line = vec![3.0; 128];
        scratch.band = vec![4.0; 128];
        scratch.taps = vec![(0, 0, 0.5); 8];
        scratch.release_if_over_budget(4096);
        assert_eq!(scratch.work.capacity(), 0);
        assert!(scratch.down.capacity() >= 32);
        assert!(scratch.line.capacity() >= 128);
        assert!(scratch.band.capacity() >= 128);
        assert!(scratch.taps.capacity() >= 8);
    }

    #[test]
    fn release_if_over_budget_preserves_output_and_small_scratch_reuse() {
        let original = noisy(128, 64);
        let mut scratch = BlurScratch::new();
        let mut actual = original.clone();
        gaussian_blur_rgba_with(&mut scratch, &mut actual, 128, 64, 3.0).unwrap();
        let expected = actual.clone();
        // 128*64*4 floats alone exceed this budget. Releasing scratch
        // after emitting pixels must not alter the result.
        scratch.release_if_over_budget(1024);
        assert_eq!(scratch.work.capacity(), 0);
        assert_eq!(scratch.down.capacity(), 0);
        assert_eq!(actual, expected);

        let mut again = original;
        gaussian_blur_rgba_with(&mut scratch, &mut again, 128, 64, 3.0).unwrap();
        assert_eq!(again, expected);
        let settled = scratch.work.capacity();
        scratch.release_if_over_budget(1_048_576);
        assert_eq!(scratch.work.capacity(), settled);
    }

    #[test]
    #[ignore = "timing; run with --release --include-ignored --nocapture"]
    fn timing_1920x1080_frosted_blur() {
        const W: u32 = 1920;
        const H: u32 = 1080;
        let base = noisy(W, H);
        let mut scratch = BlurScratch::new();
        for radius in [8.0f32, 24.0, 48.0] {
            let mut samples: Vec<Duration> = Vec::new();
            for iteration in 0..33 {
                let mut buf = base.clone();
                let start = Instant::now();
                gaussian_blur_rgba_with(&mut scratch, &mut buf, W, H, radius).unwrap();
                let elapsed = start.elapsed();
                if iteration >= 3 {
                    samples.push(elapsed);
                }
            }
            samples.sort();
            let p50 = samples[samples.len() / 2];
            let p95 = samples[samples.len() * 95 / 100];
            println!("blur 1920x1080 radius {radius}: p50 {p50:?} p95 {p95:?}");
        }
    }
}
