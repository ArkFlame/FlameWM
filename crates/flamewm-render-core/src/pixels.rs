//! Backend-neutral pixel operations. CPU and GPU renderers implement [`PixelBackend`]; X11 and
//! Wayland call the same operations and never copy shared rendering logic per backend.

/// Which pixel backend the `renderAcceleration` setting requests. `Cpu` is the default; `Gpu` is
/// opt-in and every GPU failure falls back to the CPU backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RenderAcceleration {
    #[default]
    Cpu,
    Gpu,
}

impl RenderAcceleration {
    #[must_use]
    pub fn from_setting(value: &str) -> Self {
        if value.trim().eq_ignore_ascii_case("gpu") {
            Self::Gpu
        } else {
            Self::Cpu
        }
    }

    #[must_use]
    pub const fn as_setting(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Gpu => "gpu",
        }
    }
}

/// Device-pixel rounded rectangle with its origin at the top-left of the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoundedRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub radius: u32,
}

impl RoundedRect {
    /// Clamps the corner radius to half the shorter side, the largest radius that still fits.
    /// Backends receive normalized rectangles only.
    #[must_use]
    pub fn normalized(self) -> Self {
        Self {
            radius: self.radius.min(self.width.min(self.height) / 2),
            ..self
        }
    }

    /// The rectangle `thickness` pixels inside this one, with the corner radius reduced by the
    /// same amount. A stroke of this rectangle is this rectangle minus its inset.
    #[must_use]
    pub fn inset(self, thickness: u32) -> Self {
        let double = thickness.saturating_mul(2);
        Self {
            x: self.x.saturating_add(thickness as i32),
            y: self.y.saturating_add(thickness as i32),
            width: self.width.saturating_sub(double),
            height: self.height.saturating_sub(double),
            radius: self.radius.saturating_sub(thickness),
        }
    }
}

/// One step of a frame batch. Steps apply in order to the same frame. The blend variants borrow
/// their pixels, so an op is cheap to copy and the caller keeps the coverage or image buffers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PixelOp<'a> {
    /// Source-over `color` through `outer` minus `hole`, as [`PixelBackend::fill_rounded_rect`].
    /// A stroke is a fill whose `hole` is the rectangle inset by the stroke width.
    FillRoundedRect {
        outer: RoundedRect,
        hole: Option<RoundedRect>,
        color: [u8; 4],
    },
    /// Whole-frame blur, as [`PixelBackend::blur_rgba`].
    Blur { radius: f32 },
    /// Source-over `color` through `mask`: one 8-bit coverage per pixel, row-major, `width` x
    /// `height`, with its top-left at device pixel (`x`, `y`). A pixel's alpha is
    /// `color[3] * mask / 255` and its colour is `color` straight. Pixels outside the frame are
    /// skipped. Text runs use this with an Xft A8 glyph mask, tinted by the text colour.
    BlendMask {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        mask: &'a [u8],
        color: [u8; 4],
    },
    /// Source-over of premultiplied RGBA8 `rgba` (`width` x `height`, row-major) at device pixel
    /// (`x`, `y`), as XRender `PictOpOver` composites a premultiplied ARGB32 image. Pixels outside
    /// the frame are skipped.
    BlendImage {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        rgba: &'a [u8],
    },
    /// A node backdrop: the blur (`radius` px, as [`PixelBackend::blur_rgba`]) of the pixels under
    /// `clip` (device x, y, width, height, inside the frame), written opaque where
    /// [`inside_node_shape`] holds for the rounded rect `node` with corner `corner`. Pixels outside
    /// that shape keep the frame. A zero radius or an empty clip is a no-op.
    BackdropBlur {
        clip: (i32, i32, u32, u32),
        node: (i32, i32, u32, u32),
        corner: u32,
        radius: f32,
    },
    /// `FillRoundedRect` composited only inside `clip` (device x, y, width, height, inside the
    /// frame). Pixels outside `clip` keep the frame, as a clipped AA stroke leaves them.
    FillRoundedRectClipped {
        outer: RoundedRect,
        hole: Option<RoundedRect>,
        color: [u8; 4],
        clip: (i32, i32, u32, u32),
    },
}

/// The clipped rounded fill on the CPU: the fill runs on a copy of the `clip` rows, shifted to the
/// copy's origin, and only those rows are written back. Pixels outside `clip` are never touched.
#[allow(clippy::too_many_arguments)]
fn fill_clipped_cpu<B: PixelBackend + ?Sized>(
    backend: &mut B,
    pixels: &mut [u8],
    width: u32,
    height: u32,
    outer: RoundedRect,
    hole: Option<RoundedRect>,
    color: [u8; 4],
    clip: (i32, i32, u32, u32),
) -> Result<(), String> {
    if pixels.len() != width as usize * height as usize * 4 {
        return Err("clipped fill frame does not match its size".to_owned());
    }
    let (x, y, w, h) = clip;
    if w == 0 || h == 0 {
        return Ok(());
    }
    if x < 0 || y < 0 || i64::from(x) + i64::from(w) > i64::from(width) || i64::from(y) + i64::from(h) > i64::from(height)
    {
        return Err("fill clip leaves the frame".to_owned());
    }
    let stride = width as usize * 4;
    let row_bytes = w as usize * 4;
    let mut region = Vec::with_capacity(row_bytes * h as usize);
    for row in 0..h as usize {
        let start = (y as usize + row) * stride + x as usize * 4;
        region.extend_from_slice(&pixels[start..start + row_bytes]);
    }
    let shift = |rect: RoundedRect| RoundedRect {
        x: rect.x - x,
        y: rect.y - y,
        ..rect
    };
    backend.fill_rounded_rect(&mut region, w, h, shift(outer), hole.map(shift), color)?;
    for row in 0..h as usize {
        let start = (y as usize + row) * stride + x as usize * 4;
        pixels[start..start + row_bytes]
            .copy_from_slice(&region[row * row_bytes..(row + 1) * row_bytes]);
    }
    Ok(())
}

/// Whether pixel `(px, py)` lies inside the rounded rect `node` (device x, y, width, height) with
/// corner radius `radius`, sampled at the pixel centre. The hard-edged test the node backdrop uses.
#[must_use]
pub fn inside_node_shape(px: i32, py: i32, node: (i32, i32, u32, u32), radius: u32) -> bool {
    let (x, y, width, height) = node;
    let lx = (px - x) as f32 + 0.5;
    let ly = (py - y) as f32 + 0.5;
    let (w, h) = (width as f32, height as f32);
    if lx < 0.0 || ly < 0.0 || lx > w || ly > h {
        return false;
    }
    let r = (radius as f32).min(w / 2.0).min(h / 2.0);
    let cx = lx.clamp(r, w - r);
    let cy = ly.clamp(r, h - r);
    (lx - cx).powi(2) + (ly - cy).powi(2) <= r * r
}

/// [`PixelOp::BackdropBlur`] on any backend: blurs the `clip` region through `backend.blur_rgba`
/// and writes it opaque inside the node shape. The clip must lie inside the frame.
///
/// # Errors
/// Returns `Err` when the clip leaves the frame, the frame length is wrong, or the blur fails.
/// `pixels` is untouched on error.
#[allow(clippy::too_many_arguments)]
pub fn backdrop_blur_cpu<B: PixelBackend + ?Sized>(
    backend: &mut B,
    pixels: &mut [u8],
    width: u32,
    height: u32,
    clip: (i32, i32, u32, u32),
    node: (i32, i32, u32, u32),
    corner: u32,
    radius: f32,
) -> Result<(), String> {
    if pixels.len() != width as usize * height as usize * 4 {
        return Err("backdrop frame does not match its size".to_owned());
    }
    let (x, y, w, h) = clip;
    if w == 0 || h == 0 || radius <= 0.0 {
        return Ok(());
    }
    if x < 0 || y < 0 || i64::from(x) + i64::from(w) > i64::from(width) || i64::from(y) + i64::from(h) > i64::from(height)
    {
        return Err("backdrop clip leaves the frame".to_owned());
    }
    let stride = width as usize * 4;
    let row_bytes = w as usize * 4;
    let mut region = Vec::with_capacity(row_bytes * h as usize);
    for row in 0..h as usize {
        let start = (y as usize + row) * stride + x as usize * 4;
        region.extend_from_slice(&pixels[start..start + row_bytes]);
    }
    backend.blur_rgba(&mut region, w, h, radius)?;
    for row in 0..h as i32 {
        for col in 0..w as i32 {
            let (px, py) = (x + col, y + row);
            if !inside_node_shape(px, py, node, corner) {
                continue;
            }
            let source = (row as usize * w as usize + col as usize) * 4;
            let dest = py as usize * stride + px as usize * 4;
            pixels[dest..dest + 3].copy_from_slice(&region[source..source + 3]);
            pixels[dest + 3] = 255;
        }
    }
    Ok(())
}

impl PixelOp<'_> {
    /// The op with its rectangles normalized, as the backends require.
    #[must_use]
    pub fn normalized(self) -> Self {
        match self {
            Self::FillRoundedRect { outer, hole, color } => Self::FillRoundedRect {
                outer: outer.normalized(),
                hole: hole.map(RoundedRect::normalized),
                color,
            },
            Self::FillRoundedRectClipped {
                outer,
                hole,
                color,
                clip,
            } => Self::FillRoundedRectClipped {
                outer: outer.normalized(),
                hole: hole.map(RoundedRect::normalized),
                color,
                clip,
            },
            other => other,
        }
    }
}

/// CPU compositing for [`PixelOp::BlendMask`] and [`PixelOp::BlendImage`] over a straight RGBA8
/// frame. Each pixel uses the same source-over formula as the fills: straight colour with
/// `a = color[3] * mask / 255` for masks, premultiplied colour with `a = rgba[3] / 255` for images.
///
/// # Errors
/// Returns `Err` for other ops, a frame whose length is not `width * height * 4`, or a source
/// buffer whose length does not match its rectangle. `pixels` is untouched on error.
pub fn composite_cpu(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    op: PixelOp<'_>,
) -> Result<(), String> {
    if pixels.len() != width as usize * height as usize * 4 {
        return Err("pixel buffer does not match its size".to_owned());
    }
    let (x, y, rect_width, rect_height, bytes_per_pixel) = match op {
        PixelOp::BlendMask {
            x,
            y,
            width,
            height,
            mask,
            ..
        } => (x, y, width, height, (mask.len(), 1)),
        PixelOp::BlendImage {
            x,
            y,
            width,
            height,
            rgba,
        } => (x, y, width, height, (rgba.len(), 4)),
        _ => return Err("not a blend op".to_owned()),
    };
    let (source_len, stride) = bytes_per_pixel;
    if source_len != rect_width as usize * rect_height as usize * stride {
        return Err("blend source does not match its size".to_owned());
    }
    // Columns of the rectangle that land inside the frame, shared by every row.
    let col_start = (-(x.min(0)) as i64).min(i64::from(rect_width)) as u32;
    let col_end = (i64::from(width) - i64::from(x)).clamp(0, i64::from(rect_width)) as u32;
    if col_start >= col_end {
        return Ok(());
    }
    let span = (col_end - col_start) as usize;
    let frame_x = (x + col_start as i32) as usize;
    for row in 0..rect_height {
        let frame_y = y + row as i32;
        if !(0..height as i32).contains(&frame_y) {
            continue;
        }
        let src_index = row as usize * rect_width as usize + col_start as usize;
        let dst_start = (frame_y as usize * width as usize + frame_x) * 4;
        let dst = &mut pixels[dst_start..dst_start + span * 4];
        match op {
            PixelOp::BlendMask { mask, color, .. } => {
                blend_mask_span(dst, &mask[src_index..src_index + span], color);
            }
            PixelOp::BlendImage { rgba, .. } => {
                blend_image_span(dst, &rgba[src_index * 4..(src_index + span) * 4]);
            }
            _ => {}
        }
    }
    Ok(())
}

/// Integer `round(t / 255)` for `t <= 255 * 255`, exact for every input in that range.
#[inline]
fn div255(t: u32) -> u32 {
    let t = t + 128;
    (t + (t >> 8)) >> 8
}

/// Source-over of straight `color` through per-pixel `mask` coverage onto RGBA8 `dst`.
/// Zero coverage leaves the pixel, full coverage writes the colour, and the rest blend.
fn blend_mask_span(dst: &mut [u8], mask: &[u8], color: [u8; 4]) {
    let colour_alpha = u32::from(color[3]);
    for (pixel, &coverage) in dst.chunks_exact_mut(4).zip(mask) {
        let a = div255(colour_alpha * u32::from(coverage));
        match a {
            0 => {}
            255 => pixel.copy_from_slice(&[color[0], color[1], color[2], 255]),
            _ => {
                let inverse = 255 - a;
                for channel in 0..3 {
                    pixel[channel] = div255(
                        u32::from(color[channel]) * a + u32::from(pixel[channel]) * inverse,
                    ) as u8;
                }
                pixel[3] = (a + div255(u32::from(pixel[3]) * inverse)) as u8;
            }
        }
    }
}

/// Premultiplied source-over of `rgba` onto RGBA8 `dst`. Transparent source pixels leave the
/// destination, opaque ones copy.
fn blend_image_span(dst: &mut [u8], rgba: &[u8]) {
    for (pixel, src) in dst.chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
        match src[3] {
            0 => {}
            255 => pixel.copy_from_slice(src),
            alpha => {
                let inverse = u32::from(255 - alpha);
                for channel in 0..3 {
                    pixel[channel] = (u32::from(src[channel])
                        + div255(u32::from(pixel[channel]) * inverse))
                        as u8;
                }
                pixel[3] = (u32::from(alpha) + div255(u32::from(pixel[3]) * inverse)) as u8;
            }
        }
    }
}

/// Pixel operations on straight RGBA8 buffers.
///
/// Fills are hard-edged: a pixel is either inside the rounded rectangle or not. Compositing is
/// `out.rgb = src.rgb * a + dst.rgb * (1 - a)` and `out.a = a + dst.a * (1 - a)` with
/// `a = color[3] / 255`, so the CPU and GPU backends compute the same result. Implementations must
/// leave `pixels` untouched when they return `Err`, so a caller can run another backend on the
/// same input.
pub trait PixelBackend {
    fn blur_rgba(
        &mut self,
        pixels: &mut [u8],
        width: u32,
        height: u32,
        radius: f32,
    ) -> Result<(), String>;

    /// Composites `color` through the coverage of `outer` minus `hole`. Both rectangles must be
    /// normalized by the caller.
    fn fill_rounded_rect(
        &mut self,
        pixels: &mut [u8],
        width: u32,
        height: u32,
        outer: RoundedRect,
        hole: Option<RoundedRect>,
        color: [u8; 4],
    ) -> Result<(), String>;

    /// Applies `ops` in order to one frame whose op rectangles are normalized. The default runs
    /// each op on a scratch copy and copies it back only when every op succeeds. A GPU backend
    /// overrides it to upload the frame once, run every op on the frame-resident target and read
    /// it back once.
    fn paint_batch(
        &mut self,
        pixels: &mut [u8],
        width: u32,
        height: u32,
        ops: &[PixelOp<'_>],
    ) -> Result<(), String> {
        let mut scratch = pixels.to_vec();
        for op in ops {
            match *op {
                PixelOp::FillRoundedRect { outer, hole, color } => {
                    self.fill_rounded_rect(&mut scratch, width, height, outer, hole, color)?;
                }
                PixelOp::Blur { radius } => self.blur_rgba(&mut scratch, width, height, radius)?,
                PixelOp::FillRoundedRectClipped {
                    outer,
                    hole,
                    color,
                    clip,
                } => fill_clipped_cpu(self, &mut scratch, width, height, outer, hole, color, clip)?,
                PixelOp::BackdropBlur {
                    clip,
                    node,
                    corner,
                    radius,
                } => backdrop_blur_cpu(self, &mut scratch, width, height, clip, node, corner, radius)?,
                blend @ (PixelOp::BlendMask { .. } | PixelOp::BlendImage { .. }) => {
                    composite_cpu(&mut scratch, width, height, blend)?;
                }
            }
        }
        pixels.copy_from_slice(&scratch);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_values_parse_case_insensitively_and_default_to_cpu() {
        assert_eq!(RenderAcceleration::from_setting("GPU"), RenderAcceleration::Gpu);
        assert_eq!(RenderAcceleration::from_setting(" gpu "), RenderAcceleration::Gpu);
        assert_eq!(RenderAcceleration::from_setting("cpu"), RenderAcceleration::Cpu);
        assert_eq!(RenderAcceleration::from_setting("vulkan"), RenderAcceleration::Cpu);
        assert_eq!(RenderAcceleration::Gpu.as_setting(), "gpu");
    }

    #[test]
    fn radius_is_clamped_and_stroke_inset_shrinks_the_corner() {
        let rect = RoundedRect { x: 2, y: 3, width: 10, height: 6, radius: 9 };
        assert_eq!(rect.normalized().radius, 3);
        let inner = rect.normalized().inset(1);
        assert_eq!(inner, RoundedRect { x: 3, y: 4, width: 8, height: 4, radius: 2 });
    }

    /// CPU-only backend: the default `paint_batch` runs its blend ops through `composite_cpu`.
    struct Cpu;

    impl PixelBackend for Cpu {
        fn blur_rgba(&mut self, _: &mut [u8], _: u32, _: u32, _: f32) -> Result<(), String> {
            Ok(())
        }

        fn fill_rounded_rect(
            &mut self,
            _: &mut [u8],
            _: u32,
            _: u32,
            _: RoundedRect,
            _: Option<RoundedRect>,
            _: [u8; 4],
        ) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn blend_mask_tints_coverage_and_skips_pixels_outside_the_frame() {
        // Three opaque grey pixels. The mask starts one column left of the frame, so its first
        // column is skipped and its second column (coverage 128) lands on pixel 0.
        let mut frame = [100, 100, 100, 255, 100, 100, 100, 255, 100, 100, 100, 255];
        let op = PixelOp::BlendMask {
            x: -1,
            y: 0,
            width: 2,
            height: 1,
            mask: &[255, 128],
            color: [200, 0, 0, 255],
        };
        Cpu.paint_batch(&mut frame, 3, 1, &[op]).expect("mask blends");
        assert_eq!(&frame[0..4], &[150, 50, 50, 255]);
        assert_eq!(&frame[4..], &[100, 100, 100, 255, 100, 100, 100, 255]);
    }

    /// The float rule the integer spans replaced, per pixel.
    fn float_composite(pixels: &mut [u8], width: u32, op: PixelOp<'_>, x: i32, y: i32, rect_w: u32, rect_h: u32) {
        for row in 0..rect_h {
            for col in 0..rect_w {
                let (fx, fy) = (x + col as i32, y + row as i32);
                if fx < 0 || fy < 0 || fx >= width as i32 || fy >= (pixels.len() / 4 / width as usize) as i32 {
                    continue;
                }
                let s = (row * rect_w + col) as usize;
                let i = (fy as usize * width as usize + fx as usize) * 4;
                let dst = &mut pixels[i..i + 4];
                match op {
                    PixelOp::BlendMask { mask, color, .. } => {
                        let a = f32::from(color[3]) * f32::from(mask[s]) / 65_025.0;
                        for c in 0..3 {
                            dst[c] = (f32::from(color[c]) * a + f32::from(dst[c]) * (1.0 - a)).round() as u8;
                        }
                        dst[3] = (a * 255.0 + f32::from(dst[3]) * (1.0 - a)).round() as u8;
                    }
                    PixelOp::BlendImage { rgba, .. } => {
                        let src = &rgba[s * 4..s * 4 + 4];
                        let a = f32::from(src[3]) / 255.0;
                        for c in 0..3 {
                            dst[c] = (f32::from(src[c]) + f32::from(dst[c]) * (1.0 - a)).round() as u8;
                        }
                        dst[3] = (f32::from(src[3]) + f32::from(dst[3]) * (1.0 - a)).round() as u8;
                    }
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn integer_blend_spans_stay_within_one_level_of_the_float_rule() {
        let (width, height) = (23_u32, 17_u32);
        let base: Vec<u8> = (0..width * height * 4).map(|i| (i * 37 % 256) as u8).collect();
        let mask: Vec<u8> = (0..12 * 9).map(|i| (i * 29 % 256) as u8).collect();
        let rgba: Vec<u8> = (0..12 * 9).flat_map(|i| {
            let a = (i * 53 % 256) as u8;
            [a / 2, (a / 3) , (a / 5), a]
        }).collect();
        for (x, y) in [(-4, -3), (0, 0), (15, 10), (20, 14), (-20, 5)] {
            for color in [[200, 10, 90, 255], [30, 200, 60, 128], [0, 0, 0, 1]] {
                let op = PixelOp::BlendMask { x, y, width: 12, height: 9, mask: &mask, color };
                let mut got = base.clone();
                composite_cpu(&mut got, width, height, op).expect("mask");
                let mut want = base.clone();
                float_composite(&mut want, width, op, x, y, 12, 9);
                for (g, w) in got.iter().zip(&want) {
                    assert!(g.abs_diff(*w) <= 1, "mask at {x},{y} colour {color:?}: {g} vs {w}");
                }
            }
            let op = PixelOp::BlendImage { x, y, width: 12, height: 9, rgba: &rgba };
            let mut got = base.clone();
            composite_cpu(&mut got, width, height, op).expect("image");
            let mut want = base.clone();
            float_composite(&mut want, width, op, x, y, 12, 9);
            for (g, w) in got.iter().zip(&want) {
                assert!(g.abs_diff(*w) <= 1, "image at {x},{y}: {g} vs {w}");
            }
        }
    }

    #[test]
    fn blend_image_is_premultiplied_source_over() {
        let mut frame = [0, 0, 0, 255, 0, 0, 0, 255];
        let op = PixelOp::BlendImage {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            rgba: &[128, 0, 0, 128],
        };
        Cpu.paint_batch(&mut frame, 2, 1, &[op]).expect("image blends");
        assert_eq!(&frame[0..4], &[128, 0, 0, 255]);
        assert_eq!(&frame[4..], &[0, 0, 0, 255]);
    }

    #[test]
    fn a_failing_blend_leaves_the_frame_untouched() {
        let mut frame = [9_u8; 8];
        let ops = [
            PixelOp::FillRoundedRect {
                outer: RoundedRect { x: 0, y: 0, width: 2, height: 1, radius: 0 },
                hole: None,
                color: [1, 2, 3, 255],
            },
            PixelOp::BlendMask {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
                mask: &[255],
                color: [0, 0, 0, 255],
            },
        ];
        assert!(Cpu.paint_batch(&mut frame, 2, 1, &ops).is_err());
        assert_eq!(frame, [9_u8; 8], "an error must not commit the earlier fill");
    }
}
