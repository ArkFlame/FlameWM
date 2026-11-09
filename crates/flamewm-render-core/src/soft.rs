//! Dependency-free software rasterizer for `PixelOp` batches.
//!
//! `flamewm-render-x11` paints through XRender and Xft, so a session that is not on X11 had no
//! way to turn a `PaintCommand` list into pixels. This backend runs the same normalized ops on
//! a plain RGBA8 frame, which is exactly what a Wayland shared-memory buffer holds.

use crate::pixels::{PixelBackend, PixelOp, RoundedRect};

/// Whole-frame RGBA8 software painting with a retained blur scratch buffer.
#[derive(Default)]
pub struct SoftPixels {
    scratch: flamewm_image_core::blur::BlurScratch,
}

impl SoftPixels {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Paints `ops` onto `pixels` in order.
    ///
    /// # Errors
    /// Returns `Err` when the frame length disagrees with its size, or when an op fails.
    pub fn paint(
        &mut self,
        pixels: &mut [u8],
        width: u32,
        height: u32,
        ops: &[PixelOp<'_>],
    ) -> Result<(), String> {
        if pixels.len() != width as usize * height as usize * 4 {
            return Err("pixel buffer does not match its size".to_owned());
        }
        self.paint_batch(pixels, width, height, ops)
    }
}

impl PixelBackend for SoftPixels {
    fn blur_rgba(
        &mut self,
        pixels: &mut [u8],
        width: u32,
        height: u32,
        radius: f32,
    ) -> Result<(), String> {
        flamewm_image_core::blur::gaussian_blur_rgba_with(
            &mut self.scratch,
            pixels,
            width,
            height,
            radius,
        )
        .map_err(|error| format!("{error:?}"))
    }

    fn fill_rounded_rect(
        &mut self,
        pixels: &mut [u8],
        width: u32,
        height: u32,
        outer: RoundedRect,
        hole: Option<RoundedRect>,
        color: [u8; 4],
    ) -> Result<(), String> {
        fill_rounded_rect_cpu(pixels, width, height, outer, hole, color)
    }
}

/// Horizontal inset of `rect`'s rounded outline on one row: the corner curve, or 0 away
/// from the corners. Same curve the X11 painter uses, so both agree pixel for pixel.
fn row_inset(width: u32, height: u32, radius: u32, row: u32) -> u32 {
    if radius <= 1 || width == 0 || height == 0 {
        return 0;
    }
    let corner_row = row.min(height.saturating_sub(1).saturating_sub(row));
    if corner_row >= radius {
        return 0;
    }
    let r = f64::from(radius);
    let dy = r - (f64::from(corner_row) + 0.5);
    let dx = (r * r - dy * dy).max(0.0).sqrt();
    (r - dx).floor().clamp(0.0, r) as u32
}

/// Device x range `[start, end)` of `rect`'s rounded outline on device row `y`.
fn hole_row_span(rect: RoundedRect, y: i32) -> Option<(i32, i32)> {
    let row = u32::try_from(y - rect.y)
        .ok()
        .filter(|row| *row < rect.height)?;
    let inset = row_inset(rect.width, rect.height, rect.radius, row) as i32;
    let start = rect.x.saturating_add(inset);
    let end = rect
        .x
        .saturating_add(rect.width as i32)
        .saturating_sub(inset);
    Some((start, end.max(start)))
}

/// Per-channel lookup tables for [`blend_span`]: entry `d` is the same f32 expression the
/// per-pixel rule evaluates for destination value `d`, so the table path is bit-identical.
fn blend_luts(color: [u8; 4], alpha: f32) -> [[u8; 256]; 4] {
    let inverse = 1.0 - alpha;
    let [cr, cg, cb, ca] = color.map(f32::from);
    let mut luts = [[0u8; 256]; 4];
    for d in 0..256 {
        let v = d as f32;
        luts[0][d] = (cr * alpha + v * inverse).round() as u8;
        luts[1][d] = (cg * alpha + v * inverse).round() as u8;
        luts[2][d] = (cb * alpha + v * inverse).round() as u8;
        luts[3][d] = (ca + v * inverse).round() as u8;
    }
    luts
}

/// Source-over of the colour the tables were built for, one lookup per channel per pixel.
fn blend_span(span: &mut [u8], luts: &[[u8; 256]; 4]) {
    for pixel in span.chunks_exact_mut(4) {
        for (channel, lut) in pixel.iter_mut().zip(luts) {
            *channel = lut[usize::from(*channel)];
        }
    }
}

/// Source-over `color` through `outer` minus `hole`, the CPU twin of the X11 rounded fill.
///
/// # Errors
/// Returns `Err` when the frame length disagrees with its size.
pub fn fill_rounded_rect_cpu(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    outer: RoundedRect,
    hole: Option<RoundedRect>,
    color: [u8; 4],
) -> Result<(), String> {
    if pixels.len() != width as usize * height as usize * 4 {
        return Err("pixel buffer does not match its size".to_owned());
    }
    let outer = outer.normalized();
    let hole = hole.map(RoundedRect::normalized);
    let alpha = f32::from(color[3]) / 255.0;
    let luts = blend_luts(color, alpha);
    for row in 0..outer.height {
        let y = outer.y + row as i32;
        if !(0..height as i32).contains(&y) {
            continue;
        }
        let inset = row_inset(outer.width, outer.height, outer.radius, row) as i32;
        let start = (outer.x + inset).max(0);
        let end = (outer.x + outer.width as i32 - inset).min(width as i32);
        let (hole_start, hole_end) = hole
            .and_then(|hole| hole_row_span(hole, y))
            .unwrap_or((end, end));
        let row_base = y as usize * width as usize * 4;
        for (from, to) in [(start, hole_start.min(end)), (hole_end.max(start), end)] {
            if from < to {
                let span = &mut pixels[row_base + from as usize * 4..row_base + to as usize * 4];
                blend_span(span, &luts);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pre-table per-pixel rule, kept as the reference for the lookup-table path.
    fn blend_span_reference(span: &mut [u8], color: [u8; 4], alpha: f32) {
        let inverse = 1.0 - alpha;
        let [cr, cg, cb, ca] = color.map(f32::from);
        for pixel in span.chunks_exact_mut(4) {
            pixel[0] = (cr * alpha + f32::from(pixel[0]) * inverse).round() as u8;
            pixel[1] = (cg * alpha + f32::from(pixel[1]) * inverse).round() as u8;
            pixel[2] = (cb * alpha + f32::from(pixel[2]) * inverse).round() as u8;
            pixel[3] = (ca + f32::from(pixel[3]) * inverse).round() as u8;
        }
    }

    #[test]
    #[ignore]
    fn lut_blend_ab() {
        let color = [200, 10, 90, 180];
        let alpha = f32::from(color[3]) / 255.0;
        let luts = blend_luts(color, alpha);
        let mut span: Vec<u8> = (0..1920 * 4).map(|i| (i * 7) as u8).collect();
        for round in 0..2 {
            for variant in ["per_pixel", "lut"] {
                let start = std::time::Instant::now();
                for _ in 0..2000 {
                    if variant == "per_pixel" {
                        blend_span_reference(std::hint::black_box(&mut span), color, alpha);
                    } else {
                        blend_span(std::hint::black_box(&mut span), &luts);
                    }
                }
                println!(
                    "KLUT round {round} {variant}: {:.3} ns/px",
                    start.elapsed().as_nanos() as f64 / (2000.0 * 1920.0)
                );
            }
        }
    }

    #[test]
    fn lut_blend_is_bit_identical_to_per_pixel_rule() {
        let mut seed = 0xdead_beef_u32;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for color in [
            [200, 10, 90, 255],
            [30, 200, 60, 128],
            [0, 0, 0, 1],
            [255, 255, 255, 0],
        ] {
            let alpha = f32::from(color[3]) / 255.0;
            let luts = blend_luts(color, alpha);
            let mut want: Vec<u8> = (0..256 * 4).map(|i| i as u8).collect();
            let mut got = want.clone();
            blend_span_reference(&mut want, color, alpha);
            blend_span(&mut got, &luts);
            assert_eq!(got, want, "colour {color:?}");
            for _ in 0..200 {
                let mut a: Vec<u8> = (0..64 * 4).map(|_| rnd() as u8).collect();
                let mut b = a.clone();
                blend_span_reference(&mut a, color, alpha);
                blend_span(&mut b, &luts);
                assert_eq!(a, b, "colour {color:?}");
            }
        }
    }

    fn frame(width: u32, height: u32) -> Vec<u8> {
        vec![0u8; width as usize * height as usize * 4]
    }

    fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let index = (y as usize * width as usize + x as usize) * 4;
        [
            pixels[index],
            pixels[index + 1],
            pixels[index + 2],
            pixels[index + 3],
        ]
    }

    #[test]
    fn fills_a_rect_opaque() {
        let (width, height) = (4, 4);
        let mut pixels = frame(width, height);
        let mut soft = SoftPixels::new();
        soft.paint(
            &mut pixels,
            width,
            height,
            &[PixelOp::FillRoundedRect {
                outer: RoundedRect {
                    x: 1,
                    y: 1,
                    width: 2,
                    height: 2,
                    radius: 0,
                },
                hole: None,
                color: [255, 0, 0, 255],
            }],
        )
        .expect("paint");
        assert_eq!(pixel(&pixels, width, 1, 1), [255, 0, 0, 255]);
        assert_eq!(pixel(&pixels, width, 0, 0), [0, 0, 0, 0]);
    }

    #[test]
    fn a_stroke_leaves_the_hole_untouched() {
        let (width, height) = (4, 4);
        let mut pixels = frame(width, height);
        let mut soft = SoftPixels::new();
        soft.paint(
            &mut pixels,
            width,
            height,
            &[PixelOp::FillRoundedRect {
                outer: RoundedRect {
                    x: 0,
                    y: 0,
                    width: 4,
                    height: 4,
                    radius: 0,
                },
                hole: Some(RoundedRect {
                    x: 1,
                    y: 1,
                    width: 2,
                    height: 2,
                    radius: 0,
                }),
                color: [0, 255, 0, 255],
            }],
        )
        .expect("paint");
        assert_eq!(pixel(&pixels, width, 0, 0), [0, 255, 0, 255]);
        assert_eq!(pixel(&pixels, width, 1, 1), [0, 0, 0, 0], "hole kept");
    }

    #[test]
    fn a_corner_radius_leaves_the_corner_empty() {
        let (width, height) = (8, 8);
        let mut pixels = frame(width, height);
        let mut soft = SoftPixels::new();
        soft.paint(
            &mut pixels,
            width,
            height,
            &[PixelOp::FillRoundedRect {
                outer: RoundedRect {
                    x: 0,
                    y: 0,
                    width: 8,
                    height: 8,
                    radius: 4,
                },
                hole: None,
                color: [255, 255, 255, 255],
            }],
        )
        .expect("paint");
        assert_eq!(pixel(&pixels, width, 0, 0), [0, 0, 0, 0], "corner clipped");
        assert_eq!(pixel(&pixels, width, 4, 4), [255, 255, 255, 255]);
    }

    #[test]
    fn blends_a_mask_through_its_coverage() {
        let (width, height) = (2, 1);
        let mut pixels = frame(width, height);
        let mut soft = SoftPixels::new();
        soft.paint(
            &mut pixels,
            width,
            height,
            &[PixelOp::BlendMask {
                x: 0,
                y: 0,
                width: 2,
                height: 1,
                mask: &[0, 255],
                color: [0, 0, 255, 255],
            }],
        )
        .expect("paint");
        // Zero coverage composites nothing; full coverage copies the colour.
        assert_eq!(pixel(&pixels, width, 0, 0), [0, 0, 0, 0]);
        assert_eq!(pixel(&pixels, width, 1, 0), [0, 0, 255, 255]);
    }

    #[test]
    fn a_translucent_fill_composites_over_what_is_below() {
        let (width, height) = (1, 1);
        let mut pixels = vec![0u8, 0, 0, 255];
        let mut soft = SoftPixels::new();
        soft.paint(
            &mut pixels,
            width,
            height,
            &[PixelOp::FillRoundedRect {
                outer: RoundedRect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                    radius: 0,
                },
                hole: None,
                color: [255, 255, 255, 128],
            }],
        )
        .expect("paint");
        let got = pixel(&pixels, width, 0, 0);
        assert!(
            got[0] > 120 && got[0] < 135,
            "half white over black: {got:?}"
        );
    }

    #[test]
    fn rejects_a_frame_that_disagrees_with_its_size() {
        let mut pixels = frame(2, 2);
        let mut soft = SoftPixels::new();
        let error = soft
            .paint(&mut pixels, 3, 2, &[])
            .expect_err("size mismatch must fail");
        assert!(error.contains("does not match"), "{error}");
    }

    /// A light translucent fill (the Dock material, alpha 46) over an opaque mid-grey backdrop must lighten it.
    /// Expected: 255 * 46/255 + 128 * (1 - 46/255) = 151 per colour channel, alpha stays 255.
    #[test]
    fn light_translucent_fill_over_mid_grey_lightens_not_blackens() {
        let (width, height) = (4u32, 4u32);
        let mut pixels: Vec<u8> = (0..width * height)
            .flat_map(|_| [128u8, 128, 128, 255])
            .collect();
        let rect = RoundedRect {
            x: 0,
            y: 0,
            width,
            height,
            radius: 0,
        };
        fill_rounded_rect_cpu(
            &mut pixels,
            width,
            height,
            rect,
            None,
            [255, 255, 255, 0x2e],
        )
        .expect("fill");
        for px in pixels.chunks_exact(4) {
            assert_eq!(px, [151, 151, 151, 255], "light fill must lighten grey");
        }
    }
}
