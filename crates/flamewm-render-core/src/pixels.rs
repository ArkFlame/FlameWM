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
    for row in 0..rect_height {
        let frame_y = y + row as i32;
        if !(0..height as i32).contains(&frame_y) {
            continue;
        }
        for column in 0..rect_width {
            let frame_x = x + column as i32;
            if !(0..width as i32).contains(&frame_x) {
                continue;
            }
            let source = (row as usize * rect_width as usize + column as usize) * stride;
            let index = (frame_y as usize * width as usize + frame_x as usize) * 4;
            let dst = &mut pixels[index..index + 4];
            match op {
                PixelOp::BlendMask { mask, color, .. } => {
                    let alpha = f32::from(color[3]) * f32::from(mask[source]) / 65_025.0;
                    for channel in 0..3 {
                        dst[channel] = (f32::from(color[channel]) * alpha
                            + f32::from(dst[channel]) * (1.0 - alpha))
                            .round() as u8;
                    }
                    dst[3] = (alpha * 255.0 + f32::from(dst[3]) * (1.0 - alpha)).round() as u8;
                }
                PixelOp::BlendImage { rgba, .. } => {
                    let src = &rgba[source..source + 4];
                    let alpha = f32::from(src[3]) / 255.0;
                    for channel in 0..3 {
                        dst[channel] = (f32::from(src[channel])
                            + f32::from(dst[channel]) * (1.0 - alpha))
                            .round() as u8;
                    }
                    dst[3] = (f32::from(src[3]) + f32::from(dst[3]) * (1.0 - alpha)).round() as u8;
                }
                _ => {}
            }
        }
    }
    Ok(())
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
