use super::*;

#[test]
fn opaque_argb_bytes_swap_rgb_to_little_endian_bgra() {
    assert_eq!(
        opaque_argb_bytes(&[0x11, 0x22, 0x33, 0x00]),
        vec![0x33, 0x22, 0x11, 0xff]
    );
}

/// The outline strip upload is premultiplied, so its antialiased arc blends without a fringe:
/// a straight-alpha white at half coverage would otherwise read as full white.
#[test]
fn premultiplied_argb_bytes_scale_channels_by_alpha() {
    assert_eq!(
        premultiplied_argb_bytes(&[255, 255, 255, 128]),
        vec![128, 128, 128, 128]
    );
    assert_eq!(
        premultiplied_argb_bytes(&[0x11, 0x22, 0x33, 0]),
        vec![0, 0, 0, 0],
        "a zero-coverage pixel stays fully transparent"
    );
    assert_eq!(
        premultiplied_argb_bytes(&[0x11, 0x22, 0x33, 255]),
        vec![0x33, 0x22, 0x11, 255]
    );
}

#[test]
fn crop_clamps_to_the_output_and_samples_the_reduced_capture() {
    // Output 8x8 stored at 2x2: stored pixel (x, y) has R = 10 * (2y + x) + 1.
    let pixels: Vec<u8> = (0..4u8).flat_map(|i| [10 * i + 1, 0, 0, 255]).collect();
    let mut decorations = Decorations::default();
    decorations.set_desktop((8, 8), Vec::new(), pixels);
    // The top-left output pixel maps to the top-left stored pixel.
    let corner = decorations.crop((0, 0), 1, 1, 1.0).expect("desktop set");
    assert_eq!(corner[0], 1);
    // Past the bottom-right edge every sample clamps to the bottom-right stored pixel.
    let clamped = decorations
        .crop((100, 100), 2, 2, 1.0)
        .expect("desktop set");
    let reds: Vec<u8> = clamped.as_chunks::<4>().0.iter().map(|px| px[0]).collect();
    assert_eq!(reds, vec![31, 31, 31, 31]);
}

#[test]
fn desktop_capture_is_stored_at_reduced_size() {
    assert_eq!(stored_size((1280, 800)), (320, 200));
    assert_eq!(
        stored_size((1281, 801)),
        (321, 201),
        "partial blocks round up"
    );
    assert_eq!(stored_size((1, 1)), (1, 1));
}

#[test]
fn strip_downscale_box_averages_blocks_across_strips() {
    // 4x8 output in two 4-row strips; the top block is 0, the bottom block is 200 (every channel).
    let width = 4;
    let top = vec![0u8; width * 4 * 4];
    let bottom = vec![200u8; width * 4 * 4];
    let mut stored = vec![0u8; 2 * 4];
    downscale_strip(&top, (width, 0, 4), &mut stored, (1, 2));
    downscale_strip(&bottom, (width, 4, 4), &mut stored, (1, 2));
    assert_eq!(&stored[0..4], &[0, 0, 0, 0]);
    assert_eq!(&stored[4..8], &[200, 200, 200, 200]);
}

/// The opaque fast path must equal the general premultiply for every channel and alpha.
#[test]
fn premultiplied_argb_bytes_match_the_formula_for_every_channel_and_alpha() {
    for alpha in 0..=255_u32 {
        for channel in 0..=255_u32 {
            let rgba = [
                channel as u8,
                (255 - channel) as u8,
                (channel / 2) as u8,
                alpha as u8,
            ];
            let scale = |c: u8| ((u32::from(c) * alpha + 127) / 255) as u8;
            assert_eq!(
                super::premultiplied_argb_bytes(&rgba),
                vec![scale(rgba[2]), scale(rgba[1]), scale(rgba[0]), rgba[3]],
                "channel {channel} alpha {alpha}"
            );
        }
    }
}

/// The straightforward per-block average the strip downscale must reproduce exactly, edges included.
fn downscale_reference(
    strip: &[u8],
    (width, y0, rows): (usize, usize, usize),
    stored: &mut [u8],
    (sw, sh): (usize, usize),
) {
    let end = y0 + rows;
    for sy in (y0 / DESKTOP_SCALE)..end.div_ceil(DESKTOP_SCALE).min(sh) {
        for sx in 0..sw {
            let (mut sum, mut count) = ([0u32; 4], 0u32);
            for y in (sy * DESKTOP_SCALE).max(y0)..((sy + 1) * DESKTOP_SCALE).min(end) {
                for x in (sx * DESKTOP_SCALE)..((sx + 1) * DESKTOP_SCALE).min(width) {
                    for c in 0..4 {
                        sum[c] += u32::from(strip[((y - y0) * width + x) * 4 + c]);
                    }
                    count += 1;
                }
            }
            for c in 0..4 {
                stored[(sy * sw + sx) * 4 + c] = ((sum[c] + count / 2) / count.max(1)) as u8;
            }
        }
    }
}

#[test]
fn strip_downscale_matches_the_reference_for_full_and_partial_blocks() {
    // Widths and heights that are and are not multiples of the scale, strips split mid-image.
    for (width, height) in [(16_usize, 16_usize), (17, 13), (5, 9), (1, 1), (7, 4)] {
        let sw = width.div_ceil(DESKTOP_SCALE);
        let sh = height.div_ceil(DESKTOP_SCALE);
        let source: Vec<u8> = (0..width * height * 4)
            .map(|i| (i.wrapping_mul(37) ^ (i >> 3)) as u8)
            .collect();
        let (mut fast, mut slow) = (vec![0u8; sw * sh * 4], vec![0u8; sw * sh * 4]);
        let mut y0 = 0;
        while y0 < height {
            let rows = (2 * DESKTOP_SCALE).min(height - y0);
            let strip = &source[y0 * width * 4..(y0 + rows) * width * 4];
            downscale_strip(strip, (width, y0, rows), &mut fast, (sw, sh));
            downscale_reference(strip, (width, y0, rows), &mut slow, (sw, sh));
            y0 += rows;
        }
        assert_eq!(fast, slow, "{width}x{height}");
    }
}
